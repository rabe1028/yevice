//! Terraform config → Architecture conversion using the adapter registry.

use std::collections::HashMap;

use serde_json::Value as JsonValue;
use yevice_core::{
    resource::{Architecture, Resource, ResourceShell},
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
            let shell = adapters
                .lookup(&resource.resource_type)
                .and_then(|adapter| adapter.convert(&raw).ok())
                .unwrap_or_else(|| ResourceShell::other(&resource.resource_type));
            Resource {
                logical_id: LogicalId::new(&logical_id),
                resource_type: ResourceType::new(&resource.resource_type),
                shell,
            }
        })
        .collect();

    Architecture {
        name: name.to_string(),
        region: Region::new(region),
        resources,
        connections: Vec::new(),
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
/// Returns `None` for unresolved references (`VarRef`, `LocalRef`, `Unknown`).
fn tf_value_to_json(value: &TfValue) -> Option<JsonValue> {
    match value {
        TfValue::String(s) => Some(JsonValue::String(s.clone())),
        TfValue::Number(n) => serde_json::Number::from_f64(*n).map(JsonValue::Number),
        TfValue::Bool(b) => Some(JsonValue::Bool(*b)),
        TfValue::VarRef(_) | TfValue::LocalRef(_) | TfValue::Unknown => None,
    }
}
