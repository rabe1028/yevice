//! CFn template → Architecture conversion using the adapter registry.

use std::collections::{HashMap, HashSet};

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
            let shell = match adapters.lookup(&cfn.resource_type) {
                None => ResourceShell::other(&cfn.resource_type),
                Some(adapter) => match adapter.convert(&raw) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!(
                            resource_type = %cfn.resource_type,
                            error = %e,
                            "adapter failed to convert; treating as unsupported"
                        );
                        ResourceShell::other(&cfn.resource_type)
                    }
                },
            };
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

/// Insert `conn` into `connections` if its key is not already in `seen`,
/// after passing the endpoint guard determined by `require_source_in_resources`.
///
/// - ESM edges: `require_source_in_resources = false` (source may be external ARN)
/// - All new structured-property edges: `require_source_in_resources = true`
fn try_push_connection(
    conn: Connection,
    resources: &HashMap<String, CfnResource>,
    require_source_in_resources: bool,
    seen: &mut HashSet<(String, String, String)>,
    connections: &mut Vec<Connection>,
) {
    if require_source_in_resources && !resources.contains_key(conn.source.as_str()) {
        return;
    }
    if !resources.contains_key(conn.target.as_str()) {
        return;
    }
    let key = (
        conn.source.as_str().to_string(),
        conn.target.as_str().to_string(),
        format!("{:?}", conn.connection_type),
    );
    if seen.insert(key) {
        connections.push(conn);
    }
}

fn build_connections(resources: &HashMap<String, CfnResource>) -> Vec<Connection> {
    let mut connections = Vec::new();
    // Dedup key: (source, target, connection_type) — prevents double-counting
    // when both EventSourceMapping and SAM Events create the same edge.
    let mut seen: HashSet<(String, String, String)> = HashSet::new();

    // ---- 1. AWS::Lambda::EventSourceMapping (existing) ----
    // Source may be an external ARN not in resources; only the target must exist.
    for cfn in resources.values() {
        if cfn.resource_type == "AWS::Lambda::EventSourceMapping"
            && let Some(conn) = extract_event_source_connection(cfn, resources)
        {
            try_push_connection(conn, resources, false, &mut seen, &mut connections);
        }
    }

    // ---- 2. AWS::S3::Bucket NotificationConfiguration ----
    for (id, cfn) in resources {
        if cfn.resource_type == "AWS::S3::Bucket" {
            for conn in extract_s3_notification_connections(id, cfn) {
                try_push_connection(conn, resources, true, &mut seen, &mut connections);
            }
        }
    }

    // ---- 3. AWS::SNS::Topic Subscription / AWS::SNS::Subscription ----
    for (id, cfn) in resources {
        match cfn.resource_type.as_str() {
            "AWS::SNS::Topic" => {
                for conn in extract_sns_topic_subscription_connections(id, cfn) {
                    try_push_connection(conn, resources, true, &mut seen, &mut connections);
                }
            }
            "AWS::SNS::Subscription" => {
                if let Some(conn) = extract_sns_subscription_resource_connection(cfn) {
                    try_push_connection(conn, resources, true, &mut seen, &mut connections);
                }
            }
            _ => {}
        }
    }

    // ---- 4. AWS::Events::Rule Targets ----
    for (id, cfn) in resources {
        if cfn.resource_type == "AWS::Events::Rule" {
            for conn in extract_events_rule_connections(id, cfn) {
                try_push_connection(conn, resources, true, &mut seen, &mut connections);
            }
        }
    }

    // ---- 5. AWS::Serverless::Function Events (SQS/Kinesis/DynamoDB types) ----
    // Source must be a known node in the template (no external ARN supported here).
    for (id, cfn) in resources {
        if cfn.resource_type == "AWS::Serverless::Function" {
            for conn in extract_sam_function_event_connections(id, cfn, resources) {
                try_push_connection(conn, resources, true, &mut seen, &mut connections);
            }
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

/// Extract the logical ID from a sentinel string produced by the intrinsic resolver.
///
/// The intrinsic resolver always runs before connection extraction, so a
/// reference to another resource is one of these sentinel forms. Anything else
/// (literal strings, ARNs, names) is intentionally NOT treated as a logical ID
/// to avoid spurious edges to same-named resources.
///
/// Handles:
/// - `"{{ref:X}}"` → `Some("X")`
/// - `"{{getatt:X.Attr}}"` → `Some("X")`
fn extract_logical_id_from_sentinel(s: &str) -> Option<String> {
    if let Some(rest) = s.strip_prefix("{{ref:") {
        return rest.strip_suffix("}}").map(String::from);
    }
    if let Some(rest) = s.strip_prefix("{{getatt:") {
        return rest
            .strip_suffix("}}")
            .and_then(|inner| inner.split('.').next())
            .map(String::from);
    }
    None
}

/// Make a simple connection with no batch_size / parallelization / factor.
fn simple_connection(
    source: &str,
    target: &str,
    connection_type: ConnectionType,
) -> Connection {
    Connection {
        source: LogicalId::new(source),
        target: LogicalId::new(target),
        connection_type,
        batch_size: None,
        parallelization_factor: None,
        factor: None,
        source_hint: None,
    }
}

// ---------------------------------------------------------------------------
// S3 NotificationConfiguration
// ---------------------------------------------------------------------------

fn extract_s3_notification_connections(
    bucket_id: &str,
    cfn: &CfnResource,
) -> Vec<Connection> {
    let mut conns = Vec::new();
    let Some(props) = cfn.properties.as_mapping() else {
        return conns;
    };
    let Some(notif) = props.get(YamlValue::String("NotificationConfiguration".into())) else {
        return conns;
    };
    let Some(notif_map) = notif.as_mapping() else {
        return conns;
    };

    // LambdaConfigurations (cfn) or LambdaFunctionConfigurations (SAM/CDK alias)
    for key in &[
        "LambdaConfigurations",
        "LambdaFunctionConfigurations",
    ] {
        if let Some(items) = notif_map
            .get(YamlValue::String((*key).into()))
            .and_then(|v| v.as_sequence())
        {
            for item in items {
                if let Some(m) = item.as_mapping() {
                    // Function can be in "Function" (cfn) or "LambdaFunctionArn" (cfn)
                    let fn_value = m
                        .get(YamlValue::String("Function".into()))
                        .or_else(|| m.get(YamlValue::String("LambdaFunctionArn".into())));
                    if let Some(v) = fn_value
                        && let Some(s) = v.as_str()
                        && let Some(target_id) = extract_logical_id_from_sentinel(s)
                    {
                        conns.push(simple_connection(
                            bucket_id,
                            &target_id,
                            ConnectionType::Notification,
                        ));
                    }
                }
            }
        }
    }

    // QueueConfigurations
    if let Some(items) = notif_map
        .get(YamlValue::String("QueueConfigurations".into()))
        .and_then(|v| v.as_sequence())
    {
        for item in items {
            if let Some(m) = item.as_mapping() {
                let queue_value = m
                    .get(YamlValue::String("Queue".into()))
                    .or_else(|| m.get(YamlValue::String("QueueArn".into())));
                if let Some(v) = queue_value
                    && let Some(s) = v.as_str()
                    && let Some(target_id) = extract_logical_id_from_sentinel(s)
                {
                    conns.push(simple_connection(
                        bucket_id,
                        &target_id,
                        ConnectionType::Notification,
                    ));
                }
            }
        }
    }

    // TopicConfigurations
    if let Some(items) = notif_map
        .get(YamlValue::String("TopicConfigurations".into()))
        .and_then(|v| v.as_sequence())
    {
        for item in items {
            if let Some(m) = item.as_mapping() {
                let topic_value = m
                    .get(YamlValue::String("Topic".into()))
                    .or_else(|| m.get(YamlValue::String("TopicArn".into())));
                if let Some(v) = topic_value
                    && let Some(s) = v.as_str()
                    && let Some(target_id) = extract_logical_id_from_sentinel(s)
                {
                    conns.push(simple_connection(
                        bucket_id,
                        &target_id,
                        ConnectionType::Notification,
                    ));
                }
            }
        }
    }

    conns
}

// ---------------------------------------------------------------------------
// SNS Topic Subscription (inline in AWS::SNS::Topic Properties.Subscription)
// ---------------------------------------------------------------------------

fn extract_sns_topic_subscription_connections(
    topic_id: &str,
    cfn: &CfnResource,
) -> Vec<Connection> {
    let mut conns = Vec::new();
    let Some(props) = cfn.properties.as_mapping() else {
        return conns;
    };
    let Some(subs) = props
        .get(YamlValue::String("Subscription".into()))
        .and_then(|v| v.as_sequence())
    else {
        return conns;
    };

    for sub in subs {
        if let Some(m) = sub.as_mapping() {
            let endpoint = m.get(YamlValue::String("Endpoint".into()));
            if let Some(v) = endpoint
                && let Some(s) = v.as_str()
                && let Some(target_id) = extract_logical_id_from_sentinel(s)
            {
                conns.push(simple_connection(
                    topic_id,
                    &target_id,
                    ConnectionType::Notification,
                ));
            }
        }
    }

    conns
}

// ---------------------------------------------------------------------------
// AWS::SNS::Subscription resource (standalone)
// ---------------------------------------------------------------------------

fn extract_sns_subscription_resource_connection(cfn: &CfnResource) -> Option<Connection> {
    let props = cfn.properties.as_mapping()?;

    let topic_arn = props.get(YamlValue::String("TopicArn".into()))?;
    let source_id = topic_arn
        .as_str()
        .and_then(extract_logical_id_from_sentinel)?;

    let endpoint = props.get(YamlValue::String("Endpoint".into()))?;
    let target_id = endpoint
        .as_str()
        .and_then(extract_logical_id_from_sentinel)?;

    Some(simple_connection(&source_id, &target_id, ConnectionType::Notification))
}

// ---------------------------------------------------------------------------
// AWS::Events::Rule Targets
// ---------------------------------------------------------------------------

fn extract_events_rule_connections(
    rule_id: &str,
    cfn: &CfnResource,
) -> Vec<Connection> {
    let mut conns = Vec::new();
    let Some(props) = cfn.properties.as_mapping() else {
        return conns;
    };
    let Some(targets) = props
        .get(YamlValue::String("Targets".into()))
        .and_then(|v| v.as_sequence())
    else {
        return conns;
    };

    for target in targets {
        if let Some(m) = target.as_mapping() {
            let arn_val = m.get(YamlValue::String("Arn".into()));
            if let Some(v) = arn_val
                && let Some(s) = v.as_str()
                && let Some(target_id) = extract_logical_id_from_sentinel(s)
            {
                conns.push(simple_connection(
                    rule_id,
                    &target_id,
                    ConnectionType::Invocation,
                ));
            }
        }
    }

    conns
}

// ---------------------------------------------------------------------------
// AWS::Serverless::Function Events (SQS / Kinesis / DynamoDB types only)
// ---------------------------------------------------------------------------

fn extract_sam_function_event_connections(
    function_id: &str,
    cfn: &CfnResource,
    resources: &HashMap<String, CfnResource>,
) -> Vec<Connection> {
    let mut conns = Vec::new();
    let Some(props) = cfn.properties.as_mapping() else {
        return conns;
    };
    let Some(events) = props
        .get(YamlValue::String("Events".into()))
        .and_then(|v| v.as_mapping())
    else {
        return conns;
    };

    for (_event_name, event_val) in events {
        let Some(event_map) = event_val.as_mapping() else {
            continue;
        };
        let event_type = event_map
            .get(YamlValue::String("Type".into()))
            .and_then(|v| v.as_str());
        let Some(event_type) = event_type else {
            continue;
        };

        // Only handle stream/queue event sources that create EventSource edges.
        match event_type {
            "SQS" | "Kinesis" | "DynamoDB" => {}
            _ => continue,
        }

        let event_props = event_map
            .get(YamlValue::String("Properties".into()))
            .and_then(|v| v.as_mapping());
        let Some(event_props) = event_props else {
            continue;
        };

        // Stream field name: SQS uses "Queue", Kinesis/DynamoDB use "Stream".
        let source_key = match event_type {
            "SQS" => "Queue",
            _ => "Stream",
        };

        let source_val = event_props.get(YamlValue::String(source_key.into()));
        let Some(source_val) = source_val else {
            continue;
        };
        let Some(source_str) = source_val.as_str() else {
            continue;
        };
        let Some(source_id) = extract_logical_id_from_sentinel(source_str) else {
            continue;
        };

        // Verify the source logical ID exists in the resources map.
        let Some(source_cfn) = resources.get(&source_id) else {
            continue;
        };

        // Confirm source type matches the event type.
        let expected_type = match event_type {
            "SQS" => "AWS::SQS::Queue",
            "Kinesis" => "AWS::Kinesis::Stream",
            "DynamoDB" => "AWS::DynamoDB::Table",
            _ => continue,
        };
        if source_cfn.resource_type != expected_type {
            continue;
        }

        let batch_size = event_props
            .get(YamlValue::String("BatchSize".into()))
            .and_then(|v| {
                v.as_f64()
                    .or_else(|| v.as_u64().map(|n| n as f64))
                    .or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()))
            });
        let parallelization = event_props
            .get(YamlValue::String("ParallelizationFactor".into()))
            .and_then(|v| {
                v.as_f64()
                    .or_else(|| v.as_u64().map(|n| n as f64))
                    .or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()))
            });

        conns.push(Connection {
            source: LogicalId::new(&source_id),
            target: LogicalId::new(function_id),
            connection_type: ConnectionType::EventSource,
            batch_size,
            parallelization_factor: parallelization,
            factor: None,
            source_hint: None,
        });
    }

    conns
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
