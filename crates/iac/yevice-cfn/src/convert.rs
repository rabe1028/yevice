//! CFn template → Architecture conversion using the adapter registry.

use std::collections::BTreeMap;

use serde_yaml_ng::Value as YamlValue;
use yevice_core::{
    resource::{
        Architecture, Connection, ConnectionDeduper, ConnectionType, Resource, ResourceShell,
    },
    types::{LogicalId, Region, ResourceType},
};
use yevice_service_api::{CfnAdapterRegistry, CfnPropertyValue, RawCfnResource};

use crate::parser::{ResolvedResource, ResolvedTemplate};
use crate::resolved::{Reference, ResolvedValue, StringPart, render_parts};

/// Convert a resolved CFn template to an Architecture using the adapter registry.
pub fn build_architecture(
    name: &str,
    region: &str,
    template: &ResolvedTemplate,
    adapters: &CfnAdapterRegistry,
) -> Architecture {
    let resources: Vec<Resource> = template
        .resources
        .iter()
        .map(|(logical_id, cfn)| {
            let properties = resolved_to_cfn_properties(&cfn.properties);
            let raw = RawCfnResource {
                logical_id: LogicalId::new(logical_id.as_str()),
                resource_type: ResourceType::new(cfn.resource_type.as_str()),
                properties,
            };
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

/// Insert `conn` into `dedupe` after passing the endpoint guard.
///
/// `require_source_in_resources` controls the source endpoint check:
///
/// - ESM edges: `false` (source may be an external ARN, not a template node)
/// - All structured-property edges: `true` (both endpoints must be nodes)
///
/// Target existence is always enforced.
fn try_push_connection(
    conn: Connection,
    resources: &BTreeMap<String, ResolvedResource>,
    require_source_in_resources: bool,
    dedupe: &mut ConnectionDeduper,
) {
    dedupe.try_push(
        conn,
        |src| !require_source_in_resources || resources.contains_key(src),
        |tgt| resources.contains_key(tgt),
    );
}

fn build_connections(resources: &BTreeMap<String, ResolvedResource>) -> Vec<Connection> {
    // Dedup key: (source, target, connection_type) — prevents double-counting
    // when both EventSourceMapping and SAM Events create the same edge.
    let mut dedupe = ConnectionDeduper::new();

    for (id, cfn) in resources {
        match cfn.resource_type.as_str() {
            // ESM: source may be an external ARN not in resources; only target must exist.
            "AWS::Lambda::EventSourceMapping" => {
                for conn in extract_event_source_connections(cfn, resources) {
                    try_push_connection(conn, resources, false, &mut dedupe);
                }
            }
            "AWS::S3::Bucket" => {
                for conn in extract_s3_notification_connections(id, cfn) {
                    try_push_connection(conn, resources, true, &mut dedupe);
                }
            }
            "AWS::SNS::Topic" => {
                for conn in extract_sns_topic_subscription_connections(id, cfn) {
                    try_push_connection(conn, resources, true, &mut dedupe);
                }
            }
            "AWS::SNS::Subscription" => {
                for conn in extract_sns_subscription_resource_connections(cfn) {
                    try_push_connection(conn, resources, true, &mut dedupe);
                }
            }
            "AWS::Events::Rule" => {
                for conn in extract_events_rule_connections(id, cfn) {
                    try_push_connection(conn, resources, true, &mut dedupe);
                }
            }
            // SAM: source must be a known node in the template (no external ARN supported here).
            "AWS::Serverless::Function" => {
                for conn in extract_sam_function_event_connections(id, cfn, resources) {
                    try_push_connection(conn, resources, true, &mut dedupe);
                }
            }
            _ => {}
        }
    }

    dedupe.into_connections()
}

fn extract_event_source_connections(
    esm: &ResolvedResource,
    resources: &BTreeMap<String, ResolvedResource>,
) -> Vec<Connection> {
    let props = &esm.properties;

    let batch_size = get_resolved_number(props, "BatchSize");
    let parallelization = get_resolved_number(props, "ParallelizationFactor");

    let targets = extract_function_logical_ids(props);
    let sources = extract_source_logical_ids(props, resources);

    let mut conns = Vec::new();
    for (source_id, source_type) in &sources {
        for target_id in &targets {
            conns.push(Connection {
                source: LogicalId::new(source_id),
                target: LogicalId::new(target_id),
                connection_type: ConnectionType::EventSource,
                batch_size,
                parallelization_factor: parallelization,
                factor: None,
                source_hint: Some(source_type.clone()),
            });
        }
    }
    conns
}

/// Candidate target logical IDs from an ESM `FunctionName` property.
///
/// All typed references (including every part of an `Interpolated` value such
/// as a `Fn::Sub` ARN) are candidates. A plain literal string is also accepted
/// as a candidate name; `try_push_connection` later verifies it exists in the
/// template.
fn extract_function_logical_ids(props: &ResolvedValue) -> Vec<String> {
    let Some(fn_name) = props.get("FunctionName") else {
        return Vec::new();
    };
    let refs = fn_name.references();
    if !refs.is_empty() {
        return refs.into_iter().map(|r| r.logical_id).collect();
    }
    fn_name
        .as_str()
        .map(|s| vec![s.to_string()])
        .unwrap_or_default()
}

/// Candidate (source logical ID, source type) pairs from an ESM
/// `EventSourceArn` property.
fn extract_source_logical_ids(
    props: &ResolvedValue,
    resources: &BTreeMap<String, ResolvedResource>,
) -> Vec<(String, String)> {
    let Some(source_arn) = props.get("EventSourceArn") else {
        return Vec::new();
    };

    let refs = source_arn.references();
    if !refs.is_empty() {
        return refs
            .into_iter()
            .filter_map(|r| detect_source_type(&r.logical_id, resources).map(|t| (r.logical_id, t)))
            .collect();
    }

    // From ARN pattern (literal ARN, not a reference)
    if let Some(s) = source_arn.as_str() {
        if s.contains(":sqs:") {
            return vec![(arn_last_segment(s), "sqs".to_string())];
        }
        if s.contains(":kinesis:") {
            return vec![(arn_last_segment(s), "kinesis".to_string())];
        }
        if s.contains(":dynamodb:") && s.contains("/stream/") {
            return vec![(arn_last_segment(s), "dynamodb".to_string())];
        }
    }
    Vec::new()
}

fn detect_source_type(
    logical_id: &str,
    resources: &BTreeMap<String, ResolvedResource>,
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

/// All resource references reachable from a property value, including every
/// part of an `Interpolated` string (a `Fn::Sub` ARN with multiple references
/// yields multiple entries).
///
/// Literal strings, ARNs, and names are intentionally NOT treated as logical
/// IDs to avoid spurious edges to same-named resources.
fn referenced_logical_ids(value: &ResolvedValue) -> Vec<Reference> {
    value.references()
}

/// Determine the containment parent for a CFn resource.
///
/// Checks a prioritized list of single-reference properties and returns the
/// logical ID of the first one that resolves to a known resource in `resources`.
///
/// Priority: `Cluster` → `ClusterName` → `SubnetId` → `VpcId`.
///
/// Array/multi-reference properties (e.g. `SubnetIds`) are intentionally skipped
/// because they cannot unambiguously identify a single parent. Likewise, a
/// property value carrying more than one reference (e.g. a `Fn::Join` of two
/// refs) is ambiguous and yields no parent.
///
/// Returns `None` when:
/// - no matching property is found,
/// - the property does not carry exactly one resource reference,
/// - the referenced logical ID does not exist in `resources` (dangling parent), or
/// - the referenced logical ID equals the resource's own logical ID (self-reference).
fn extract_group(
    cfn: &ResolvedResource,
    resources: &BTreeMap<String, ResolvedResource>,
) -> Option<LogicalId> {
    // Ordered list of single-reference property names to probe.
    const SINGLE_REF_PROPS: &[&str] = &["Cluster", "ClusterName", "SubnetId", "VpcId"];

    for &prop in SINGLE_REF_PROPS {
        let Some(val) = cfn.properties.get(prop) else {
            continue;
        };
        let refs = referenced_logical_ids(val);
        // Exactly one reference is required to identify a single parent.
        let [parent] = refs.as_slice() else {
            continue;
        };
        // Skip self-references.
        if parent.logical_id == cfn.logical_id {
            continue;
        }
        // Only accept the parent if it exists in the template.
        if resources.contains_key(&parent.logical_id) {
            return Some(LogicalId::new(&parent.logical_id));
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
fn is_object_created_event(item: &ResolvedValue) -> bool {
    item.get("Event")
        .and_then(ResolvedValue::as_str)
        .is_some_and(|e| e.starts_with("s3:ObjectCreated"))
}

fn extract_s3_notification_connections(bucket_id: &str, cfn: &ResolvedResource) -> Vec<Connection> {
    // (config list key candidates, target value key candidates)
    const CONFIG_KINDS: &[(&[&str], &[&str])] = &[
        // LambdaConfigurations (cfn) or LambdaFunctionConfigurations (SAM/CDK alias);
        // function can be in "Function" (cfn) or "LambdaFunctionArn" (cfn)
        (
            &["LambdaConfigurations", "LambdaFunctionConfigurations"],
            &["Function", "LambdaFunctionArn"],
        ),
        (&["QueueConfigurations"], &["Queue", "QueueArn"]),
        (&["TopicConfigurations"], &["Topic", "TopicArn"]),
    ];

    let mut conns = Vec::new();
    let Some(notif) = cfn.properties.get("NotificationConfiguration") else {
        return conns;
    };

    for (list_keys, value_keys) in CONFIG_KINDS {
        for list_key in *list_keys {
            let Some(items) = notif.get(list_key).and_then(ResolvedValue::as_seq) else {
                continue;
            };
            for item in items {
                if !is_object_created_event(item) {
                    continue;
                }
                let target_value = value_keys.iter().find_map(|k| item.get(k));
                let Some(v) = target_value else { continue };
                for r in referenced_logical_ids(v) {
                    conns.push(simple_connection(
                        bucket_id,
                        &r.logical_id,
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
    cfn: &ResolvedResource,
) -> Vec<Connection> {
    let mut conns = Vec::new();
    let Some(subs) = cfn
        .properties
        .get("Subscription")
        .and_then(ResolvedValue::as_seq)
    else {
        return conns;
    };

    for sub in subs {
        // Only emit edges for runtime protocols that invoke a Lambda or SQS
        // resource.  Non-runtime protocols (https, email, sms, http,
        // application, …) do not model a cost-graph edge.
        let protocol = sub
            .get("Protocol")
            .and_then(ResolvedValue::as_str)
            .unwrap_or("");
        if !matches!(protocol, "lambda" | "sqs") {
            continue;
        }
        let Some(endpoint) = sub.get("Endpoint") else {
            continue;
        };
        for r in referenced_logical_ids(endpoint) {
            conns.push(simple_connection(
                topic_id,
                &r.logical_id,
                ConnectionType::Notification,
            ));
        }
    }

    conns
}

// ---------------------------------------------------------------------------
// AWS::SNS::Subscription resource (standalone)
// ---------------------------------------------------------------------------

fn extract_sns_subscription_resource_connections(cfn: &ResolvedResource) -> Vec<Connection> {
    let props = &cfn.properties;

    // Only emit edges for runtime protocols (lambda / sqs).  Non-runtime
    // protocols (https, email, sms, http, application, …) are not wired into
    // the cost model.
    let protocol = props
        .get("Protocol")
        .and_then(ResolvedValue::as_str)
        .unwrap_or("");
    if !matches!(protocol, "lambda" | "sqs") {
        return Vec::new();
    }

    let sources: Vec<Reference> = props
        .get("TopicArn")
        .map(referenced_logical_ids)
        .unwrap_or_default();
    let targets: Vec<Reference> = props
        .get("Endpoint")
        .map(referenced_logical_ids)
        .unwrap_or_default();

    let mut conns = Vec::new();
    for source in &sources {
        for target in &targets {
            conns.push(simple_connection(
                &source.logical_id,
                &target.logical_id,
                ConnectionType::Notification,
            ));
        }
    }
    conns
}

// ---------------------------------------------------------------------------
// AWS::Events::Rule Targets
// ---------------------------------------------------------------------------

fn extract_events_rule_connections(rule_id: &str, cfn: &ResolvedResource) -> Vec<Connection> {
    let mut conns = Vec::new();
    let Some(targets) = cfn
        .properties
        .get("Targets")
        .and_then(ResolvedValue::as_seq)
    else {
        return conns;
    };

    for target in targets {
        let Some(arn) = target.get("Arn") else {
            continue;
        };
        // Every reference in the ARN value produces an edge — an Interpolated
        // `Fn::Sub` ARN carrying multiple references yields multiple edges.
        for r in referenced_logical_ids(arn) {
            conns.push(simple_connection(
                rule_id,
                &r.logical_id,
                ConnectionType::Invocation,
            ));
        }
    }

    conns
}

// ---------------------------------------------------------------------------
// AWS::Serverless::Function Events (SQS / Kinesis / DynamoDB / S3 types)
// ---------------------------------------------------------------------------

/// Returns `true` when the SAM S3 event `Events` field contains at least one
/// `s3:ObjectCreated` variant.  The field may be a single string or a YAML
/// sequence of strings (both are valid in SAM).
fn sam_s3_events_has_object_created(event_props: &ResolvedValue) -> bool {
    let Some(events_val) = event_props.get("Events") else {
        return false;
    };
    if let Some(s) = events_val.as_str() {
        return s.starts_with("s3:ObjectCreated");
    }
    events_val.as_seq().is_some_and(|seq| {
        seq.iter().any(|v| {
            v.as_str()
                .is_some_and(|s| s.starts_with("s3:ObjectCreated"))
        })
    })
}

fn extract_sam_function_event_connections(
    function_id: &str,
    cfn: &ResolvedResource,
    resources: &BTreeMap<String, ResolvedResource>,
) -> Vec<Connection> {
    let mut conns = Vec::new();
    let Some(ResolvedValue::Map(events)) = cfn.properties.get("Events") else {
        return conns;
    };

    for event_val in events.values() {
        let event_type = event_val.get("Type").and_then(ResolvedValue::as_str);
        let Some(event_type) = event_type else {
            continue;
        };

        let Some(event_props) = event_val.get("Properties") else {
            continue;
        };

        if event_type == "S3" {
            // SAM S3 event: emit a Notification edge from the bucket to the
            // function, mirroring what SAM transform writes into
            // AWS::S3::Bucket NotificationConfiguration.
            if !sam_s3_events_has_object_created(event_props) {
                continue;
            }
            let Some(bucket_val) = event_props.get("Bucket") else {
                continue;
            };
            for r in referenced_logical_ids(bucket_val) {
                if !resources.contains_key(&r.logical_id) {
                    continue;
                }
                conns.push(simple_connection(
                    &r.logical_id,
                    function_id,
                    ConnectionType::Notification,
                ));
            }
            continue;
        }

        // Only handle stream/queue event sources that create EventSource edges.
        match event_type {
            "SQS" | "Kinesis" | "DynamoDB" => {}
            _ => continue,
        }

        // Stream field name: SQS uses "Queue", Kinesis/DynamoDB use "Stream".
        let source_key = match event_type {
            "SQS" => "Queue",
            _ => "Stream",
        };

        let Some(source_val) = event_props.get(source_key) else {
            continue;
        };

        // Confirm source type matches the event type.
        let expected_type = match event_type {
            "SQS" => "AWS::SQS::Queue",
            "Kinesis" => "AWS::Kinesis::Stream",
            "DynamoDB" => "AWS::DynamoDB::Table",
            _ => continue,
        };

        let batch_size = get_resolved_number(event_props, "BatchSize");
        let parallelization = get_resolved_number(event_props, "ParallelizationFactor");

        for r in referenced_logical_ids(source_val) {
            // Verify the source logical ID exists in the resources map.
            let Some(source_cfn) = resources.get(&r.logical_id) else {
                continue;
            };
            if source_cfn.resource_type != expected_type {
                continue;
            }

            conns.push(Connection {
                source: LogicalId::new(&r.logical_id),
                target: LogicalId::new(function_id),
                connection_type: ConnectionType::EventSource,
                batch_size,
                parallelization_factor: parallelization,
                factor: None,
                source_hint: None,
            });
        }
    }

    conns
}

/// Extract a numeric property value (`Concrete` Number, or a numeric string).
fn get_resolved_number(props: &ResolvedValue, key: &str) -> Option<f64> {
    match props.get(key)? {
        ResolvedValue::Concrete(YamlValue::Number(n)) => {
            n.as_f64().or_else(|| n.as_u64().map(|u| u as f64))
        }
        ResolvedValue::Concrete(YamlValue::String(s)) => s.parse::<f64>().ok(),
        _ => None,
    }
}

/// Convert a resolved properties block to a
/// `BTreeMap<String, CfnPropertyValue>` for the adapter boundary.
///
/// Each top-level entry is converted: typed references (`Ref` / `GetAtt`,
/// including an `Interpolated` value consisting of a single reference) become
/// `ResourceRef` / `ResourceGetAtt`.  Everything else becomes `Concrete` JSON,
/// with references nested inside containers or mixed with literal text
/// rendered using the CFn-native `${LogicalId}` / `${LogicalId.Attr}` syntax.
fn resolved_to_cfn_properties(value: &ResolvedValue) -> BTreeMap<String, CfnPropertyValue> {
    match value {
        ResolvedValue::Map(map) => map
            .iter()
            .map(|(k, v)| (k.clone(), resolved_to_cfn_property(v)))
            .collect(),
        // Pass-through case (e.g. a Concrete mapping from hand-built input).
        ResolvedValue::Concrete(YamlValue::Mapping(map)) => map
            .iter()
            .filter_map(|(k, v)| {
                k.as_str()
                    .map(|key| (key.to_string(), CfnPropertyValue::Concrete(yaml_to_json(v))))
            })
            .collect(),
        _ => BTreeMap::new(),
    }
}

/// Convert a single top-level resolved property to a `CfnPropertyValue`.
fn resolved_to_cfn_property(value: &ResolvedValue) -> CfnPropertyValue {
    match value {
        ResolvedValue::Ref(id) => CfnPropertyValue::ResourceRef(id.clone()),
        ResolvedValue::GetAtt { logical_id, attr } => CfnPropertyValue::ResourceGetAtt {
            logical_id: logical_id.clone(),
            attr: attr.clone(),
        },
        ResolvedValue::Interpolated(parts) => {
            // Defensive: `ResolvedValue::from_parts` normalizes a lone
            // reference into the typed variants above, but promote it here
            // too in case an Interpolated was constructed directly.
            let non_literal: Vec<&StringPart> = parts
                .iter()
                .filter(|p| !matches!(p, StringPart::Literal(_)))
                .collect();
            if parts.len() == 1 && non_literal.len() == 1 {
                return match non_literal[0] {
                    StringPart::Ref(id) => CfnPropertyValue::ResourceRef(id.clone()),
                    StringPart::GetAtt { logical_id, attr } => CfnPropertyValue::ResourceGetAtt {
                        logical_id: logical_id.clone(),
                        attr: attr.clone(),
                    },
                    StringPart::Literal(_) => unreachable!("filtered above"),
                };
            }
            CfnPropertyValue::Concrete(serde_json::Value::String(render_parts(parts)))
        }
        other => CfnPropertyValue::Concrete(resolved_to_json(other)),
    }
}

/// Render a resolved value to JSON for adapters.
///
/// References are rendered with the CFn-native `${...}` substitution syntax;
/// adapters treat them as opaque strings.
fn resolved_to_json(value: &ResolvedValue) -> serde_json::Value {
    match value {
        ResolvedValue::Concrete(v) => yaml_to_json(v),
        ResolvedValue::Ref(id) => serde_json::Value::String(format!("${{{id}}}")),
        ResolvedValue::GetAtt { logical_id, attr } => {
            serde_json::Value::String(format!("${{{logical_id}.{attr}}}"))
        }
        ResolvedValue::Interpolated(parts) => serde_json::Value::String(render_parts(parts)),
        ResolvedValue::Seq(items) => {
            serde_json::Value::Array(items.iter().map(resolved_to_json).collect())
        }
        ResolvedValue::Map(map) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in map {
                obj.insert(k.clone(), resolved_to_json(v));
            }
            serde_json::Value::Object(obj)
        }
    }
}

/// Convert a `serde_yaml_ng::Value` to a `serde_json::Value`.
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
mod test_helpers {
    use super::*;

    pub fn rv_str(s: &str) -> ResolvedValue {
        ResolvedValue::Concrete(YamlValue::String(s.to_string()))
    }

    pub fn rv_ref(id: &str) -> ResolvedValue {
        ResolvedValue::Ref(id.to_string())
    }

    pub fn rv_getatt(logical_id: &str, attr: &str) -> ResolvedValue {
        ResolvedValue::GetAtt {
            logical_id: logical_id.to_string(),
            attr: attr.to_string(),
        }
    }

    pub fn rv_seq(items: Vec<ResolvedValue>) -> ResolvedValue {
        ResolvedValue::Seq(items)
    }

    pub fn rv_map(pairs: Vec<(&str, ResolvedValue)>) -> ResolvedValue {
        ResolvedValue::Map(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    pub fn part_lit(s: &str) -> StringPart {
        StringPart::Literal(s.to_string())
    }

    pub fn part_ref(id: &str) -> StringPart {
        StringPart::Ref(id.to_string())
    }

    pub fn part_getatt(logical_id: &str, attr: &str) -> StringPart {
        StringPart::GetAtt {
            logical_id: logical_id.to_string(),
            attr: attr.to_string(),
        }
    }

    pub fn cfn_resource_typed(
        logical_id: &str,
        resource_type: &str,
        properties: ResolvedValue,
    ) -> ResolvedResource {
        ResolvedResource {
            logical_id: logical_id.to_string(),
            resource_type: resource_type.to_string(),
            properties,
            condition: None,
            depends_on: Vec::new(),
        }
    }
}

#[cfg(test)]
mod containment_tests {
    use super::test_helpers::*;
    use super::*;

    // Build a minimal resource with the given logical_id and resolved properties.
    fn cfn_resource(logical_id: &str, properties: ResolvedValue) -> ResolvedResource {
        cfn_resource_typed(logical_id, "AWS::Unknown", properties)
    }

    #[test]
    fn vpc_id_ref_resolves_to_vpc_group() {
        let vpc = cfn_resource("MyVpc", rv_map(vec![]));
        let subnet = cfn_resource("MySubnet", rv_map(vec![("VpcId", rv_ref("MyVpc"))]));

        let mut resources = BTreeMap::new();
        resources.insert("MyVpc".to_string(), vpc);
        resources.insert("MySubnet".to_string(), subnet);

        let group = extract_group(&resources["MySubnet"], &resources);
        assert_eq!(group, Some(LogicalId::new("MyVpc")));
    }

    #[test]
    fn subnet_id_ref_resolves_to_subnet_group() {
        let subnet = cfn_resource("MySubnet", rv_map(vec![]));
        let nat = cfn_resource("MyNat", rv_map(vec![("SubnetId", rv_ref("MySubnet"))]));

        let mut resources = BTreeMap::new();
        resources.insert("MySubnet".to_string(), subnet);
        resources.insert("MyNat".to_string(), nat);

        let group = extract_group(&resources["MyNat"], &resources);
        assert_eq!(group, Some(LogicalId::new("MySubnet")));
    }

    #[test]
    fn cluster_ref_resolves_to_cluster_group() {
        let cluster = cfn_resource("MyCluster", rv_map(vec![]));
        let service = cfn_resource("MyService", rv_map(vec![("Cluster", rv_ref("MyCluster"))]));

        let mut resources = BTreeMap::new();
        resources.insert("MyCluster".to_string(), cluster);
        resources.insert("MyService".to_string(), service);

        let group = extract_group(&resources["MyService"], &resources);
        assert_eq!(group, Some(LogicalId::new("MyCluster")));
    }

    #[test]
    fn cluster_takes_priority_over_vpc_id() {
        // Both Cluster and VpcId present — Cluster wins (higher priority).
        let cluster = cfn_resource("MyCluster", rv_map(vec![]));
        let vpc = cfn_resource("MyVpc", rv_map(vec![]));
        let resource = cfn_resource(
            "MyResource",
            rv_map(vec![
                ("Cluster", rv_ref("MyCluster")),
                ("VpcId", rv_ref("MyVpc")),
            ]),
        );

        let mut resources = BTreeMap::new();
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
            rv_map(vec![("SubnetId", rv_ref("NonExistentSubnet"))]),
        );

        let mut resources = BTreeMap::new();
        resources.insert("MyNat".to_string(), nat);

        let group = extract_group(&resources["MyNat"], &resources);
        assert_eq!(group, None);
    }

    #[test]
    fn literal_property_value_yields_no_group() {
        // A plain string (not a reference) must not be treated as a logical ID.
        let nat = cfn_resource(
            "MyNat",
            rv_map(vec![("SubnetId", rv_str("subnet-12345678"))]),
        );

        let mut resources = BTreeMap::new();
        resources.insert("MyNat".to_string(), nat);

        let group = extract_group(&resources["MyNat"], &resources);
        assert_eq!(group, None);
    }

    #[test]
    fn no_containment_properties_yields_no_group() {
        let lambda = cfn_resource("MyFunction", rv_map(vec![("MemorySize", rv_str("256"))]));

        let mut resources = BTreeMap::new();
        resources.insert("MyFunction".to_string(), lambda);

        let group = extract_group(&resources["MyFunction"], &resources);
        assert_eq!(group, None);
    }

    #[test]
    fn getatt_resolves_group() {
        let subnet = cfn_resource("MySubnet", rv_map(vec![]));
        let instance = cfn_resource(
            "MyInstance",
            rv_map(vec![("SubnetId", rv_getatt("MySubnet", "SubnetId"))]),
        );

        let mut resources = BTreeMap::new();
        resources.insert("MySubnet".to_string(), subnet);
        resources.insert("MyInstance".to_string(), instance);

        let group = extract_group(&resources["MyInstance"], &resources);
        assert_eq!(group, Some(LogicalId::new("MySubnet")));
    }

    /// A property carrying more than one reference cannot unambiguously name
    /// a single parent — it must yield no group (this preserves the previous
    /// concatenated-sentinel fix semantics).
    #[test]
    fn multi_reference_property_yields_no_group() {
        let a = cfn_resource("ClusterA", rv_map(vec![]));
        let b = cfn_resource("ClusterB", rv_map(vec![]));
        let resource = cfn_resource(
            "MyResource",
            rv_map(vec![(
                "Cluster",
                ResolvedValue::Interpolated(vec![part_ref("ClusterA"), part_ref("ClusterB")]),
            )]),
        );

        let mut resources = BTreeMap::new();
        resources.insert("ClusterA".to_string(), a);
        resources.insert("ClusterB".to_string(), b);
        resources.insert("MyResource".to_string(), resource);

        let group = extract_group(&resources["MyResource"], &resources);
        assert_eq!(group, None, "ambiguous multi-ref must not pick a parent");
    }
}

#[cfg(test)]
mod connection_tests {
    use super::test_helpers::*;
    use super::*;

    /// `AWS::Events::Rule` with a typed Ref Arn must still create an
    /// Invocation edge (existing behavior must not regress).
    #[test]
    fn events_rule_typed_ref_arn_creates_edge() {
        let lambda = cfn_resource_typed("HandlerFunction", "AWS::Lambda::Function", rv_map(vec![]));

        let target_entry = rv_map(vec![
            ("Id", rv_str("TargetId")),
            ("Arn", rv_ref("HandlerFunction")),
        ]);
        let rule_props = rv_map(vec![("Targets", rv_seq(vec![target_entry]))]);
        let rule = cfn_resource_typed("MyRule", "AWS::Events::Rule", rule_props);

        let mut resources = BTreeMap::new();
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

    /// `AWS::Events::Rule` with a reference embedded inside a full-ARN
    /// `Fn::Sub` value (Interpolated) must create an Invocation edge.
    #[test]
    fn events_rule_interpolated_arn_creates_edge() {
        let lambda = cfn_resource_typed("HandlerFunction", "AWS::Lambda::Function", rv_map(vec![]));

        // Simulates what the intrinsic resolver produces for:
        //   Arn: !Sub 'arn:aws:lambda:${AWS::Region}:${AWS::AccountId}:function:${HandlerFunction}'
        // Pseudo-params stay literal text; the resource ref becomes a typed part.
        let embedded_arn = ResolvedValue::Interpolated(vec![
            part_lit("arn:aws:lambda:${AWS::Region}:${AWS::AccountId}:function:"),
            part_ref("HandlerFunction"),
        ]);
        let target_entry = rv_map(vec![("Id", rv_str("TargetId")), ("Arn", embedded_arn)]);
        let rule_props = rv_map(vec![("Targets", rv_seq(vec![target_entry]))]);
        let rule = cfn_resource_typed("MyRule", "AWS::Events::Rule", rule_props);

        let mut resources = BTreeMap::new();
        resources.insert("HandlerFunction".to_string(), lambda);
        resources.insert("MyRule".to_string(), rule);

        let conns = build_connections(&resources);
        assert_eq!(
            conns.len(),
            1,
            "expected one Invocation edge for interpolated ARN"
        );
        assert_eq!(conns[0].source.as_str(), "MyRule");
        assert_eq!(conns[0].target.as_str(), "HandlerFunction");
        assert!(matches!(
            conns[0].connection_type,
            ConnectionType::Invocation
        ));
    }

    /// **Regression test for issue #14**: a `Fn::Sub` value containing TWO
    /// resource references must produce TWO edges. The previous
    /// sentinel-based `find_embedded` only returned the first reference,
    /// silently dropping the second edge.
    #[test]
    fn events_rule_interpolated_arn_with_two_getatts_creates_two_edges() {
        let fn_a = cfn_resource_typed("Fn1", "AWS::Lambda::Function", rv_map(vec![]));
        let fn_b = cfn_resource_typed("Fn2", "AWS::Lambda::Function", rv_map(vec![]));

        // Simulates: Arn: !Sub '${Fn1.Arn}:${Fn2.Arn}'
        let arn = ResolvedValue::Interpolated(vec![
            part_getatt("Fn1", "Arn"),
            part_lit(":"),
            part_getatt("Fn2", "Arn"),
        ]);
        let target_entry = rv_map(vec![("Id", rv_str("TargetId")), ("Arn", arn)]);
        let rule_props = rv_map(vec![("Targets", rv_seq(vec![target_entry]))]);
        let rule = cfn_resource_typed("MyRule", "AWS::Events::Rule", rule_props);

        let mut resources = BTreeMap::new();
        resources.insert("Fn1".to_string(), fn_a);
        resources.insert("Fn2".to_string(), fn_b);
        resources.insert("MyRule".to_string(), rule);

        let conns = build_connections(&resources);
        assert_eq!(
            conns.len(),
            2,
            "expected two Invocation edges (one per reference); connections = {conns:?}"
        );
        let targets: Vec<&str> = conns.iter().map(|c| c.target.as_str()).collect();
        assert!(targets.contains(&"Fn1"), "edge to Fn1 missing: {targets:?}");
        assert!(targets.contains(&"Fn2"), "edge to Fn2 missing: {targets:?}");
    }

    /// **End-to-end regression test for issue #14**: from YAML template text
    /// through parse → resolve → build_connections, a `Fn::Sub` with two
    /// `GetAtt`-style references must produce two connection edges.
    #[test]
    fn end_to_end_sub_with_two_getatts_creates_two_edges() {
        const TEMPLATE: &str = r#"
AWSTemplateFormatVersion: "2010-09-09"
Resources:
  Fn1:
    Type: AWS::Lambda::Function
    Properties:
      MemorySize: 128
  Fn2:
    Type: AWS::Lambda::Function
    Properties:
      MemorySize: 128
  FanoutRule:
    Type: AWS::Events::Rule
    Properties:
      Targets:
        - Id: BothFunctions
          Arn: !Sub '${Fn1.Arn}:${Fn2.Arn}'
"#;
        let tmpl = crate::parser::parse_template_str(TEMPLATE).unwrap();
        let resources = crate::parser::resolve_template(
            &tmpl,
            &std::collections::HashMap::new(),
            &std::collections::HashMap::new(),
        )
        .unwrap();

        let conns = build_connections(&resources);
        let edge_to = |target: &str| {
            conns.iter().find(|c| {
                c.source.as_str() == "FanoutRule"
                    && c.target.as_str() == target
                    && matches!(c.connection_type, ConnectionType::Invocation)
            })
        };
        assert!(
            edge_to("Fn1").is_some(),
            "expected edge FanoutRule -> Fn1; connections = {conns:?}"
        );
        assert!(
            edge_to("Fn2").is_some(),
            "expected edge FanoutRule -> Fn2 (this edge was silently dropped \
             by the sentinel-based implementation); connections = {conns:?}"
        );
    }

    // -----------------------------------------------------------------------
    // S3 NotificationConfiguration event-type gating tests
    // -----------------------------------------------------------------------

    /// Helper: build a notification config item with the given `Event` string
    /// and target key/value.
    fn notif_item(event: &str, target_key: &str, target: ResolvedValue) -> ResolvedValue {
        rv_map(vec![("Event", rv_str(event)), (target_key, target)])
    }

    /// `s3:ObjectCreated:*` lambda notification must produce a Notification edge.
    #[test]
    fn s3_object_created_lambda_notification_produces_edge() {
        let lambda = cfn_resource_typed("MyFunction", "AWS::Lambda::Function", rv_map(vec![]));
        let notif_config = rv_map(vec![(
            "LambdaConfigurations",
            rv_seq(vec![notif_item(
                "s3:ObjectCreated:*",
                "Function",
                rv_ref("MyFunction"),
            )]),
        )]);
        let bucket_props = rv_map(vec![("NotificationConfiguration", notif_config)]);
        let bucket = cfn_resource_typed("MyBucket", "AWS::S3::Bucket", bucket_props);

        let mut resources = BTreeMap::new();
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
        let lambda = cfn_resource_typed("MyFunction", "AWS::Lambda::Function", rv_map(vec![]));
        let notif_config = rv_map(vec![(
            "LambdaConfigurations",
            rv_seq(vec![notif_item(
                "s3:ObjectRemoved:*",
                "Function",
                rv_ref("MyFunction"),
            )]),
        )]);
        let bucket_props = rv_map(vec![("NotificationConfiguration", notif_config)]);
        let bucket = cfn_resource_typed("MyBucket", "AWS::S3::Bucket", bucket_props);

        let mut resources = BTreeMap::new();
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
        let queue = cfn_resource_typed("MyQueue", "AWS::SQS::Queue", rv_map(vec![]));
        let notif_config = rv_map(vec![(
            "QueueConfigurations",
            rv_seq(vec![notif_item(
                "s3:ObjectRemoved:*",
                "Queue",
                rv_ref("MyQueue"),
            )]),
        )]);
        let bucket_props = rv_map(vec![("NotificationConfiguration", notif_config)]);
        let bucket = cfn_resource_typed("MyBucket", "AWS::S3::Bucket", bucket_props);

        let mut resources = BTreeMap::new();
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
        let topic = cfn_resource_typed("MyTopic", "AWS::SNS::Topic", rv_map(vec![]));
        let notif_config = rv_map(vec![(
            "TopicConfigurations",
            rv_seq(vec![notif_item(
                "s3:ObjectRemoved:*",
                "Topic",
                rv_ref("MyTopic"),
            )]),
        )]);
        let bucket_props = rv_map(vec![("NotificationConfiguration", notif_config)]);
        let bucket = cfn_resource_typed("MyBucket", "AWS::S3::Bucket", bucket_props);

        let mut resources = BTreeMap::new();
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
        let lambda_a = cfn_resource_typed("FnA", "AWS::Lambda::Function", rv_map(vec![]));
        let lambda_b = cfn_resource_typed("FnB", "AWS::Lambda::Function", rv_map(vec![]));
        let notif_config = rv_map(vec![(
            "LambdaConfigurations",
            rv_seq(vec![
                notif_item("s3:ObjectCreated:*", "Function", rv_ref("FnA")),
                notif_item("s3:ObjectRemoved:*", "Function", rv_ref("FnB")),
            ]),
        )]);
        let bucket_props = rv_map(vec![("NotificationConfiguration", notif_config)]);
        let bucket = cfn_resource_typed("MyBucket", "AWS::S3::Bucket", bucket_props);

        let mut resources = BTreeMap::new();
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

    // -----------------------------------------------------------------------
    // SAM AWS::Serverless::Function S3 event type tests
    // -----------------------------------------------------------------------

    /// Helper: build a SAM function with a single S3 event entry.
    fn sam_function_with_s3_event(
        function_id: &str,
        bucket: ResolvedValue,
        s3_events: ResolvedValue,
    ) -> ResolvedResource {
        let event_props = rv_map(vec![("Bucket", bucket), ("Events", s3_events)]);
        let event_entry = rv_map(vec![("Type", rv_str("S3")), ("Properties", event_props)]);
        let fn_props = rv_map(vec![("Events", rv_map(vec![("Photo", event_entry)]))]);
        cfn_resource_typed(function_id, "AWS::Serverless::Function", fn_props)
    }

    /// SAM `Type: S3` with `Events: s3:ObjectCreated:*` (string) must produce a
    /// Notification edge from the bucket to the function.
    #[test]
    fn sam_s3_event_object_created_string_produces_notification_edge() {
        let bucket = cfn_resource_typed("MediaBucket", "AWS::S3::Bucket", rv_map(vec![]));
        let function = sam_function_with_s3_event(
            "MyFunction",
            rv_ref("MediaBucket"),
            rv_str("s3:ObjectCreated:*"),
        );

        let mut resources = BTreeMap::new();
        resources.insert("MediaBucket".to_string(), bucket);
        resources.insert("MyFunction".to_string(), function);

        let conns = build_connections(&resources);
        let notif = conns.iter().find(|c| {
            c.source.as_str() == "MediaBucket"
                && c.target.as_str() == "MyFunction"
                && matches!(c.connection_type, ConnectionType::Notification)
        });
        assert!(
            notif.is_some(),
            "expected Notification edge for SAM S3 ObjectCreated:*; connections = {conns:?}",
        );
    }

    /// SAM `Type: S3` with `Events: [s3:ObjectCreated:Put]` (sequence) must
    /// produce a Notification edge.
    #[test]
    fn sam_s3_event_object_created_list_produces_notification_edge() {
        let bucket = cfn_resource_typed("MediaBucket", "AWS::S3::Bucket", rv_map(vec![]));
        let function = sam_function_with_s3_event(
            "MyFunction",
            rv_ref("MediaBucket"),
            rv_seq(vec![rv_str("s3:ObjectCreated:Put")]),
        );

        let mut resources = BTreeMap::new();
        resources.insert("MediaBucket".to_string(), bucket);
        resources.insert("MyFunction".to_string(), function);

        let conns = build_connections(&resources);
        let notif = conns.iter().find(|c| {
            c.source.as_str() == "MediaBucket"
                && c.target.as_str() == "MyFunction"
                && matches!(c.connection_type, ConnectionType::Notification)
        });
        assert!(
            notif.is_some(),
            "expected Notification edge for SAM S3 ObjectCreated:Put list; connections = {conns:?}",
        );
    }

    /// SAM `Type: S3` with `Events: s3:ObjectRemoved:*` must NOT produce any edge.
    #[test]
    fn sam_s3_event_object_removed_produces_no_edge() {
        let bucket = cfn_resource_typed("MediaBucket", "AWS::S3::Bucket", rv_map(vec![]));
        let function = sam_function_with_s3_event(
            "MyFunction",
            rv_ref("MediaBucket"),
            rv_str("s3:ObjectRemoved:*"),
        );

        let mut resources = BTreeMap::new();
        resources.insert("MediaBucket".to_string(), bucket);
        resources.insert("MyFunction".to_string(), function);

        let conns = build_connections(&resources);
        assert!(
            conns.is_empty(),
            "expected no edge for SAM S3 ObjectRemoved:*; connections = {conns:?}",
        );
    }

    /// SAM `Type: S3` where the bucket logical ID is absent from resources must
    /// NOT produce any edge (dangling bucket reference).
    #[test]
    fn sam_s3_event_dangling_bucket_produces_no_edge() {
        // Only the function is in resources; the bucket is not.
        let function = sam_function_with_s3_event(
            "MyFunction",
            rv_ref("NonExistentBucket"),
            rv_str("s3:ObjectCreated:*"),
        );

        let mut resources = BTreeMap::new();
        resources.insert("MyFunction".to_string(), function);

        let conns = build_connections(&resources);
        assert!(
            conns.is_empty(),
            "expected no edge for SAM S3 with dangling bucket; connections = {conns:?}",
        );
    }

    // -----------------------------------------------------------------------
    // SNS subscription protocol gating tests (CFN)
    // -----------------------------------------------------------------------

    /// Inline subscription with `Protocol: lambda` must produce a Notification edge.
    #[test]
    fn sns_topic_subscription_lambda_protocol_produces_edge() {
        let lambda = cfn_resource_typed("MyFunction", "AWS::Lambda::Function", rv_map(vec![]));
        let sub_item = rv_map(vec![
            ("Protocol", rv_str("lambda")),
            ("Endpoint", rv_ref("MyFunction")),
        ]);
        let topic_props = rv_map(vec![("Subscription", rv_seq(vec![sub_item]))]);
        let topic = cfn_resource_typed("MyTopic", "AWS::SNS::Topic", topic_props);

        let mut resources = BTreeMap::new();
        resources.insert("MyFunction".to_string(), lambda);
        resources.insert("MyTopic".to_string(), topic);

        let conns = build_connections(&resources);
        assert_eq!(
            conns.len(),
            1,
            "expected one Notification edge for lambda protocol; connections = {conns:?}"
        );
        assert_eq!(conns[0].source.as_str(), "MyTopic");
        assert_eq!(conns[0].target.as_str(), "MyFunction");
        assert!(matches!(
            conns[0].connection_type,
            ConnectionType::Notification
        ));
    }

    /// Inline subscription with `Protocol: https` must NOT produce any edge.
    #[test]
    fn sns_topic_subscription_https_protocol_produces_no_edge() {
        let topic_props = rv_map(vec![(
            "Subscription",
            rv_seq(vec![rv_map(vec![
                ("Protocol", rv_str("https")),
                (
                    "Endpoint",
                    ResolvedValue::Interpolated(vec![
                        part_lit("arn:aws:lambda:${AWS::Region}:${AWS::AccountId}:function:"),
                        part_ref("MyFn"),
                    ]),
                ),
            ])]),
        )]);
        let topic = cfn_resource_typed("MyTopic", "AWS::SNS::Topic", topic_props);
        let lambda = cfn_resource_typed("MyFn", "AWS::Lambda::Function", rv_map(vec![]));

        let mut resources = BTreeMap::new();
        resources.insert("MyTopic".to_string(), topic);
        resources.insert("MyFn".to_string(), lambda);

        let conns = build_connections(&resources);
        assert!(
            conns.is_empty(),
            "expected no edge for https protocol; connections = {conns:?}"
        );
    }

    /// `AWS::SNS::Subscription` resource with `Protocol: lambda` must produce
    /// a Notification edge.
    #[test]
    fn sns_subscription_resource_lambda_protocol_produces_edge() {
        let topic = cfn_resource_typed("MyTopic", "AWS::SNS::Topic", rv_map(vec![]));
        let lambda = cfn_resource_typed("MyFunction", "AWS::Lambda::Function", rv_map(vec![]));
        let sub_props = rv_map(vec![
            ("TopicArn", rv_ref("MyTopic")),
            ("Protocol", rv_str("lambda")),
            ("Endpoint", rv_ref("MyFunction")),
        ]);
        let sub = cfn_resource_typed("MySub", "AWS::SNS::Subscription", sub_props);

        let mut resources = BTreeMap::new();
        resources.insert("MyTopic".to_string(), topic);
        resources.insert("MyFunction".to_string(), lambda);
        resources.insert("MySub".to_string(), sub);

        let conns = build_connections(&resources);
        assert_eq!(
            conns.len(),
            1,
            "expected one Notification edge for lambda protocol; connections = {conns:?}"
        );
        assert_eq!(conns[0].source.as_str(), "MyTopic");
        assert_eq!(conns[0].target.as_str(), "MyFunction");
        assert!(matches!(
            conns[0].connection_type,
            ConnectionType::Notification
        ));
    }

    /// `AWS::SNS::Subscription` resource with `Protocol: https` must NOT
    /// produce any edge, even if `Endpoint` carries a reference.
    #[test]
    fn sns_subscription_resource_https_protocol_produces_no_edge() {
        let topic = cfn_resource_typed("MyTopic", "AWS::SNS::Topic", rv_map(vec![]));
        let lambda = cfn_resource_typed("MyFunction", "AWS::Lambda::Function", rv_map(vec![]));
        let embedded_endpoint = ResolvedValue::Interpolated(vec![
            part_lit("arn:aws:lambda:${AWS::Region}:${AWS::AccountId}:function:"),
            part_ref("MyFunction"),
        ]);
        let sub_props = rv_map(vec![
            ("TopicArn", rv_ref("MyTopic")),
            ("Protocol", rv_str("https")),
            ("Endpoint", embedded_endpoint),
        ]);
        let sub = cfn_resource_typed("MySub", "AWS::SNS::Subscription", sub_props);

        let mut resources = BTreeMap::new();
        resources.insert("MyTopic".to_string(), topic);
        resources.insert("MyFunction".to_string(), lambda);
        resources.insert("MySub".to_string(), sub);

        let conns = build_connections(&resources);
        assert!(
            conns.is_empty(),
            "expected no edge for https protocol; connections = {conns:?}"
        );
    }

    // -----------------------------------------------------------------------
    // ESM FunctionName / EventSourceArn interpolated reference tests
    // -----------------------------------------------------------------------

    /// ESM with `FunctionName` given as a `Fn::Sub`-style ARN containing a
    /// reference must produce an EventSource edge to the Lambda.
    ///
    /// Simulates:
    ///   FunctionName: !Sub 'arn:aws:lambda:${AWS::Region}:${AWS::AccountId}:function:${MyFn}'
    #[test]
    fn esm_function_name_interpolated_produces_edge() {
        let queue = cfn_resource_typed("MyQueue", "AWS::SQS::Queue", rv_map(vec![]));
        let lambda = cfn_resource_typed("MyFn", "AWS::Lambda::Function", rv_map(vec![]));

        let interpolated_fn_name = ResolvedValue::Interpolated(vec![
            part_lit("arn:aws:lambda:${AWS::Region}:${AWS::AccountId}:function:"),
            part_ref("MyFn"),
        ]);
        let esm_props = rv_map(vec![
            ("FunctionName", interpolated_fn_name),
            ("EventSourceArn", rv_ref("MyQueue")),
        ]);
        let esm = cfn_resource_typed("MyESM", "AWS::Lambda::EventSourceMapping", esm_props);

        let mut resources = BTreeMap::new();
        resources.insert("MyQueue".to_string(), queue);
        resources.insert("MyFn".to_string(), lambda);
        resources.insert("MyESM".to_string(), esm);

        let conns = build_connections(&resources);
        assert_eq!(
            conns.len(),
            1,
            "expected one EventSource edge; connections = {conns:?}"
        );
        assert_eq!(conns[0].source.as_str(), "MyQueue");
        assert_eq!(conns[0].target.as_str(), "MyFn");
        assert!(matches!(
            conns[0].connection_type,
            ConnectionType::EventSource
        ));
    }

    /// ESM with `EventSourceArn` as an interpolated Fn::Sub ARN must produce
    /// an EventSource edge from the SQS queue to the Lambda.
    #[test]
    fn esm_event_source_arn_interpolated_produces_edge() {
        let queue = cfn_resource_typed("MyQueue", "AWS::SQS::Queue", rv_map(vec![]));
        let lambda = cfn_resource_typed("MyFn", "AWS::Lambda::Function", rv_map(vec![]));

        let interpolated_source_arn = ResolvedValue::Interpolated(vec![
            part_lit("arn:aws:sqs:${AWS::Region}:${AWS::AccountId}:"),
            part_ref("MyQueue"),
        ]);
        let esm_props = rv_map(vec![
            ("FunctionName", rv_ref("MyFn")),
            ("EventSourceArn", interpolated_source_arn),
        ]);
        let esm = cfn_resource_typed("MyESM", "AWS::Lambda::EventSourceMapping", esm_props);

        let mut resources = BTreeMap::new();
        resources.insert("MyQueue".to_string(), queue);
        resources.insert("MyFn".to_string(), lambda);
        resources.insert("MyESM".to_string(), esm);

        let conns = build_connections(&resources);
        assert_eq!(
            conns.len(),
            1,
            "expected one EventSource edge; connections = {conns:?}"
        );
        assert_eq!(conns[0].source.as_str(), "MyQueue");
        assert_eq!(conns[0].target.as_str(), "MyFn");
        assert!(matches!(
            conns[0].connection_type,
            ConnectionType::EventSource
        ));
    }

    /// ESM with a literal external ARN as `EventSourceArn` (no reference) must
    /// still fall back to ARN-pattern detection.
    #[test]
    fn esm_literal_external_sqs_arn_uses_arn_pattern() {
        let lambda = cfn_resource_typed("MyFn", "AWS::Lambda::Function", rv_map(vec![]));
        let esm_props = rv_map(vec![
            ("FunctionName", rv_ref("MyFn")),
            (
                "EventSourceArn",
                rv_str("arn:aws:sqs:us-east-1:123456789012:ExternalQueue"),
            ),
        ]);
        let esm = cfn_resource_typed("MyESM", "AWS::Lambda::EventSourceMapping", esm_props);

        let mut resources = BTreeMap::new();
        resources.insert("MyFn".to_string(), lambda);
        resources.insert("MyESM".to_string(), esm);

        let conns = build_connections(&resources);
        assert_eq!(
            conns.len(),
            1,
            "expected one EventSource edge for external ARN; connections = {conns:?}"
        );
        assert_eq!(conns[0].source.as_str(), "ExternalQueue");
        assert_eq!(conns[0].target.as_str(), "MyFn");
        assert_eq!(conns[0].source_hint.as_deref(), Some("sqs"));
    }

    /// SAM `Type: S3` and `Type: SQS` events on the same function must both
    /// produce edges independently.
    #[test]
    fn sam_s3_and_sqs_events_both_produce_edges() {
        let bucket = cfn_resource_typed("MediaBucket", "AWS::S3::Bucket", rv_map(vec![]));
        let queue = cfn_resource_typed("MyQueue", "AWS::SQS::Queue", rv_map(vec![]));

        // Build a function with two events: S3 and SQS.
        let s3_event_props = rv_map(vec![
            ("Bucket", rv_ref("MediaBucket")),
            ("Events", rv_str("s3:ObjectCreated:*")),
        ]);
        let s3_event = rv_map(vec![("Type", rv_str("S3")), ("Properties", s3_event_props)]);
        let sqs_event_props = rv_map(vec![("Queue", rv_ref("MyQueue"))]);
        let sqs_event = rv_map(vec![
            ("Type", rv_str("SQS")),
            ("Properties", sqs_event_props),
        ]);
        let fn_props = rv_map(vec![(
            "Events",
            rv_map(vec![("PhotoEvent", s3_event), ("QueueEvent", sqs_event)]),
        )]);
        let function = cfn_resource_typed("MyFunction", "AWS::Serverless::Function", fn_props);

        let mut resources = BTreeMap::new();
        resources.insert("MediaBucket".to_string(), bucket);
        resources.insert("MyQueue".to_string(), queue);
        resources.insert("MyFunction".to_string(), function);

        let conns = build_connections(&resources);
        let notif = conns.iter().find(|c| {
            c.source.as_str() == "MediaBucket"
                && c.target.as_str() == "MyFunction"
                && matches!(c.connection_type, ConnectionType::Notification)
        });
        let esm = conns.iter().find(|c| {
            c.source.as_str() == "MyQueue"
                && c.target.as_str() == "MyFunction"
                && matches!(c.connection_type, ConnectionType::EventSource)
        });
        assert!(
            notif.is_some(),
            "expected Notification edge for S3 event; connections = {conns:?}"
        );
        assert!(
            esm.is_some(),
            "expected EventSource edge for SQS event; connections = {conns:?}"
        );
    }
}

#[cfg(test)]
mod determinism_tests {
    use std::collections::HashMap;

    use super::test_helpers::*;
    use super::*;
    use crate::parser::CfnTemplate;
    use yevice_service_api::CfnAdapterRegistry;

    /// Build a minimal resolved `CfnTemplate` whose `resources` map contains
    /// one entry per logical ID in `names`, inserted in the order given.
    fn make_template_with_resources(names: &[&str]) -> ResolvedTemplate {
        let mut resources = BTreeMap::new();
        for &name in names {
            resources.insert(
                name.to_string(),
                cfn_resource_typed(
                    name,
                    "AWS::CloudFormation::WaitConditionHandle",
                    rv_map(vec![]),
                ),
            );
        }
        CfnTemplate {
            parameters: HashMap::new(),
            mappings: HashMap::new(),
            conditions: HashMap::new(),
            resources,
        }
    }

    #[test]
    fn build_architecture_resource_order_is_deterministic() {
        // Use a template with at least 3 resources whose logical IDs are not in
        // alphabetical insert order. Expected: resources are returned in logical_id
        // sort order, identical across two invocations.
        let template = make_template_with_resources(&["ZooLogs", "AlphaFn", "MidTable"]);
        let adapters = CfnAdapterRegistry::new();
        let arch1 = build_architecture("test", "us-east-1", &template, &adapters);
        let arch2 = build_architecture("test", "us-east-1", &template, &adapters);
        let names1: Vec<&str> = arch1
            .resources
            .iter()
            .map(|r| r.logical_id.as_str())
            .collect();
        let names2: Vec<&str> = arch2
            .resources
            .iter()
            .map(|r| r.logical_id.as_str())
            .collect();
        assert_eq!(names1, names2, "resource order must be deterministic");
        assert_eq!(
            names1,
            vec!["AlphaFn", "MidTable", "ZooLogs"],
            "resource order must be sorted by logical_id"
        );
    }

    // -----------------------------------------------------------------------
    // CfnPropertyValue conversion tests
    // -----------------------------------------------------------------------

    #[test]
    fn typed_ref_becomes_resource_ref() {
        let result = resolved_to_cfn_property(&rv_ref("MyBucket"));
        assert!(
            matches!(result, CfnPropertyValue::ResourceRef(ref id) if id == "MyBucket"),
            "expected ResourceRef(MyBucket), got {result:?}"
        );
    }

    #[test]
    fn typed_getatt_becomes_resource_get_att() {
        let result = resolved_to_cfn_property(&rv_getatt("MyFunction", "Arn"));
        assert!(
            matches!(
                result,
                CfnPropertyValue::ResourceGetAtt { ref logical_id, ref attr }
                    if logical_id == "MyFunction" && attr == "Arn"
            ),
            "expected ResourceGetAtt{{ MyFunction, Arn }}, got {result:?}"
        );
    }

    #[test]
    fn concrete_string_stays_concrete() {
        let result = resolved_to_cfn_property(&rv_str("us-east-1"));
        assert!(
            matches!(
                result,
                CfnPropertyValue::Concrete(serde_json::Value::String(ref s)) if s == "us-east-1"
            ),
            "expected Concrete(string), got {result:?}"
        );
    }

    /// An Interpolated value mixing literal text and references renders to a
    /// CFn-native `${...}` string (NOT an internal sentinel format).
    #[test]
    fn interpolated_with_literals_renders_to_cfn_native_string() {
        let value = ResolvedValue::Interpolated(vec![
            part_lit("arn:aws:lambda:${AWS::Region}:fn:"),
            part_ref("MyFn"),
            part_lit(":"),
            part_getatt("Other", "Arn"),
        ]);
        let result = resolved_to_cfn_property(&value);
        match result {
            CfnPropertyValue::Concrete(serde_json::Value::String(s)) => {
                assert_eq!(s, "arn:aws:lambda:${AWS::Region}:fn:${MyFn}:${Other.Arn}");
            }
            other => panic!("expected Concrete(String), got {other:?}"),
        }
    }

    /// A (directly constructed) Interpolated consisting of a single reference
    /// is promoted to the typed variant.
    #[test]
    fn interpolated_single_ref_promotes_to_resource_ref() {
        let value = ResolvedValue::Interpolated(vec![part_ref("MyTable")]);
        let result = resolved_to_cfn_property(&value);
        assert!(
            matches!(result, CfnPropertyValue::ResourceRef(ref id) if id == "MyTable"),
            "expected ResourceRef(MyTable), got {result:?}"
        );
    }

    /// References nested inside containers render to `${...}` strings in the
    /// Concrete JSON.
    #[test]
    fn nested_reference_renders_to_cfn_native_string() {
        let value = rv_seq(vec![rv_ref("MyQueue"), rv_str("literal")]);
        let result = resolved_to_cfn_property(&value);
        match result {
            CfnPropertyValue::Concrete(serde_json::Value::Array(items)) => {
                assert_eq!(
                    items[0],
                    serde_json::Value::String("${MyQueue}".to_string())
                );
                assert_eq!(items[1], serde_json::Value::String("literal".to_string()));
            }
            other => panic!("expected Concrete(Array), got {other:?}"),
        }
    }

    #[test]
    fn resolved_to_cfn_properties_mixed() {
        let props_value = rv_map(vec![
            ("Region", rv_str("us-east-1")),
            ("FunctionArn", rv_getatt("MyFunction", "Arn")),
            ("TableName", rv_ref("MyTable")),
        ]);
        let props = resolved_to_cfn_properties(&props_value);

        assert!(
            matches!(props["Region"], CfnPropertyValue::Concrete(_)),
            "Region should be Concrete"
        );
        assert!(
            matches!(props["FunctionArn"], CfnPropertyValue::ResourceGetAtt { ref logical_id, ref attr } if logical_id == "MyFunction" && attr == "Arn"),
            "FunctionArn should be ResourceGetAtt"
        );
        assert!(
            matches!(props["TableName"], CfnPropertyValue::ResourceRef(ref id) if id == "MyTable"),
            "TableName should be ResourceRef"
        );
    }
}
