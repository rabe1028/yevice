//! CFn template → Architecture conversion using the adapter registry.

use std::collections::{HashMap, HashSet};

use serde_yaml_ng::Value as YamlValue;
use yevice_core::{
    resource::{Architecture, Connection, ConnectionType, Resource, ResourceShell},
    types::{LogicalId, Region, ResourceType},
};
use yevice_service_api::{CfnAdapterRegistry, RawCfnResource};

use crate::parser::{CfnResource, CfnTemplate};
use crate::sentinel;

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
                group: extract_group(cfn, &template.resources),
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

    // Sort by logical ID for deterministic output (HashMap iteration order is unspecified).
    let mut entries: Vec<(&String, &CfnResource)> = resources.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));

    for (id, cfn) in entries {
        match cfn.resource_type.as_str() {
            // ESM: source may be an external ARN not in resources; only target must exist.
            "AWS::Lambda::EventSourceMapping" => {
                if let Some(conn) = extract_event_source_connection(cfn, resources) {
                    try_push_connection(conn, resources, false, &mut seen, &mut connections);
                }
            }
            "AWS::S3::Bucket" => {
                for conn in extract_s3_notification_connections(id, cfn) {
                    try_push_connection(conn, resources, true, &mut seen, &mut connections);
                }
            }
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
            "AWS::Events::Rule" => {
                for conn in extract_events_rule_connections(id, cfn) {
                    try_push_connection(conn, resources, true, &mut seen, &mut connections);
                }
            }
            // SAM: source must be a known node in the template (no external ARN supported here).
            "AWS::Serverless::Function" => {
                for conn in extract_sam_function_event_connections(id, cfn, resources) {
                    try_push_connection(conn, resources, true, &mut seen, &mut connections);
                }
            }
            _ => {}
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
    let s = fn_name.as_str()?;
    if let Some(cfn_ref) = sentinel::parse(s) {
        return Some(cfn_ref.logical_id);
    }
    Some(s.to_string())
}

fn extract_source_logical_id(
    props: &serde_yaml_ng::Mapping,
    resources: &HashMap<String, CfnResource>,
) -> Option<(String, String)> {
    let source_arn = props.get(YamlValue::String("EventSourceArn".into()))?;

    if let Some(s) = source_arn.as_str() {
        // Resolved sentinel: "{{ref:X}}" or "{{getatt:X.Attr}}"
        if let Some(cfn_ref) = sentinel::parse(s) {
            let source_type = detect_source_type(&cfn_ref.logical_id, resources)?;
            return Some((cfn_ref.logical_id, source_type));
        }
        // From ARN pattern (literal ARN, not a sentinel)
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
/// - Whole-string sentinels: `"{{ref:X}}"` → `Some("X")`
/// - Whole-string sentinels: `"{{getatt:X.Attr}}"` → `Some("X")`
/// - Embedded sentinels (e.g. `Fn::Sub` ARNs after resolution):
///   `"arn:...:function:{{ref:X}}"` → `Some("X")` (first embedded sentinel wins)
fn extract_logical_id_from_sentinel(s: &str) -> Option<String> {
    sentinel::parse(s)
        .or_else(|| sentinel::find_embedded(s))
        .map(|r| r.logical_id)
}

/// Determine the containment parent for a CFn resource.
///
/// Checks a prioritized list of single-reference properties and returns the
/// logical ID of the first one that resolves to a known resource in `resources`.
///
/// Priority: `Cluster` → `ClusterName` → `SubnetId` → `VpcId`.
///
/// Array/multi-reference properties (e.g. `SubnetIds`) are intentionally skipped
/// because they cannot unambiguously identify a single parent.
///
/// Returns `None` when:
/// - no matching property is found,
/// - the resolved logical ID does not exist in `resources` (dangling parent), or
/// - the resolved logical ID equals the resource's own logical ID (self-reference).
fn extract_group(cfn: &CfnResource, resources: &HashMap<String, CfnResource>) -> Option<LogicalId> {
    // Ordered list of single-reference property names to probe.
    const SINGLE_REF_PROPS: &[&str] = &["Cluster", "ClusterName", "SubnetId", "VpcId"];

    let props = cfn.properties.as_mapping()?;

    for &prop in SINGLE_REF_PROPS {
        let Some(val) = props.get(YamlValue::String(prop.into())) else {
            continue;
        };
        let Some(s) = val.as_str() else {
            continue;
        };
        let Some(parent_id) = extract_logical_id_from_sentinel(s) else {
            continue;
        };
        // Skip self-references.
        if parent_id == cfn.logical_id {
            continue;
        }
        // Only accept the parent if it exists in the template.
        if resources.contains_key(&parent_id) {
            return Some(LogicalId::new(&parent_id));
        }
    }

    None
}

/// Make a simple connection with no batch_size / parallelization / factor.
fn simple_connection(source: &str, target: &str, connection_type: ConnectionType) -> Connection {
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

/// Returns `true` when a notification config item targets an `s3:ObjectCreated`
/// event.  Only `s3:ObjectCreated:*` / `:Put` / `:Post` etc. (any sub-type)
/// should produce a cost-model edge, because the source-rate variable bound in
/// the cost model is derived from `put_requests` and is semantically meaningful
/// only for object-creation events.
fn is_object_created_event(item: &serde_yaml_ng::Mapping) -> bool {
    item.get(serde_yaml_ng::Value::String("Event".into()))
        .and_then(serde_yaml_ng::Value::as_str)
        .is_some_and(|e| e.starts_with("s3:ObjectCreated"))
}

fn extract_s3_notification_connections(bucket_id: &str, cfn: &CfnResource) -> Vec<Connection> {
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
    for key in &["LambdaConfigurations", "LambdaFunctionConfigurations"] {
        if let Some(items) = notif_map
            .get(YamlValue::String((*key).into()))
            .and_then(|v| v.as_sequence())
        {
            for item in items {
                if let Some(m) = item.as_mapping() {
                    if !is_object_created_event(m) {
                        continue;
                    }
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
                if !is_object_created_event(m) {
                    continue;
                }
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
                if !is_object_created_event(m) {
                    continue;
                }
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

    Some(simple_connection(
        &source_id,
        &target_id,
        ConnectionType::Notification,
    ))
}

// ---------------------------------------------------------------------------
// AWS::Events::Rule Targets
// ---------------------------------------------------------------------------

fn extract_events_rule_connections(rule_id: &str, cfn: &CfnResource) -> Vec<Connection> {
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

#[cfg(test)]
mod containment_tests {
    use super::*;

    // Build a minimal CfnResource with the given logical_id and a YamlValue mapping for properties.
    fn cfn_resource(logical_id: &str, properties: YamlValue) -> CfnResource {
        CfnResource {
            logical_id: logical_id.to_string(),
            resource_type: "AWS::Unknown".to_string(),
            properties,
            condition: None,
            depends_on: Vec::new(),
        }
    }

    // Build a YamlValue mapping from key-value pairs.
    fn yaml_map(pairs: &[(&str, &str)]) -> YamlValue {
        let mut map = serde_yaml_ng::Mapping::new();
        for &(k, v) in pairs {
            map.insert(
                YamlValue::String(k.to_string()),
                YamlValue::String(v.to_string()),
            );
        }
        YamlValue::Mapping(map)
    }

    #[test]
    fn vpc_id_sentinel_resolves_to_vpc_group() {
        let vpc = cfn_resource("MyVpc", yaml_map(&[]));
        let subnet = cfn_resource("MySubnet", yaml_map(&[("VpcId", "{{ref:MyVpc}}")]));

        let mut resources = HashMap::new();
        resources.insert("MyVpc".to_string(), vpc);
        resources.insert("MySubnet".to_string(), subnet);

        let group = extract_group(&resources["MySubnet"], &resources);
        assert_eq!(group, Some(LogicalId::new("MyVpc")));
    }

    #[test]
    fn subnet_id_sentinel_resolves_to_subnet_group() {
        let subnet = cfn_resource("MySubnet", yaml_map(&[]));
        let nat = cfn_resource("MyNat", yaml_map(&[("SubnetId", "{{ref:MySubnet}}")]));

        let mut resources = HashMap::new();
        resources.insert("MySubnet".to_string(), subnet);
        resources.insert("MyNat".to_string(), nat);

        let group = extract_group(&resources["MyNat"], &resources);
        assert_eq!(group, Some(LogicalId::new("MySubnet")));
    }

    #[test]
    fn cluster_sentinel_resolves_to_cluster_group() {
        let cluster = cfn_resource("MyCluster", yaml_map(&[]));
        let service = cfn_resource("MyService", yaml_map(&[("Cluster", "{{ref:MyCluster}}")]));

        let mut resources = HashMap::new();
        resources.insert("MyCluster".to_string(), cluster);
        resources.insert("MyService".to_string(), service);

        let group = extract_group(&resources["MyService"], &resources);
        assert_eq!(group, Some(LogicalId::new("MyCluster")));
    }

    #[test]
    fn cluster_takes_priority_over_vpc_id() {
        // Both Cluster and VpcId present — Cluster wins (higher priority).
        let cluster = cfn_resource("MyCluster", yaml_map(&[]));
        let vpc = cfn_resource("MyVpc", yaml_map(&[]));
        let resource = cfn_resource(
            "MyResource",
            yaml_map(&[("Cluster", "{{ref:MyCluster}}"), ("VpcId", "{{ref:MyVpc}}")]),
        );

        let mut resources = HashMap::new();
        resources.insert("MyCluster".to_string(), cluster);
        resources.insert("MyVpc".to_string(), vpc);
        resources.insert("MyResource".to_string(), resource);

        let group = extract_group(&resources["MyResource"], &resources);
        assert_eq!(group, Some(LogicalId::new("MyCluster")));
    }

    #[test]
    fn dangling_parent_yields_no_group() {
        // SubnetId points to a logical ID not present in resources.
        let nat = cfn_resource(
            "MyNat",
            yaml_map(&[("SubnetId", "{{ref:NonExistentSubnet}}")]),
        );

        let mut resources = HashMap::new();
        resources.insert("MyNat".to_string(), nat);

        let group = extract_group(&resources["MyNat"], &resources);
        assert_eq!(group, None);
    }

    #[test]
    fn literal_property_value_yields_no_group() {
        // A plain string (not a sentinel) must not be treated as a logical ID.
        let nat = cfn_resource("MyNat", yaml_map(&[("SubnetId", "subnet-12345678")]));

        let mut resources = HashMap::new();
        resources.insert("MyNat".to_string(), nat);

        let group = extract_group(&resources["MyNat"], &resources);
        assert_eq!(group, None);
    }

    #[test]
    fn no_containment_properties_yields_no_group() {
        let lambda = cfn_resource("MyFunction", yaml_map(&[("MemorySize", "256")]));

        let mut resources = HashMap::new();
        resources.insert("MyFunction".to_string(), lambda);

        let group = extract_group(&resources["MyFunction"], &resources);
        assert_eq!(group, None);
    }

    #[test]
    fn getatt_sentinel_resolves_group() {
        let subnet = cfn_resource("MySubnet", yaml_map(&[]));
        let instance = cfn_resource(
            "MyInstance",
            yaml_map(&[("SubnetId", "{{getatt:MySubnet.SubnetId}}")]),
        );

        let mut resources = HashMap::new();
        resources.insert("MySubnet".to_string(), subnet);
        resources.insert("MyInstance".to_string(), instance);

        let group = extract_group(&resources["MyInstance"], &resources);
        assert_eq!(group, Some(LogicalId::new("MySubnet")));
    }
}

#[cfg(test)]
mod connection_tests {
    use super::*;

    fn cfn_resource_typed(
        logical_id: &str,
        resource_type: &str,
        properties: YamlValue,
    ) -> CfnResource {
        CfnResource {
            logical_id: logical_id.to_string(),
            resource_type: resource_type.to_string(),
            properties,
            condition: None,
            depends_on: Vec::new(),
        }
    }

    fn yaml_str(s: &str) -> YamlValue {
        YamlValue::String(s.to_string())
    }

    fn yaml_seq(items: Vec<YamlValue>) -> YamlValue {
        YamlValue::Sequence(items)
    }

    fn yaml_map_values(pairs: Vec<(&str, YamlValue)>) -> YamlValue {
        let mut map = serde_yaml_ng::Mapping::new();
        for (k, v) in pairs {
            map.insert(YamlValue::String(k.to_string()), v);
        }
        YamlValue::Mapping(map)
    }

    /// `AWS::Events::Rule` with a bare sentinel Arn must still create an
    /// Invocation edge (existing behavior must not regress).
    #[test]
    fn events_rule_bare_sentinel_arn_creates_edge() {
        let lambda = cfn_resource_typed(
            "HandlerFunction",
            "AWS::Lambda::Function",
            yaml_map_values(vec![]),
        );

        let target_entry = yaml_map_values(vec![
            ("Id", yaml_str("TargetId")),
            ("Arn", yaml_str("{{ref:HandlerFunction}}")),
        ]);
        let rule_props = yaml_map_values(vec![("Targets", yaml_seq(vec![target_entry]))]);
        let rule = cfn_resource_typed("MyRule", "AWS::Events::Rule", rule_props);

        let mut resources = HashMap::new();
        resources.insert("HandlerFunction".to_string(), lambda);
        resources.insert("MyRule".to_string(), rule);

        let conns = build_connections(&resources);
        assert_eq!(conns.len(), 1, "expected one Invocation edge");
        assert_eq!(conns[0].source.as_str(), "MyRule");
        assert_eq!(conns[0].target.as_str(), "HandlerFunction");
        assert!(matches!(
            conns[0].connection_type,
            ConnectionType::Invocation
        ));
    }

    /// `AWS::Events::Rule` with an embedded sentinel inside a full-ARN `Fn::Sub`
    /// value must create an Invocation edge to the target Lambda.
    #[test]
    fn events_rule_embedded_sentinel_arn_creates_edge() {
        let lambda = cfn_resource_typed(
            "HandlerFunction",
            "AWS::Lambda::Function",
            yaml_map_values(vec![]),
        );

        // Simulates what the intrinsic resolver produces for:
        //   Arn: !Sub 'arn:aws:lambda:${AWS::Region}:${AWS::AccountId}:function:${HandlerFunction}'
        // After resolution: pseudo-params are left verbatim, resource refs become sentinels.
        let embedded_arn =
            "arn:aws:lambda:${AWS::Region}:${AWS::AccountId}:function:{{ref:HandlerFunction}}";
        let target_entry = yaml_map_values(vec![
            ("Id", yaml_str("TargetId")),
            ("Arn", yaml_str(embedded_arn)),
        ]);
        let rule_props = yaml_map_values(vec![("Targets", yaml_seq(vec![target_entry]))]);
        let rule = cfn_resource_typed("MyRule", "AWS::Events::Rule", rule_props);

        let mut resources = HashMap::new();
        resources.insert("HandlerFunction".to_string(), lambda);
        resources.insert("MyRule".to_string(), rule);

        let conns = build_connections(&resources);
        assert_eq!(
            conns.len(),
            1,
            "expected one Invocation edge for embedded-sentinel ARN"
        );
        assert_eq!(conns[0].source.as_str(), "MyRule");
        assert_eq!(conns[0].target.as_str(), "HandlerFunction");
        assert!(matches!(
            conns[0].connection_type,
            ConnectionType::Invocation
        ));
    }

    // -----------------------------------------------------------------------
    // S3 NotificationConfiguration event-type gating tests
    // -----------------------------------------------------------------------

    /// Helper: build a LambdaConfiguration item with the given `Event` string.
    fn lambda_config_item(event: &str, function_sentinel: &str) -> YamlValue {
        yaml_map_values(vec![
            ("Event", yaml_str(event)),
            ("Function", yaml_str(function_sentinel)),
        ])
    }

    /// Helper: build a QueueConfiguration item with the given `Event` string.
    fn queue_config_item(event: &str, queue_sentinel: &str) -> YamlValue {
        yaml_map_values(vec![
            ("Event", yaml_str(event)),
            ("Queue", yaml_str(queue_sentinel)),
        ])
    }

    /// Helper: build a TopicConfiguration item with the given `Event` string.
    fn topic_config_item(event: &str, topic_sentinel: &str) -> YamlValue {
        yaml_map_values(vec![
            ("Event", yaml_str(event)),
            ("Topic", yaml_str(topic_sentinel)),
        ])
    }

    /// `s3:ObjectCreated:*` lambda notification must produce a Notification edge.
    #[test]
    fn s3_object_created_lambda_notification_produces_edge() {
        let lambda = cfn_resource_typed(
            "MyFunction",
            "AWS::Lambda::Function",
            yaml_map_values(vec![]),
        );
        let notif_config = yaml_map_values(vec![(
            "LambdaConfigurations",
            yaml_seq(vec![lambda_config_item(
                "s3:ObjectCreated:*",
                "{{ref:MyFunction}}",
            )]),
        )]);
        let bucket_props = yaml_map_values(vec![("NotificationConfiguration", notif_config)]);
        let bucket = cfn_resource_typed("MyBucket", "AWS::S3::Bucket", bucket_props);

        let mut resources = HashMap::new();
        resources.insert("MyFunction".to_string(), lambda);
        resources.insert("MyBucket".to_string(), bucket);

        let conns = build_connections(&resources);
        let notif = conns.iter().find(|c| {
            c.source.as_str() == "MyBucket"
                && c.target.as_str() == "MyFunction"
                && matches!(c.connection_type, ConnectionType::Notification)
        });
        assert!(
            notif.is_some(),
            "expected Notification edge for s3:ObjectCreated:*; connections = {conns:?}",
        );
    }

    /// `s3:ObjectRemoved:*` lambda notification must NOT produce a Notification edge.
    #[test]
    fn s3_object_removed_lambda_notification_skipped() {
        let lambda = cfn_resource_typed(
            "MyFunction",
            "AWS::Lambda::Function",
            yaml_map_values(vec![]),
        );
        let notif_config = yaml_map_values(vec![(
            "LambdaConfigurations",
            yaml_seq(vec![lambda_config_item(
                "s3:ObjectRemoved:*",
                "{{ref:MyFunction}}",
            )]),
        )]);
        let bucket_props = yaml_map_values(vec![("NotificationConfiguration", notif_config)]);
        let bucket = cfn_resource_typed("MyBucket", "AWS::S3::Bucket", bucket_props);

        let mut resources = HashMap::new();
        resources.insert("MyFunction".to_string(), lambda);
        resources.insert("MyBucket".to_string(), bucket);

        let conns = build_connections(&resources);
        assert!(
            conns.is_empty(),
            "expected no edge for s3:ObjectRemoved:*; connections = {conns:?}",
        );
    }

    /// `s3:ObjectRemoved:*` queue notification must NOT produce a Notification edge.
    #[test]
    fn s3_object_removed_queue_notification_skipped() {
        let queue = cfn_resource_typed("MyQueue", "AWS::SQS::Queue", yaml_map_values(vec![]));
        let notif_config = yaml_map_values(vec![(
            "QueueConfigurations",
            yaml_seq(vec![queue_config_item(
                "s3:ObjectRemoved:*",
                "{{ref:MyQueue}}",
            )]),
        )]);
        let bucket_props = yaml_map_values(vec![("NotificationConfiguration", notif_config)]);
        let bucket = cfn_resource_typed("MyBucket", "AWS::S3::Bucket", bucket_props);

        let mut resources = HashMap::new();
        resources.insert("MyQueue".to_string(), queue);
        resources.insert("MyBucket".to_string(), bucket);

        let conns = build_connections(&resources);
        assert!(
            conns.is_empty(),
            "expected no edge for s3:ObjectRemoved:* on queue; connections = {conns:?}",
        );
    }

    /// `s3:ObjectRemoved:*` topic notification must NOT produce a Notification edge.
    #[test]
    fn s3_object_removed_topic_notification_skipped() {
        let topic = cfn_resource_typed("MyTopic", "AWS::SNS::Topic", yaml_map_values(vec![]));
        let notif_config = yaml_map_values(vec![(
            "TopicConfigurations",
            yaml_seq(vec![topic_config_item(
                "s3:ObjectRemoved:*",
                "{{ref:MyTopic}}",
            )]),
        )]);
        let bucket_props = yaml_map_values(vec![("NotificationConfiguration", notif_config)]);
        let bucket = cfn_resource_typed("MyBucket", "AWS::S3::Bucket", bucket_props);

        let mut resources = HashMap::new();
        resources.insert("MyTopic".to_string(), topic);
        resources.insert("MyBucket".to_string(), bucket);

        let conns = build_connections(&resources);
        assert!(
            conns.is_empty(),
            "expected no edge for s3:ObjectRemoved:* on topic; connections = {conns:?}",
        );
    }

    /// Mixed: one ObjectCreated + one ObjectRemoved lambda config → only one edge.
    #[test]
    fn s3_mixed_events_only_object_created_produces_edge() {
        let lambda_a = cfn_resource_typed("FnA", "AWS::Lambda::Function", yaml_map_values(vec![]));
        let lambda_b = cfn_resource_typed("FnB", "AWS::Lambda::Function", yaml_map_values(vec![]));
        let notif_config = yaml_map_values(vec![(
            "LambdaConfigurations",
            yaml_seq(vec![
                lambda_config_item("s3:ObjectCreated:*", "{{ref:FnA}}"),
                lambda_config_item("s3:ObjectRemoved:*", "{{ref:FnB}}"),
            ]),
        )]);
        let bucket_props = yaml_map_values(vec![("NotificationConfiguration", notif_config)]);
        let bucket = cfn_resource_typed("MyBucket", "AWS::S3::Bucket", bucket_props);

        let mut resources = HashMap::new();
        resources.insert("FnA".to_string(), lambda_a);
        resources.insert("FnB".to_string(), lambda_b);
        resources.insert("MyBucket".to_string(), bucket);

        let conns = build_connections(&resources);
        assert_eq!(
            conns.len(),
            1,
            "expected exactly one Notification edge (only ObjectCreated); connections = {conns:?}",
        );
        assert_eq!(conns[0].source.as_str(), "MyBucket");
        assert_eq!(conns[0].target.as_str(), "FnA");
        assert!(matches!(
            conns[0].connection_type,
            ConnectionType::Notification
        ));
    }
}
