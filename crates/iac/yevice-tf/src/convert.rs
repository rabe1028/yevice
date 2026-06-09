//! Terraform config → Architecture conversion using the adapter registry.

use std::collections::{HashMap, HashSet};

use serde_json::Value as JsonValue;
use yevice_core::{
    resource::{Architecture, Connection, ConnectionType, Resource, ResourceShell},
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
fn tf_resource_to_raw(resource: &TfResource, logical_id: &str) -> RawTfResource {
    let attrs: HashMap<String, JsonValue> = resource
        .attrs
        .iter()
        .filter_map(|(k, v)| tf_value_to_json(v).map(|jv| (k.clone(), jv)))
        .collect();

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
        TfValue::VarRef(_) | TfValue::LocalRef(_) | TfValue::ResourceRef { .. } | TfValue::Unknown => {
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

/// Deduplicated edge key: `(source, target, type)`.
type EdgeKey = (String, String, String);

fn edge_key(source: &str, target: &str, conn_type: &ConnectionType) -> EdgeKey {
    let type_str = match conn_type {
        ConnectionType::EventSource => "EventSource",
        ConnectionType::Invocation => "Invocation",
        ConnectionType::DataFlow => "DataFlow",
        ConnectionType::Notification => "Notification",
    };
    (source.to_string(), target.to_string(), type_str.to_string())
}

/// Push a connection only if:
/// 1. Both endpoints exist in `nodes`.
/// 2. The `(source, target, type)` triple has not been seen yet.
fn push_unique(
    connections: &mut Vec<Connection>,
    seen: &mut HashSet<EdgeKey>,
    nodes: &HashSet<String>,
    source: &str,
    target: &str,
    conn_type: ConnectionType,
) {
    if !nodes.contains(source) || !nodes.contains(target) {
        return;
    }
    let key = edge_key(source, target, &conn_type);
    if seen.insert(key) {
        connections.push(Connection {
            source: LogicalId::new(source),
            target: LogicalId::new(target),
            connection_type: conn_type,
            batch_size: None,
            parallelization_factor: None,
            factor: None,
            source_hint: None,
        });
    }
}

/// Recursively collect every `ResourceRef` reachable from `value`.
///
/// Each found ref is appended to `out` as `(resource_type, name, attr)`.
fn collect_resource_refs<'a>(
    value: &'a TfValue,
    out: &mut Vec<(&'a str, &'a str, &'a str)>,
) {
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
    let mut connections: Vec<Connection> = Vec::new();
    let mut seen: HashSet<EdgeKey> = HashSet::new();

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
                    &mut connections,
                    &mut seen,
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

            // Collect all ResourceRefs from blocks (e.g. lambda_function,
            // queue, topic) — these are the targets.
            let mut target_refs: Vec<(&str, &str, &str)> = Vec::new();
            for block_list in src_resource.blocks.values() {
                for block_attrs in block_list {
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
                    &mut connections,
                    &mut seen,
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

        for attr_val in src_resource.attrs.values() {
            collect_resource_refs(attr_val, &mut refs);
        }

        for block_list in src_resource.blocks.values() {
            for block_attrs in block_list {
                for attr_val in block_attrs.values() {
                    collect_resource_refs(attr_val, &mut refs);
                }
            }
        }

        for (tgt_type, tgt_name, _attr) in refs {
            let tgt_lid = logical_id_for(tgt_type, tgt_name);
            let conn_type = classify_connection(src_type, tgt_type);
            push_unique(
                &mut connections,
                &mut seen,
                &nodes,
                &src_lid,
                &tgt_lid,
                conn_type,
            );
        }
    }

    connections
}

/// Determine `ConnectionType` for a generic (non-ESM, non-notification) edge.
fn classify_connection(src_type: &str, tgt_type: &str) -> ConnectionType {
    if src_type == "aws_lambda_function" {
        if STORAGE_RESOURCE_TYPES.contains(&tgt_type) {
            return ConnectionType::DataFlow;
        }
        if COMPUTE_RESOURCE_TYPES.contains(&tgt_type) {
            return ConnectionType::Invocation;
        }
    }
    // Fallback: generic DataFlow
    ConnectionType::DataFlow
}
