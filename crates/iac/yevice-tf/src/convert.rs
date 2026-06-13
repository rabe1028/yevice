//! Terraform config → Architecture conversion using the adapter registry.

use std::collections::{HashMap, HashSet};

use serde_json::Value as JsonValue;
use yevice_core::{
    resource::{
        Architecture, Connection, ConnectionDeduper, ConnectionType, Resource, ResourceShell,
    },
    types::{LogicalId, Region, ResourceType},
};
use yevice_service_api::{RawTfResource, TfAdapterRegistry};

use crate::{
    parser::{TfResource, TfValue},
    resolver::ResolvedConfig,
};

pub fn build_architecture(
    name: &str,
    region: &str,
    resolved: &ResolvedConfig,
    adapters: &TfAdapterRegistry,
) -> Architecture {
    let resources: Vec<Resource> = resolved
        .resources
        .iter()
        .map(|resource| {
            let logical_id = format!("{}_{}", resource.resource_type, resource.name);
            let raw = tf_resource_to_raw(resource, &logical_id);
            let shell = match adapters.lookup(&resource.resource_type) {
                None => ResourceShell::other(&resource.resource_type),
                Some(adapter) => match adapter.convert(&raw) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!(
                            resource_type = %resource.resource_type,
                            error = %e,
                            "adapter failed to convert; treating as unsupported"
                        );
                        ResourceShell::other(&resource.resource_type)
                    }
                },
            };
            Resource {
                logical_id: LogicalId::new(&logical_id),
                resource_type: ResourceType::new(&resource.resource_type),
                shell,
                group: None,
            }
        })
        .collect();

    let connections = build_connections(&resolved.resources, &resources);

    Architecture {
        name: name.to_string(),
        region: Region::new(region),
        resources,
        connections,
    }
}

/// Convert a `TfResource` (with resolved `TfValue` attrs/blocks) into a `RawTfResource`.
///
/// Top-level attrs whose value is `VarRef`, `LocalRef`, or `Unknown` — i.e. references
/// that could not be resolved — are dropped from the output and logged as `tracing::warn!`.
/// Adapters that fall back to a hardcoded default for missing attrs will therefore use that
/// default, so the warning names both the resource and the attr so operators can supply the
/// variable via tfvars for accurate pricing.
fn tf_resource_to_raw(resource: &TfResource, logical_id: &str) -> RawTfResource {
    let mut attrs: HashMap<String, JsonValue> = HashMap::new();
    for (k, v) in &resource.attrs {
        match tf_value_to_json(v) {
            Some(jv) => {
                attrs.insert(k.clone(), jv);
            }
            None => {
                tracing::warn!(
                    resource = %logical_id,
                    attr = %k,
                    "unresolved Terraform reference dropped; adapter default will be used — \
                     supply the variable via tfvars for accurate pricing"
                );
            }
        }
    }

    let blocks: HashMap<String, Vec<HashMap<String, JsonValue>>> = resource
        .blocks
        .iter()
        .map(|(block_name, block_list)| {
            let converted: Vec<HashMap<String, JsonValue>> = block_list
                .iter()
                .map(|block_attrs| {
                    block_attrs
                        .iter()
                        .filter_map(|(k, v)| tf_value_to_json(v).map(|jv| (k.clone(), jv)))
                        .collect()
                })
                .collect();
            (block_name.clone(), converted)
        })
        .collect();

    let mut raw = RawTfResource::new(logical_id, &resource.resource_type);
    raw.attrs = attrs;
    raw.blocks = blocks;
    raw
}

/// Convert a concrete `TfValue` to a `serde_json::Value`.
///
/// Returns `None` for unresolved references (`VarRef`, `LocalRef`, `ResourceRef`,
/// `Unknown`). `ResourceRef` is not a scalar value; it is consumed separately by
/// [`build_connections`].
///
/// For `Object` and `Array`, nested references are silently dropped so that the
/// spec JSON is not polluted by un-serialisable reference values. Keys/elements
/// whose value cannot be converted are omitted.
fn tf_value_to_json(value: &TfValue) -> Option<JsonValue> {
    match value {
        TfValue::String(s) => Some(JsonValue::String(s.clone())),
        TfValue::Number(n) => serde_json::Number::from_f64(*n).map(JsonValue::Number),
        TfValue::Bool(b) => Some(JsonValue::Bool(*b)),
        TfValue::Object(map) => {
            let obj: serde_json::Map<String, JsonValue> = map
                .iter()
                .filter_map(|(k, v)| tf_value_to_json(v).map(|jv| (k.clone(), jv)))
                .collect();
            Some(JsonValue::Object(obj))
        }
        TfValue::Array(items) => {
            let arr: Vec<JsonValue> = items.iter().filter_map(tf_value_to_json).collect();
            Some(JsonValue::Array(arr))
        }
        TfValue::VarRef(_)
        | TfValue::LocalRef(_)
        | TfValue::ResourceRef { .. }
        | TfValue::Unknown => {
            tracing::debug!(value = ?value, "unresolved TfValue reference dropped during conversion");
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Connection extraction from ResourceRef attrs
// ---------------------------------------------------------------------------

/// Storage resource types: Lambda → these produce DataFlow edges.
const STORAGE_RESOURCE_TYPES: &[&str] = &[
    "aws_dynamodb_table",
    "aws_s3_bucket",
    "aws_sqs_queue",
    "aws_sns_topic",
];

/// Compute resource types: Lambda → these produce Invocation edges.
const COMPUTE_RESOURCE_TYPES: &[&str] = &["aws_sfn_state_machine", "aws_lambda_function"];

/// Notification resource types: these → Lambda/SQS produce Notification edges.
const NOTIFICATION_SOURCE_TYPES: &[&str] = &["aws_s3_bucket_notification"];

/// Terraform top-level attributes on `aws_lambda_function` that describe
/// deployment artefacts, IAM, or encryption — not runtime data flow.
/// References in these attributes must not produce DataFlow/Invocation edges.
const NON_RUNTIME_ATTRS: &[&str] = &[
    // Deployment source artefacts
    "s3_bucket",
    "s3_key",
    "s3_object_version",
    "filename",
    "source_code_hash",
    "image_uri",
    "layers",
    // Permissions / encryption
    "role",
    "kms_key_arn",
    // Terraform meta-arguments
    "depends_on",
    "count",
    "for_each",
    "provider",
];

/// Terraform blocks that describe deployment/configuration, not runtime data
/// flow; references inside them must not become runtime connection edges.
const NON_RUNTIME_BLOCKS: &[&str] = &[
    "dead_letter_config",
    "vpc_config",
    "tracing_config",
    "file_system_config",
    "image_config",
    "ephemeral_storage",
    "logging_config",
    "snap_start",
    "timeouts",
    "lifecycle",
];

/// Logical-id format used throughout this crate: `<resource_type>_<name>`.
fn logical_id_for(resource_type: &str, name: &str) -> String {
    format!("{resource_type}_{name}")
}

/// Build the set of logical IDs present in the architecture so we can guard
/// against dangling edges.
fn node_set(resources: &[Resource]) -> HashSet<String> {
    resources
        .iter()
        .map(|r| r.logical_id.as_str().to_string())
        .collect()
}

/// Push a connection into `dedupe` only if:
/// 1. Both endpoints exist in `nodes`.
/// 2. The `(source, target, type)` triple has not been seen yet.
///
/// Delegates to [`ConnectionDeduper`] (in `yevice-core`) so CFn and Terraform
/// share the same dedupe semantics.
fn push_unique(
    dedupe: &mut ConnectionDeduper,
    nodes: &HashSet<String>,
    source: &str,
    target: &str,
    conn_type: ConnectionType,
) {
    let conn = Connection {
        source: LogicalId::new(source),
        target: LogicalId::new(target),
        connection_type: conn_type,
        batch_size: None,
        parallelization_factor: None,
        factor: None,
        source_hint: None,
    };
    dedupe.try_push(conn, |id| nodes.contains(id), |id| nodes.contains(id));
}

/// Returns `true` when a notification block's `events` attribute contains at
/// least one value that begins with `"s3:ObjectCreated"`.
///
/// This is used to gate `aws_s3_bucket_notification` block refs so that only
/// object-creation notifications are wired to the cost model (the source-rate
/// variable is derived from `put_requests`).  Delete / restore / tagging events
/// must not produce a put-bound edge.
///
/// Blocks without an `events` key, or whose `events` array contains only
/// non-ObjectCreated strings, return `false`.
fn block_has_object_created_event(
    block_attrs: &std::collections::HashMap<String, TfValue>,
) -> bool {
    let Some(events_val) = block_attrs.get("events") else {
        return false;
    };
    let TfValue::Array(items) = events_val else {
        return false;
    };
    items.iter().any(|v| {
        v.as_str()
            .is_some_and(|s| s.starts_with("s3:ObjectCreated"))
    })
}

/// Recursively collect every `ResourceRef` reachable from `value`.
///
/// Each found ref is appended to `out` as `(resource_type, name, attr)`.
fn collect_resource_refs<'a>(value: &'a TfValue, out: &mut Vec<(&'a str, &'a str, &'a str)>) {
    match value {
        TfValue::ResourceRef {
            resource_type,
            name,
            attr,
        } => out.push((resource_type.as_str(), name.as_str(), attr.as_str())),
        TfValue::Object(map) => {
            for v in map.values() {
                collect_resource_refs(v, out);
            }
        }
        TfValue::Array(items) => {
            for v in items {
                collect_resource_refs(v, out);
            }
        }
        TfValue::String(_)
        | TfValue::Number(_)
        | TfValue::Bool(_)
        | TfValue::VarRef(_)
        | TfValue::LocalRef(_)
        | TfValue::Unknown => {}
    }
}

/// Walk all resolved `TfResource`s and produce `Connection` edges from every
/// `TfValue::ResourceRef` found in their attrs or block attrs (including nested
/// Object/Array values).
fn build_connections(tf_resources: &[TfResource], resources: &[Resource]) -> Vec<Connection> {
    let nodes = node_set(resources);
    let mut dedupe = ConnectionDeduper::new();

    for src_resource in tf_resources {
        let src_type = src_resource.resource_type.as_str();
        let src_lid = logical_id_for(src_type, &src_resource.name);

        // ---------------------------------------------------------------
        // Special case: aws_lambda_event_source_mapping
        // This resource represents an ESM and is not itself a node;
        // we create one EventSource edge from the event-source to the lambda.
        // ---------------------------------------------------------------
        if src_type == "aws_lambda_event_source_mapping" {
            if let (
                Some(TfValue::ResourceRef {
                    resource_type: esrc_type,
                    name: esrc_name,
                    ..
                }),
                Some(TfValue::ResourceRef {
                    resource_type: fn_type,
                    name: fn_name,
                    ..
                }),
            ) = (
                src_resource.attrs.get("event_source_arn"),
                src_resource.attrs.get("function_name"),
            ) {
                let esrc_lid = logical_id_for(esrc_type, esrc_name);
                let fn_lid = logical_id_for(fn_type, fn_name);
                push_unique(
                    &mut dedupe,
                    &nodes,
                    &esrc_lid,
                    &fn_lid,
                    ConnectionType::EventSource,
                );
            }
            // ESM itself is not a node — no further generic edges needed.
            continue;
        }

        // ---------------------------------------------------------------
        // Special case: aws_sns_topic_subscription
        // This resource is glue (topic → subscriber); it is not itself a
        // node.  Only emit a Notification edge when `protocol` is "lambda"
        // or "sqs" — the only protocols that represent a runtime invocation
        // modelled by the cost graph.  Non-runtime protocols (https, email,
        // sms, http, application, …) are skipped.
        // ---------------------------------------------------------------
        if src_type == "aws_sns_topic_subscription" {
            let protocol = src_resource
                .attrs
                .get("protocol")
                .and_then(|v| {
                    if let TfValue::String(s) = v {
                        Some(s.as_str())
                    } else {
                        None
                    }
                })
                .unwrap_or("");
            if matches!(protocol, "lambda" | "sqs")
                && let (
                    Some(TfValue::ResourceRef {
                        resource_type: topic_type,
                        name: topic_name,
                        ..
                    }),
                    Some(TfValue::ResourceRef {
                        resource_type: ep_type,
                        name: ep_name,
                        ..
                    }),
                ) = (
                    src_resource.attrs.get("topic_arn"),
                    src_resource.attrs.get("endpoint"),
                )
            {
                let topic_lid = logical_id_for(topic_type, topic_name);
                let ep_lid = logical_id_for(ep_type, ep_name);
                push_unique(
                    &mut dedupe,
                    &nodes,
                    &topic_lid,
                    &ep_lid,
                    ConnectionType::Notification,
                );
            }
            // Subscription itself is not a node — no further generic edges.
            continue;
        }

        // ---------------------------------------------------------------
        // aws_s3_bucket_notification: Notification edges
        //
        // The `bucket` attribute holds a ResourceRef to the S3 bucket that
        // owns this notification configuration.  All other refs (lambda ARNs,
        // queue ARNs, topic ARNs, …) are the notification targets.
        // Correct direction: bucket → target.
        // ---------------------------------------------------------------
        if NOTIFICATION_SOURCE_TYPES.contains(&src_type) {
            // Resolve the bucket source.
            let bucket_lid = match src_resource.attrs.get("bucket") {
                Some(TfValue::ResourceRef {
                    resource_type: bucket_type,
                    name: bucket_name,
                    ..
                }) => logical_id_for(bucket_type, bucket_name),
                // `bucket` is missing or not a ResourceRef — skip notification edges.
                _ => continue,
            };

            // Collect ResourceRefs from notification blocks (e.g. lambda_function,
            // queue, topic) — but only from blocks whose `events` array contains
            // at least one `s3:ObjectCreated` event.  Non-create events (delete,
            // restore, etc.) are driven by different source-rate variables and
            // must not produce a put_requests-bound Notification edge.
            let mut target_refs: Vec<(&str, &str, &str)> = Vec::new();
            for block_list in src_resource.blocks.values() {
                for block_attrs in block_list {
                    if !block_has_object_created_event(block_attrs) {
                        continue;
                    }
                    for ref_val in block_attrs.values() {
                        collect_resource_refs(ref_val, &mut target_refs);
                    }
                }
            }
            // Also collect any non-`bucket` attr refs.
            for (attr_key, ref_val) in &src_resource.attrs {
                if attr_key == "bucket" {
                    continue;
                }
                collect_resource_refs(ref_val, &mut target_refs);
            }

            for (tgt_type, tgt_name, _attr) in target_refs {
                let tgt_lid = logical_id_for(tgt_type, tgt_name);
                push_unique(
                    &mut dedupe,
                    &nodes,
                    &bucket_lid,
                    &tgt_lid,
                    ConnectionType::Notification,
                );
            }
            continue;
        }

        // ---------------------------------------------------------------
        // Generic: walk every ResourceRef reachable from attrs and block
        // attrs, including those nested inside Object/Array values.
        // Connection type depends on source / target resource types.
        // ---------------------------------------------------------------
        let mut refs: Vec<(&str, &str, &str)> = Vec::new();

        for (attr_key, attr_val) in &src_resource.attrs {
            if NON_RUNTIME_ATTRS.contains(&attr_key.as_str()) {
                continue;
            }
            collect_resource_refs(attr_val, &mut refs);
        }

        for (block_name, block_list) in &src_resource.blocks {
            if NON_RUNTIME_BLOCKS.contains(&block_name.as_str()) {
                continue;
            }
            for block_attrs in block_list {
                for attr_val in block_attrs.values() {
                    collect_resource_refs(attr_val, &mut refs);
                }
            }
        }

        for (tgt_type, tgt_name, _attr) in refs {
            let tgt_lid = logical_id_for(tgt_type, tgt_name);
            if let Some(conn_type) = classify_connection(src_type, tgt_type) {
                push_unique(&mut dedupe, &nodes, &src_lid, &tgt_lid, conn_type);
            }
        }
    }

    dedupe.into_connections()
}

/// Determine `ConnectionType` for a generic (non-ESM, non-notification) edge.
///
/// Returns `Some` only for recognised source/target pairs:
/// - `aws_lambda_function` → STORAGE_RESOURCE_TYPES  ⇒  `DataFlow`
/// - `aws_lambda_function` → COMPUTE_RESOURCE_TYPES  ⇒  `Invocation`
///
/// Unrecognised pairs (e.g. lambda → IAM role, lambda → CloudWatch log group)
/// return `None` so that no spurious edges are created.
fn classify_connection(src_type: &str, tgt_type: &str) -> Option<ConnectionType> {
    if src_type == "aws_lambda_function" {
        if STORAGE_RESOURCE_TYPES.contains(&tgt_type) {
            return Some(ConnectionType::DataFlow);
        }
        if COMPUTE_RESOURCE_TYPES.contains(&tgt_type) {
            return Some(ConnectionType::Invocation);
        }
    }
    None
}

#[cfg(test)]
mod s3_notification_gating_tests {
    use super::*;

    /// Build a minimal `Resource` node for the node-set guard.
    fn node(resource_type: &str, name: &str) -> Resource {
        let lid = logical_id_for(resource_type, name);
        Resource {
            logical_id: LogicalId::new(&lid),
            resource_type: ResourceType::new(resource_type),
            shell: ResourceShell::other(resource_type),
            group: None,
        }
    }

    /// Build a minimal `TfResource` for `aws_s3_bucket_notification` with one
    /// `lambda_function` block whose `events` attribute is `events_list`.
    fn s3_notif_resource(
        bucket_type: &str,
        bucket_name: &str,
        lambda_type: &str,
        lambda_name: &str,
        events_list: Vec<&str>,
    ) -> TfResource {
        let mut attrs = HashMap::new();
        attrs.insert(
            "bucket".to_string(),
            TfValue::ResourceRef {
                resource_type: bucket_type.to_string(),
                name: bucket_name.to_string(),
                attr: "id".to_string(),
            },
        );

        let events_tf: Vec<TfValue> = events_list
            .into_iter()
            .map(|e| TfValue::String(e.to_string()))
            .collect();
        let mut block_attrs = HashMap::new();
        block_attrs.insert(
            "lambda_function_arn".to_string(),
            TfValue::ResourceRef {
                resource_type: lambda_type.to_string(),
                name: lambda_name.to_string(),
                attr: "arn".to_string(),
            },
        );
        block_attrs.insert("events".to_string(), TfValue::Array(events_tf));

        let mut blocks = HashMap::new();
        blocks.insert("lambda_function".to_string(), vec![block_attrs]);

        TfResource {
            resource_type: "aws_s3_bucket_notification".to_string(),
            name: "notif".to_string(),
            attrs,
            blocks,
        }
    }

    /// `events = ["s3:ObjectCreated:*"]` must produce a Notification edge.
    #[test]
    fn object_created_event_produces_notification_edge() {
        let bucket = node("aws_s3_bucket", "my_bucket");
        let lambda = node("aws_lambda_function", "my_lambda");
        let notif = s3_notif_resource(
            "aws_s3_bucket",
            "my_bucket",
            "aws_lambda_function",
            "my_lambda",
            vec!["s3:ObjectCreated:*"],
        );

        let tf_resources = vec![notif];
        let resources = vec![bucket, lambda];
        let conns = build_connections(&tf_resources, &resources);

        let edge = conns.iter().find(|c| {
            c.connection_type == ConnectionType::Notification
                && c.source.as_str() == "aws_s3_bucket_my_bucket"
                && c.target.as_str() == "aws_lambda_function_my_lambda"
        });
        assert!(
            edge.is_some(),
            "expected Notification edge for s3:ObjectCreated:*; connections = {conns:?}",
        );
    }

    /// `events = ["s3:ObjectRemoved:*"]` must NOT produce a Notification edge.
    #[test]
    fn object_removed_event_skipped() {
        let bucket = node("aws_s3_bucket", "my_bucket");
        let lambda = node("aws_lambda_function", "my_lambda");
        let notif = s3_notif_resource(
            "aws_s3_bucket",
            "my_bucket",
            "aws_lambda_function",
            "my_lambda",
            vec!["s3:ObjectRemoved:*"],
        );

        let tf_resources = vec![notif];
        let resources = vec![bucket, lambda];
        let conns = build_connections(&tf_resources, &resources);

        assert!(
            conns.is_empty(),
            "expected no Notification edge for s3:ObjectRemoved:*; connections = {conns:?}",
        );
    }

    /// `events = ["s3:ObjectRestore:*"]` must NOT produce a Notification edge.
    #[test]
    fn object_restore_event_skipped() {
        let bucket = node("aws_s3_bucket", "my_bucket");
        let lambda = node("aws_lambda_function", "my_lambda");
        let notif = s3_notif_resource(
            "aws_s3_bucket",
            "my_bucket",
            "aws_lambda_function",
            "my_lambda",
            vec!["s3:ObjectRestore:*"],
        );

        let tf_resources = vec![notif];
        let resources = vec![bucket, lambda];
        let conns = build_connections(&tf_resources, &resources);

        assert!(
            conns.is_empty(),
            "expected no Notification edge for s3:ObjectRestore:*; connections = {conns:?}",
        );
    }

    /// Mixed events: one ObjectCreated + one ObjectRemoved block → only the
    /// ObjectCreated block contributes a Notification edge.
    #[test]
    fn mixed_events_only_object_created_block_produces_edge() {
        let bucket = node("aws_s3_bucket", "my_bucket");
        let lambda_a = node("aws_lambda_function", "fn_a");
        let lambda_b = node("aws_lambda_function", "fn_b");

        // Build the notification resource manually with two lambda_function blocks.
        let mut attrs = HashMap::new();
        attrs.insert(
            "bucket".to_string(),
            TfValue::ResourceRef {
                resource_type: "aws_s3_bucket".to_string(),
                name: "my_bucket".to_string(),
                attr: "id".to_string(),
            },
        );

        let mut block_created = HashMap::new();
        block_created.insert(
            "lambda_function_arn".to_string(),
            TfValue::ResourceRef {
                resource_type: "aws_lambda_function".to_string(),
                name: "fn_a".to_string(),
                attr: "arn".to_string(),
            },
        );
        block_created.insert(
            "events".to_string(),
            TfValue::Array(vec![TfValue::String("s3:ObjectCreated:*".to_string())]),
        );

        let mut block_removed = HashMap::new();
        block_removed.insert(
            "lambda_function_arn".to_string(),
            TfValue::ResourceRef {
                resource_type: "aws_lambda_function".to_string(),
                name: "fn_b".to_string(),
                attr: "arn".to_string(),
            },
        );
        block_removed.insert(
            "events".to_string(),
            TfValue::Array(vec![TfValue::String("s3:ObjectRemoved:*".to_string())]),
        );

        let mut blocks = HashMap::new();
        blocks.insert(
            "lambda_function".to_string(),
            vec![block_created, block_removed],
        );

        let notif = TfResource {
            resource_type: "aws_s3_bucket_notification".to_string(),
            name: "notif".to_string(),
            attrs,
            blocks,
        };

        let tf_resources = vec![notif];
        let resources = vec![bucket, lambda_a, lambda_b];
        let conns = build_connections(&tf_resources, &resources);

        assert_eq!(
            conns.len(),
            1,
            "expected exactly one Notification edge; connections = {conns:?}",
        );
        assert_eq!(conns[0].source.as_str(), "aws_s3_bucket_my_bucket");
        assert_eq!(conns[0].target.as_str(), "aws_lambda_function_fn_a");
        assert_eq!(conns[0].connection_type, ConnectionType::Notification);
    }
}

#[cfg(test)]
mod sns_subscription_tests {
    use super::*;

    fn node(resource_type: &str, name: &str) -> Resource {
        let lid = logical_id_for(resource_type, name);
        Resource {
            logical_id: LogicalId::new(&lid),
            resource_type: ResourceType::new(resource_type),
            shell: ResourceShell::other(resource_type),
            group: None,
        }
    }

    fn subscription(
        sub_name: &str,
        topic_type: &str,
        topic_name: &str,
        endpoint: TfValue,
    ) -> TfResource {
        let mut attrs = HashMap::new();
        attrs.insert(
            "topic_arn".to_string(),
            TfValue::ResourceRef {
                resource_type: topic_type.to_string(),
                name: topic_name.to_string(),
                attr: "arn".to_string(),
            },
        );
        attrs.insert("endpoint".to_string(), endpoint);
        attrs.insert(
            "protocol".to_string(),
            TfValue::String("lambda".to_string()),
        );
        TfResource {
            resource_type: "aws_sns_topic_subscription".to_string(),
            name: sub_name.to_string(),
            attrs,
            blocks: HashMap::new(),
        }
    }

    /// Two subscriptions with ResourceRef endpoints must each produce a
    /// Notification edge from the SNS topic to the respective subscriber.
    #[test]
    fn two_subscriptions_produce_two_notification_edges() {
        let topic = node("aws_sns_topic", "my_topic");
        let lambda = node("aws_lambda_function", "fn_a");
        let queue = node("aws_sqs_queue", "q_a");

        let sub_lambda = subscription(
            "sub_lambda",
            "aws_sns_topic",
            "my_topic",
            TfValue::ResourceRef {
                resource_type: "aws_lambda_function".to_string(),
                name: "fn_a".to_string(),
                attr: "arn".to_string(),
            },
        );
        let sub_sqs = subscription(
            "sub_sqs",
            "aws_sns_topic",
            "my_topic",
            TfValue::ResourceRef {
                resource_type: "aws_sqs_queue".to_string(),
                name: "q_a".to_string(),
                attr: "arn".to_string(),
            },
        );

        let tf_resources = vec![sub_lambda, sub_sqs];
        let resources = vec![topic, lambda, queue];
        let conns = build_connections(&tf_resources, &resources);

        assert_eq!(
            conns.len(),
            2,
            "expected 2 Notification edges; connections = {conns:?}",
        );
        let has_lambda_edge = conns.iter().any(|c| {
            c.connection_type == ConnectionType::Notification
                && c.source.as_str() == "aws_sns_topic_my_topic"
                && c.target.as_str() == "aws_lambda_function_fn_a"
        });
        let has_sqs_edge = conns.iter().any(|c| {
            c.connection_type == ConnectionType::Notification
                && c.source.as_str() == "aws_sns_topic_my_topic"
                && c.target.as_str() == "aws_sqs_queue_q_a"
        });
        assert!(
            has_lambda_edge,
            "missing topic→lambda Notification edge; connections = {conns:?}",
        );
        assert!(
            has_sqs_edge,
            "missing topic→sqs Notification edge; connections = {conns:?}",
        );
    }

    /// A subscription whose `endpoint` is a literal string (e.g. http/email)
    /// must NOT produce any edge.
    #[test]
    fn literal_endpoint_produces_no_edge() {
        let topic = node("aws_sns_topic", "my_topic");

        let sub_http = subscription(
            "sub_http",
            "aws_sns_topic",
            "my_topic",
            TfValue::String("https://example.com/hook".to_string()),
        );

        let tf_resources = vec![sub_http];
        let resources = vec![topic];
        let conns = build_connections(&tf_resources, &resources);

        assert!(
            conns.is_empty(),
            "expected no edge for literal endpoint; connections = {conns:?}",
        );
    }

    /// Helper: build a subscription with an explicit protocol string.
    fn subscription_with_protocol(
        sub_name: &str,
        topic_type: &str,
        topic_name: &str,
        protocol: &str,
        endpoint: TfValue,
    ) -> TfResource {
        let mut attrs = HashMap::new();
        attrs.insert(
            "topic_arn".to_string(),
            TfValue::ResourceRef {
                resource_type: topic_type.to_string(),
                name: topic_name.to_string(),
                attr: "arn".to_string(),
            },
        );
        attrs.insert("endpoint".to_string(), endpoint);
        attrs.insert(
            "protocol".to_string(),
            TfValue::String(protocol.to_string()),
        );
        TfResource {
            resource_type: "aws_sns_topic_subscription".to_string(),
            name: sub_name.to_string(),
            attrs,
            blocks: HashMap::new(),
        }
    }

    /// protocol=https with ResourceRef endpoint must NOT produce any edge.
    #[test]
    fn https_protocol_with_resource_ref_endpoint_produces_no_edge() {
        let topic = node("aws_sns_topic", "my_topic");
        let lambda = node("aws_lambda_function", "fn_a");

        let sub_https = subscription_with_protocol(
            "sub_https",
            "aws_sns_topic",
            "my_topic",
            "https",
            TfValue::ResourceRef {
                resource_type: "aws_lambda_function".to_string(),
                name: "fn_a".to_string(),
                attr: "invoke_arn".to_string(),
            },
        );

        let tf_resources = vec![sub_https];
        let resources = vec![topic, lambda];
        let conns = build_connections(&tf_resources, &resources);

        assert!(
            conns.is_empty(),
            "expected no edge for https protocol even with ResourceRef endpoint; connections = {conns:?}",
        );
    }

    /// protocol=sqs with ResourceRef endpoint must produce a Notification edge.
    #[test]
    fn sqs_protocol_produces_notification_edge() {
        let topic = node("aws_sns_topic", "my_topic");
        let queue = node("aws_sqs_queue", "my_queue");

        let sub_sqs = subscription_with_protocol(
            "sub_sqs",
            "aws_sns_topic",
            "my_topic",
            "sqs",
            TfValue::ResourceRef {
                resource_type: "aws_sqs_queue".to_string(),
                name: "my_queue".to_string(),
                attr: "arn".to_string(),
            },
        );

        let tf_resources = vec![sub_sqs];
        let resources = vec![topic, queue];
        let conns = build_connections(&tf_resources, &resources);

        assert_eq!(
            conns.len(),
            1,
            "expected one Notification edge for sqs protocol; connections = {conns:?}",
        );
        assert_eq!(conns[0].source.as_str(), "aws_sns_topic_my_topic");
        assert_eq!(conns[0].target.as_str(), "aws_sqs_queue_my_queue");
        assert_eq!(conns[0].connection_type, ConnectionType::Notification);
    }
}

#[cfg(test)]
mod tf_resource_to_raw_tests {
    use super::*;

    /// A resource with one concrete attr and one unresolved VarRef attr must
    /// produce a `RawTfResource` where only the concrete attr is present.
    /// The unresolved key must be absent from `raw.attrs`.
    #[test]
    fn unresolved_varref_attr_is_absent_from_raw() {
        let mut attrs = HashMap::new();
        attrs.insert(
            "instance_type".to_string(),
            TfValue::VarRef("instance_type_var".to_string()),
        );
        attrs.insert("ami".to_string(), TfValue::String("ami-12345".to_string()));

        let resource = TfResource {
            resource_type: "aws_instance".to_string(),
            name: "web".to_string(),
            attrs,
            blocks: HashMap::new(),
        };

        let raw = tf_resource_to_raw(&resource, "aws_instance_web");

        // The concrete attr must be present.
        assert_eq!(
            raw.get_str("ami"),
            Some("ami-12345"),
            "concrete attr must be present in raw"
        );
        // The unresolved VarRef attr must be absent.
        assert!(
            !raw.attrs.contains_key("instance_type"),
            "unresolved VarRef attr must be absent from raw; raw.attrs = {:?}",
            raw.attrs
        );
    }

    /// Same check for `LocalRef`.
    #[test]
    fn unresolved_localref_attr_is_absent_from_raw() {
        let mut attrs = HashMap::new();
        attrs.insert(
            "node_type".to_string(),
            TfValue::LocalRef("node_type_local".to_string()),
        );

        let resource = TfResource {
            resource_type: "aws_elasticache_cluster".to_string(),
            name: "cache".to_string(),
            attrs,
            blocks: HashMap::new(),
        };

        let raw = tf_resource_to_raw(&resource, "aws_elasticache_cluster_cache");

        assert!(
            !raw.attrs.contains_key("node_type"),
            "unresolved LocalRef attr must be absent from raw; raw.attrs = {:?}",
            raw.attrs
        );
    }

    /// Same check for `Unknown`.
    #[test]
    fn unknown_attr_is_absent_from_raw() {
        let mut attrs = HashMap::new();
        attrs.insert("memory_size".to_string(), TfValue::Unknown);
        attrs.insert(
            "runtime".to_string(),
            TfValue::String("python3.12".to_string()),
        );

        let resource = TfResource {
            resource_type: "aws_lambda_function".to_string(),
            name: "fn".to_string(),
            attrs,
            blocks: HashMap::new(),
        };

        let raw = tf_resource_to_raw(&resource, "aws_lambda_function_fn");

        assert!(
            !raw.attrs.contains_key("memory_size"),
            "Unknown attr must be absent from raw; raw.attrs = {:?}",
            raw.attrs
        );
        assert_eq!(raw.get_str("runtime"), Some("python3.12"));
    }
}

#[cfg(test)]
mod tf_value_to_json_tests {
    use std::collections::BTreeMap;

    use super::*;

    /// An Object whose values are all concrete scalars must convert to a
    /// JSON object with the same keys and values.
    #[test]
    fn object_of_scalars_converts_to_json_object() {
        let mut map: BTreeMap<String, Box<TfValue>> = BTreeMap::new();
        map.insert(
            "KEY".to_string(),
            Box::new(TfValue::String("value".to_string())),
        );
        map.insert("NUM".to_string(), Box::new(TfValue::Number(42.0)));
        let obj = TfValue::Object(map);

        let json = tf_value_to_json(&obj).expect("Object of scalars must convert");
        assert!(json.is_object(), "expected JSON object; got {json:?}");
        assert_eq!(json["KEY"], serde_json::json!("value"));
        assert_eq!(json["NUM"], serde_json::json!(42.0));
    }

    /// An Object that contains an unresolved VarRef must silently drop that
    /// key and still return a JSON object for the remaining scalars.
    #[test]
    fn object_with_var_ref_drops_unresolved_key() {
        let mut map: BTreeMap<String, Box<TfValue>> = BTreeMap::new();
        map.insert(
            "RESOLVED".to_string(),
            Box::new(TfValue::String("ok".to_string())),
        );
        map.insert(
            "UNRESOLVED".to_string(),
            Box::new(TfValue::VarRef("some_var".to_string())),
        );
        let obj = TfValue::Object(map);

        let json = tf_value_to_json(&obj).expect("Object must produce Some even with VarRef");
        assert!(json.is_object());
        assert_eq!(json["RESOLVED"], serde_json::json!("ok"));
        assert!(
            json.get("UNRESOLVED").is_none(),
            "VarRef key must be dropped"
        );
    }
}

#[cfg(test)]
mod non_runtime_attrs_tests {
    use super::*;

    fn node(resource_type: &str, name: &str) -> Resource {
        let lid = logical_id_for(resource_type, name);
        Resource {
            logical_id: LogicalId::new(&lid),
            resource_type: ResourceType::new(resource_type),
            shell: ResourceShell::other(resource_type),
            group: None,
        }
    }

    /// Lambda with `s3_bucket = aws_s3_bucket.code.id` and
    /// `depends_on = [aws_sqs_queue.q]` must NOT produce any DataFlow edge —
    /// these attrs are deployment-only / meta.
    #[test]
    fn deployment_attrs_do_not_produce_dataflow_edges() {
        let lambda = node("aws_lambda_function", "fn");
        let code_bucket = node("aws_s3_bucket", "code");
        let dep_queue = node("aws_sqs_queue", "q");

        let mut attrs = HashMap::new();
        attrs.insert(
            "s3_bucket".to_string(),
            TfValue::ResourceRef {
                resource_type: "aws_s3_bucket".to_string(),
                name: "code".to_string(),
                attr: "id".to_string(),
            },
        );
        attrs.insert(
            "depends_on".to_string(),
            TfValue::ResourceRef {
                resource_type: "aws_sqs_queue".to_string(),
                name: "q".to_string(),
                attr: "arn".to_string(),
            },
        );

        let tf = TfResource {
            resource_type: "aws_lambda_function".to_string(),
            name: "fn".to_string(),
            attrs,
            blocks: HashMap::new(),
        };

        let tf_resources = vec![tf];
        let resources = vec![lambda, code_bucket, dep_queue];
        let conns = build_connections(&tf_resources, &resources);

        assert!(
            conns.is_empty(),
            "deployment attrs must not produce edges; got: {conns:?}"
        );
    }

    /// Lambda with `role = aws_iam_role.exec.arn` (non-runtime IAM attr) must
    /// not produce any edge, even if the IAM role were in the node set.
    #[test]
    fn role_attr_does_not_produce_edge() {
        let lambda = node("aws_lambda_function", "fn");
        // Create a fake "iam role" node to verify no spurious edge is emitted
        // (in practice IAM roles are not in the node set, but we test the attr
        // denylist regardless of whether the target node exists).
        let fake_role = node("aws_iam_role", "exec");

        let mut attrs = HashMap::new();
        attrs.insert(
            "role".to_string(),
            TfValue::ResourceRef {
                resource_type: "aws_iam_role".to_string(),
                name: "exec".to_string(),
                attr: "arn".to_string(),
            },
        );

        let tf = TfResource {
            resource_type: "aws_lambda_function".to_string(),
            name: "fn".to_string(),
            attrs,
            blocks: HashMap::new(),
        };

        let tf_resources = vec![tf];
        let resources = vec![lambda, fake_role];
        let conns = build_connections(&tf_resources, &resources);

        assert!(
            conns.is_empty(),
            "role attr must not produce an edge; got: {conns:?}"
        );
    }

    /// Lambda with `environment` block that references another resource via a
    /// variable (not a ResourceRef) is fine; and a genuine runtime DataFlow edge
    /// via `STORAGE_RESOURCE_TYPES` must still be produced when the attr is not
    /// in `NON_RUNTIME_ATTRS`.
    #[test]
    fn runtime_attr_ref_still_produces_dataflow_edge() {
        let lambda = node("aws_lambda_function", "fn");
        let table = node("aws_dynamodb_table", "tbl");

        // Use a non-denylisted attr (e.g. a custom "table_name" attr that holds
        // a ResourceRef — simulates how some modules pass table ARN).
        let mut attrs = HashMap::new();
        attrs.insert(
            "table_name".to_string(),
            TfValue::ResourceRef {
                resource_type: "aws_dynamodb_table".to_string(),
                name: "tbl".to_string(),
                attr: "name".to_string(),
            },
        );

        let tf = TfResource {
            resource_type: "aws_lambda_function".to_string(),
            name: "fn".to_string(),
            attrs,
            blocks: HashMap::new(),
        };

        let tf_resources = vec![tf];
        let resources = vec![lambda, table];
        let conns = build_connections(&tf_resources, &resources);

        assert_eq!(
            conns.len(),
            1,
            "runtime attr ref must produce DataFlow edge; got: {conns:?}"
        );
        assert_eq!(conns[0].connection_type, ConnectionType::DataFlow);
        assert_eq!(conns[0].source.as_str(), "aws_lambda_function_fn");
        assert_eq!(conns[0].target.as_str(), "aws_dynamodb_table_tbl");
    }
}
