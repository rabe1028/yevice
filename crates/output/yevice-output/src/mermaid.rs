//! Mermaid flowchart renderer.

use std::collections::{HashMap, HashSet};

use yevice_core::cost::ArchitectureCost;
use yevice_core::resource::ConnectionType;
use yevice_core::topology::TopologyNode;
use yevice_core::types::LogicalId;

use crate::ArchitectureRenderer;
use crate::error::RenderError;

/// Renders an [`ArchitectureCost`] as a Mermaid `flowchart LR` diagram.
///
/// - Each topology node becomes either a leaf node `id["label (ResourceType)"]`
///   or a `subgraph id["label"]` container when it has children.
/// - Containment is determined by `TopologyNode::group`: a node whose `group`
///   field points to another node in the topology is a child of that node.
/// - Nesting is unlimited: `subgraph` blocks are emitted recursively.
/// - Each node appears exactly once (no double-rendering).
/// - Cycles in the `group` relation are detected via a visited set; nodes
///   involved in a cycle are treated as roots.
/// - Each connection becomes a labeled arrow `src -->|Type| dst`.
/// - Node IDs are sanitized to Mermaid-safe identifiers (alphanumeric + `_`).
pub struct MermaidRenderer;

impl ArchitectureRenderer for MermaidRenderer {
    fn format_name(&self) -> &'static str {
        "mermaid"
    }

    fn render(&self, cost: &ArchitectureCost) -> Result<String, RenderError> {
        let topology = &cost.topology;

        // Build a deterministic ID → sanitized Mermaid ID map.
        // If two distinct logical IDs sanitize to the same string, append
        // `_2`, `_3`, … (in topology.nodes order) to make them unique.
        // Nodes whose sanitized form is already unique get no suffix.
        let id_map: HashMap<&LogicalId, String> = {
            // First pass: collect raw sanitized IDs in order.
            let raw: Vec<(&LogicalId, String)> = topology
                .nodes
                .iter()
                .map(|n| (&n.logical_id, sanitize_id(n.logical_id.as_str())))
                .collect();

            // Count how many times each raw sanitized ID appears.
            let mut occurrence_count: HashMap<&str, usize> = HashMap::new();
            for (_, raw_id) in &raw {
                *occurrence_count.entry(raw_id.as_str()).or_insert(0) += 1;
            }

            // Second pass: assign unique IDs.
            // For IDs that appear more than once, maintain a per-base counter
            // so the second occurrence gets `_2`, the third `_3`, etc.
            let mut assigned_counter: HashMap<String, usize> = HashMap::new();
            let mut result: HashMap<&LogicalId, String> = HashMap::new();
            for (lid, raw_id) in &raw {
                if occurrence_count[raw_id.as_str()] == 1 {
                    // No collision — use as-is.
                    result.insert(lid, raw_id.clone());
                } else {
                    // Collision — append counter starting from 1; first gets no
                    // suffix, subsequent ones get `_2`, `_3`, …
                    let cnt = assigned_counter.entry(raw_id.clone()).or_insert(0);
                    *cnt += 1;
                    let unique_id = if *cnt == 1 {
                        raw_id.clone()
                    } else {
                        format!("{raw_id}_{cnt}")
                    };
                    result.insert(lid, unique_id);
                }
            }
            result
        };

        // Build children map: parent_id → children (in topology.nodes Vec order).
        // Only record a parent relationship when the parent exists in the topology.
        let node_id_set: HashSet<&LogicalId> =
            topology.nodes.iter().map(|n| &n.logical_id).collect();

        let mut children: HashMap<&LogicalId, Vec<&TopologyNode>> = HashMap::new();
        for node in &topology.nodes {
            if let Some(group_id) = &node.group
                && node_id_set.contains(group_id)
            {
                children.entry(group_id).or_default().push(node);
            }
        }

        // Determine roots: nodes whose group is None, or whose group points to a
        // non-existent node (dangling parent).
        let roots: Vec<&TopologyNode> = topology
            .nodes
            .iter()
            .filter(|n| match &n.group {
                None => true,
                Some(g) => !node_id_set.contains(g),
            })
            .collect();

        let mut lines: Vec<String> = Vec::new();
        lines.push("flowchart LR".to_string());

        // Cycle guard: track nodes that have already been emitted.
        let mut visited: HashSet<&LogicalId> = HashSet::new();

        // Emit from roots recursively.
        for root in &roots {
            emit_node(root, &id_map, &children, &mut visited, &mut lines, 1);
        }

        // Emit edges.
        for conn in &topology.connections {
            let src = id_map
                .get(&conn.source)
                .cloned()
                .unwrap_or_else(|| sanitize_id(conn.source.as_str()));
            let dst = id_map
                .get(&conn.target)
                .cloned()
                .unwrap_or_else(|| sanitize_id(conn.target.as_str()));
            let label = connection_type_label(&conn.connection_type);
            lines.push(format!("    {src} -->|{label}| {dst}"));
        }

        Ok(lines.join("\n"))
    }
}

/// Recursively emit a node.
///
/// - If the node has children (it is a container), emit a `subgraph` block
///   using the node's own label as the title, then recurse into children.
/// - If the node has no children (it is a leaf), emit a plain node line.
/// - The `visited` set prevents double-rendering and breaks cycles.
/// - `depth` controls indentation (1 = top-level inside `flowchart LR`).
fn emit_node<'a>(
    node: &'a TopologyNode,
    id_map: &HashMap<&'a LogicalId, String>,
    children: &HashMap<&'a LogicalId, Vec<&'a TopologyNode>>,
    visited: &mut HashSet<&'a LogicalId>,
    lines: &mut Vec<String>,
    depth: usize,
) {
    if !visited.insert(&node.logical_id) {
        // Already emitted — skip (cycle guard).
        return;
    }

    let indent = "    ".repeat(depth);
    let mermaid_id = &id_map[&node.logical_id];

    if let Some(child_nodes) = children.get(&node.logical_id) {
        // Container: emit subgraph with node's own label as title.
        let label = escape_mermaid_label(&node_label(node));
        lines.push(format!(r#"{indent}subgraph {mermaid_id}["{label}"]"#));
        for child in child_nodes {
            emit_node(child, id_map, children, visited, lines, depth + 1);
        }
        lines.push(format!("{indent}end"));
    } else {
        // Leaf: plain node.
        let label = escape_mermaid_label(&node_label(node));
        lines.push(format!(r#"{indent}{mermaid_id}["{label}"]"#));
    }
}

/// Convert a [`LogicalId`] to a Mermaid-safe identifier.
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
fn node_label(node: &TopologyNode) -> String {
    let name = node
        .label
        .as_deref()
        .unwrap_or_else(|| node.logical_id.as_str());
    format!("{} ({})", name, node.resource_type.as_str())
}

/// Escape characters that would break Mermaid's `["..."]` label syntax.
///
/// Inside `["..."]`, a bare `"` would end the string prematurely.
/// Replace `"` with `#quot;` (Mermaid HTML entity syntax).
fn escape_mermaid_label(s: &str) -> String {
    s.replace('"', "#quot;")
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
    use yevice_core::cost::ArchitectureCost;
    use yevice_core::resource::{Connection, ConnectionType, Provider};
    use yevice_core::topology::{Topology, TopologyNode};
    use yevice_core::types::{LogicalId, ResourceType};

    #[test]
    fn sanitize_id_replaces_non_alphanumeric() {
        assert_eq!(
            sanitize_id("AWS::Lambda::Function"),
            "AWS__Lambda__Function"
        );
        assert_eq!(sanitize_id("my-resource"), "my_resource");
        assert_eq!(sanitize_id("MyTable"), "MyTable");
    }

    #[test]
    fn escape_mermaid_label_replaces_double_quote() {
        assert_eq!(escape_mermaid_label(r#"A "B" C"#), "A #quot;B#quot; C");
        assert_eq!(escape_mermaid_label("plain"), "plain");
    }

    fn make_leaf_node(logical_id: &str) -> TopologyNode {
        TopologyNode {
            logical_id: LogicalId::new(logical_id),
            resource_type: ResourceType::new("aws_lambda_function"),
            provider: Provider::Aws,
            service_id: "aws.lambda".to_string(),
            label: None,
            group: None,
        }
    }

    fn minimal_cost(topology: Topology) -> ArchitectureCost {
        use yevice_core::types::Region;
        ArchitectureCost {
            name: "test".into(),
            resources: vec![],
            bindings: vec![],
            region: Region::new("ap-northeast-1"),
            topology,
        }
    }

    /// Two logical IDs that differ only by `-` vs `_` must map to distinct
    /// Mermaid IDs, and an edge between them must be correctly wired.
    #[test]
    fn colliding_sanitized_ids_get_unique_suffixes() {
        let node_a = make_leaf_node("my-resource"); // sanitizes to "my_resource"
        let node_b = make_leaf_node("my_resource"); // also "my_resource" → collision

        let conn = Connection {
            source: LogicalId::new("my-resource"),
            target: LogicalId::new("my_resource"),
            connection_type: ConnectionType::DataFlow,
            batch_size: None,
            parallelization_factor: None,
            factor: None,
            source_hint: None,
        };

        let topology = Topology {
            nodes: vec![node_a, node_b],
            connections: vec![conn],
        };
        let cost = minimal_cost(topology);
        let output = MermaidRenderer.render(&cost).unwrap();

        // The two nodes must have different IDs in the output.
        // First occurrence keeps "my_resource", second gets "my_resource_2".
        assert!(
            output.contains("my_resource["),
            "first node should be 'my_resource': {output}"
        );
        assert!(
            output.contains("my_resource_2["),
            "second node should be 'my_resource_2': {output}"
        );

        // The edge must wire the two distinct IDs, not use the same ID twice.
        assert!(
            output.contains("my_resource -->|DataFlow| my_resource_2")
                || output.contains("my_resource_2 -->|DataFlow| my_resource"),
            "edge must connect the two distinct IDs: {output}"
        );
    }

    /// Nodes with no collision must keep their plain sanitized ID (no suffix).
    #[test]
    fn non_colliding_ids_have_no_suffix() {
        let node_a = make_leaf_node("alpha");
        let node_b = make_leaf_node("beta");
        let topology = Topology {
            nodes: vec![node_a, node_b],
            connections: vec![],
        };
        let cost = minimal_cost(topology);
        let output = MermaidRenderer.render(&cost).unwrap();
        assert!(
            output.contains("alpha["),
            "alpha should have no suffix: {output}"
        );
        assert!(
            output.contains("beta["),
            "beta should have no suffix: {output}"
        );
        assert!(
            !output.contains("alpha_2"),
            "alpha must not get a suffix: {output}"
        );
        assert!(
            !output.contains("beta_2"),
            "beta must not get a suffix: {output}"
        );
    }
}
