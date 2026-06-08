//! JSON Schema generation and template YAML generation from cost models.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::cost::ArchitectureCost;

/// JSON Schema for the hierarchical usage parameters file.
///
/// Structure:
/// ```yaml
/// IngestFunction:
///   requests: 5000000
///   avg_duration_ms: 200
/// DataTable:
///   write_request_units: 500000
/// ```
#[derive(Debug, Serialize)]
pub struct UsageSchema {
    #[serde(rename = "$schema")]
    pub schema: String,
    pub title: String,
    pub description: String,
    #[serde(rename = "type")]
    pub schema_type: String,
    pub properties: BTreeMap<String, ResourceSchema>,
    pub required: Vec<String>,
    #[serde(rename = "additionalProperties")]
    pub additional_properties: bool,
}

#[derive(Debug, Serialize)]
pub struct ResourceSchema {
    #[serde(rename = "type")]
    pub schema_type: String,
    pub description: String,
    pub properties: BTreeMap<String, PropertySchema>,
    pub required: Vec<String>,
    #[serde(rename = "additionalProperties")]
    pub additional_properties: bool,
}

#[derive(Debug, Serialize)]
pub struct PropertySchema {
    #[serde(rename = "type")]
    pub schema_type: String,
    pub description: String,
}

/// Generate a JSON Schema from an `ArchitectureCost`.
pub fn generate_usage_schema(arch: &ArchitectureCost) -> UsageSchema {
    let mut properties = BTreeMap::new();
    let mut required = Vec::new();

    for resource in &arch.resources {
        if resource.required_variables.is_empty() {
            continue;
        }

        let logical_id = resource.logical_id.to_string();
        let prefix = format!("{logical_id}_");

        let mut resource_props = BTreeMap::new();
        let mut resource_required = Vec::new();

        for var in &resource.required_variables {
            let var_name = var.name.to_string();
            let short_name = var_name.strip_prefix(&prefix).unwrap_or(&var_name);

            resource_props.insert(
                short_name.to_string(),
                PropertySchema {
                    schema_type: "number".to_string(),
                    description: format!("{} [{}]", var.description, var.unit),
                },
            );
            resource_required.push(short_name.to_string());
        }

        required.push(logical_id.clone());
        properties.insert(
            logical_id,
            ResourceSchema {
                schema_type: "object".to_string(),
                description: resource.label.clone(),
                properties: resource_props,
                required: resource_required,
                additional_properties: false,
            },
        );
    }

    UsageSchema {
        schema: "https://json-schema.org/draft/2020-12/schema".to_string(),
        title: format!("Usage parameters for {}", arch.name),
        description: "Usage parameters for cost evaluation".to_string(),
        schema_type: "object".to_string(),
        properties,
        required,
        additional_properties: true,
    }
}

/// Generate a template usage YAML with placeholder values.
pub fn generate_usage_template(arch: &ArchitectureCost) -> String {
    let mut lines = Vec::new();
    lines.push(format!("# Usage parameters for: {}", arch.name));
    lines.push(format!("# Region: {}", arch.region));
    lines.push(String::new());

    for resource in &arch.resources {
        if resource.required_variables.is_empty() {
            continue;
        }

        let logical_id = resource.logical_id.to_string();
        let prefix = format!("{logical_id}_");

        lines.push(format!("# {}", resource.label));
        lines.push(format!("{logical_id}:"));

        for var in &resource.required_variables {
            let var_name = var.name.to_string();
            let short_name = var_name.strip_prefix(&prefix).unwrap_or(&var_name);
            lines.push(format!(
                "  {short_name}: 0  # {} [{}]",
                var.description, var.unit
            ));
        }
        lines.push(String::new());
    }

    lines.join("\n")
}
