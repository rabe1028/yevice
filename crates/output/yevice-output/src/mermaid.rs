//! Mermaid flowchart renderer.

use std::collections::HashMap;

use yevice_core::cost::ArchitectureCost;
use yevice_core::resource::ConnectionType;
use yevice_core::types::LogicalId;

use crate::ArchitectureRenderer;
use crate::error::RenderError;

/// Renders an [`ArchitectureCost`] as a Mermaid `flowchart LR` diagram.
///
/// - Each topology node becomes an `id["label (ResourceType)"]` node.
/// - Each connection becomes a labeled arrow `src -->|Type| dst`.
/// - Nodes sharing the same `group` are wrapped in a `subgraph` block.
/// - Node IDs are sanitized to Mermaid-safe identifiers (alphanumeric + `_`).
pub struct MermaidRenderer;

impl ArchitectureRenderer for MermaidRenderer {
    fn format_name(&self) -> &'static str {
        "mermaid"
    }

    fn render(&self, cost: &ArchitectureCost) -> Result<String, RenderError> {
        let topology = &cost.topology;

        // Build a deterministic ID → sanitized Mermaid ID map.
        let id_map: HashMap<&LogicalId, String> = topology
            .nodes
            .iter()
            .map(|n| (&n.logical_id, sanitize_id(n.logical_id.as_str())))
            .collect();

        let mut lines: Vec<String> = Vec::new();
        lines.push("flowchart LR".to_string());

        // Separate nodes into grouped and ungrouped.
        let mut groups: HashMap<&LogicalId, Vec<&yevice_core::topology::TopologyNode>> =
            HashMap::new();
        let mut ungrouped: Vec<&yevice_core::topology::TopologyNode> = Vec::new();

        for node in &topology.nodes {
            if let Some(group_id) = &node.group {
                groups.entry(group_id).or_default().push(node);
            } else {
                ungrouped.push(node);
            }
        }

        // Emit ungrouped nodes.
        for node in &ungrouped {
            let mermaid_id = &id_map[&node.logical_id];
            let label = node_label(node);
            lines.push(format!("    {mermaid_id}[\"{label}\"]"));
        }

        // Emit subgraph blocks for grouped nodes, in a deterministic order.
        let mut group_ids: Vec<&&LogicalId> = groups.keys().collect();
        group_ids.sort_by_key(|id| id.as_str());

        for group_id in group_ids {
            let members = &groups[group_id];
            lines.push(format!("    subgraph {}", sanitize_id(group_id.as_str())));
            // Emit members in Vec order (deterministic — topology.nodes order preserved).
            for node in members {
                let mermaid_id = &id_map[&node.logical_id];
                let label = node_label(node);
                lines.push(format!("        {mermaid_id}[\"{label}\"]"));
            }
            lines.push("    end".to_string());
        }

        // Emit edges.
        for conn in &topology.connections {
            let src = id_map.get(&conn.source).cloned().unwrap_or_else(|| {
                sanitize_id(conn.source.as_str())
            });
            let dst = id_map.get(&conn.target).cloned().unwrap_or_else(|| {
                sanitize_id(conn.target.as_str())
            });
            let label = connection_type_label(&conn.connection_type);
            lines.push(format!("    {src} -->|{label}| {dst}"));
        }

        Ok(lines.join("\n"))
    }
}

/// Convert a [`LogicalId`] or group ID to a Mermaid-safe identifier.
///
/// Mermaid node IDs must consist of alphanumeric characters and underscores.
/// Any other character is replaced with `_`.
fn sanitize_id(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// Build the display label for a node.
///
/// Format: `"<label or logical_id> (<resource_type>)"`.
fn node_label(node: &yevice_core::topology::TopologyNode) -> String {
    let name = node
        .label
        .as_deref()
        .unwrap_or_else(|| node.logical_id.as_str());
    format!("{} ({})", name, node.resource_type.as_str())
}

/// Human-readable label for a connection type (used as Mermaid edge label).
fn connection_type_label(ct: &ConnectionType) -> &'static str {
    match ct {
        ConnectionType::EventSource => "EventSource",
        ConnectionType::Invocation => "Invocation",
        ConnectionType::DataFlow => "DataFlow",
        ConnectionType::Notification => "Notification",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_id_replaces_non_alphanumeric() {
        assert_eq!(sanitize_id("AWS::Lambda::Function"), "AWS__Lambda__Function");
        assert_eq!(sanitize_id("my-resource"), "my_resource");
        assert_eq!(sanitize_id("MyTable"), "MyTable");
    }
}
