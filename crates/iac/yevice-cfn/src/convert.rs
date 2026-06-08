//! CFn template → Architecture conversion using the adapter registry.

use std::collections::HashMap;

use serde_yaml_ng::Value as YamlValue;
use yevice_core::{
    resource::{Architecture, Connection, ConnectionType, Resource, ResourceShell},
    types::{LogicalId, Region, ResourceType},
};
use yevice_service_api::{CfnAdapterRegistry, RawCfnResource};

use crate::parser::{CfnResource, CfnTemplate};

/// Convert a resolved CFn template to an Architecture using the adapter registry.
pub fn build_architecture(
    name: &str,
    region: &str,
    template: &CfnTemplate,
    adapters: &CfnAdapterRegistry,
) -> Architecture {
    let resources: Vec<Resource> = template
        .resources
        .iter()
        .map(|(logical_id, cfn)| {
            let properties = yaml_to_json(&cfn.properties);
            let raw =
                RawCfnResource::new(logical_id.as_str(), cfn.resource_type.as_str(), properties);
            let shell = adapters
                .lookup(&cfn.resource_type)
                .and_then(|adapter| adapter.convert(&raw).ok())
                .unwrap_or_else(|| ResourceShell::other(&cfn.resource_type));
            Resource {
                logical_id: LogicalId::new(logical_id),
                resource_type: ResourceType::new(&cfn.resource_type),
                shell,
            }
        })
        .collect();

    let connections = build_connections(&template.resources);

    Architecture {
        name: name.to_string(),
        region: Region::new(region),
        resources,
        connections,
    }
}

fn build_connections(resources: &HashMap<String, CfnResource>) -> Vec<Connection> {
    let mut connections = Vec::new();

    for cfn in resources.values() {
        if cfn.resource_type == "AWS::Lambda::EventSourceMapping"
            && let Some(conn) = extract_event_source_connection(cfn, resources)
        {
            connections.push(conn);
        }
    }

    connections
}

fn extract_event_source_connection(
    esm: &CfnResource,
    resources: &HashMap<String, CfnResource>,
) -> Option<Connection> {
    let props = esm.properties.as_mapping()?;

    let batch_size = get_yaml_number(&esm.properties, "BatchSize");
    let parallelization = get_yaml_number(&esm.properties, "ParallelizationFactor");

    let target_id = extract_function_logical_id(props)?;
    let (source_id, source_type) = extract_source_logical_id(props, resources)?;

    Some(Connection {
        source: LogicalId::new(&source_id),
        target: LogicalId::new(&target_id),
        connection_type: ConnectionType::EventSource,
        batch_size,
        parallelization_factor: parallelization,
        factor: None,
        source_hint: Some(source_type),
    })
}

fn extract_function_logical_id(props: &serde_yaml_ng::Mapping) -> Option<String> {
    let fn_name = props.get(YamlValue::String("FunctionName".into()))?;
    if let Some(s) = fn_name.as_str() {
        if let Some(rest) = s.strip_prefix("{{getatt:") {
            return rest
                .strip_suffix("}}")
                .and_then(|s| s.split('.').next())
                .map(String::from);
        }
        return Some(s.to_string());
    }
    None
}

fn extract_source_logical_id(
    props: &serde_yaml_ng::Mapping,
    resources: &HashMap<String, CfnResource>,
) -> Option<(String, String)> {
    let source_arn = props.get(YamlValue::String("EventSourceArn".into()))?;

    if let Some(s) = source_arn.as_str() {
        // Resolved !GetAtt: "{{getatt:QueueName.Arn}}"
        if let Some(rest) = s.strip_prefix("{{getatt:") {
            let logical_id = rest.strip_suffix("}}")?.split('.').next()?;
            let source_type = detect_source_type(logical_id, resources)?;
            return Some((logical_id.to_string(), source_type));
        }
        // From ARN pattern
        if s.contains(":sqs:") {
            return Some((arn_last_segment(s), "sqs".to_string()));
        }
        if s.contains(":kinesis:") {
            return Some((arn_last_segment(s), "kinesis".to_string()));
        }
        if s.contains(":dynamodb:") && s.contains("/stream/") {
            return Some((arn_last_segment(s), "dynamodb".to_string()));
        }
    }
    None
}

fn detect_source_type(
    logical_id: &str,
    resources: &HashMap<String, CfnResource>,
) -> Option<String> {
    let resource = resources.get(logical_id)?;
    match resource.resource_type.as_str() {
        "AWS::SQS::Queue" => Some("sqs".to_string()),
        "AWS::Kinesis::Stream" => Some("kinesis".to_string()),
        "AWS::DynamoDB::Table" => Some("dynamodb".to_string()),
        _ => None,
    }
}

fn arn_last_segment(arn: &str) -> String {
    arn.rsplit(':').next().unwrap_or("unknown").to_string()
}

fn get_yaml_number(value: &YamlValue, key: &str) -> Option<f64> {
    value
        .as_mapping()
        .and_then(|m| m.get(YamlValue::String(key.into())))
        .and_then(|v| {
            v.as_f64()
                .or_else(|| v.as_u64().map(|n| n as f64))
                .or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()))
        })
}

/// Convert a `serde_yaml_ng::Value` to a `serde_json::Value`.
///
/// This is needed because `RawCfnResource.properties` expects `serde_json::Value`.
fn yaml_to_json(value: &YamlValue) -> serde_json::Value {
    match value {
        YamlValue::Null => serde_json::Value::Null,
        YamlValue::Bool(b) => serde_json::Value::Bool(*b),
        YamlValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                serde_json::Value::Number(i.into())
            } else if let Some(u) = n.as_u64() {
                serde_json::Value::Number(u.into())
            } else if let Some(f) = n.as_f64() {
                serde_json::Number::from_f64(f)
                    .map_or(serde_json::Value::Null, serde_json::Value::Number)
            } else {
                serde_json::Value::Null
            }
        }
        YamlValue::String(s) => serde_json::Value::String(s.clone()),
        YamlValue::Sequence(seq) => {
            serde_json::Value::Array(seq.iter().map(yaml_to_json).collect())
        }
        YamlValue::Mapping(map) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in map {
                if let Some(key_str) = k.as_str() {
                    obj.insert(key_str.to_string(), yaml_to_json(v));
                }
            }
            serde_json::Value::Object(obj)
        }
        // Tagged values (unresolved intrinsics) become a string representation
        YamlValue::Tagged(tagged) => serde_json::Value::String(format!("{:?}", tagged.value)),
    }
}
