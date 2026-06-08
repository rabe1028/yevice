//! draw.io (mxGraph XML) renderer.

use std::collections::HashMap;
use std::fmt::Write as _;

use yevice_core::cost::ArchitectureCost;
use yevice_core::resource::{ConnectionType, Provider};
use yevice_core::topology::TopologyNode;
use yevice_core::types::LogicalId;

use crate::ArchitectureRenderer;
use crate::error::RenderError;

/// Grid layout constants.
const CELL_WIDTH: u32 = 160;
const CELL_HEIGHT: u32 = 60;
const CELL_COL_STEP: u32 = 200;
const CELL_ROW_STEP: u32 = 120;
const GRID_COLUMNS: u32 = 4;

/// Group container padding (pixels inside the container cell).
const GROUP_PADDING: u32 = 20;
const GROUP_HEADER: u32 = 30;

/// Renders an [`ArchitectureCost`] as a draw.io-compatible `<mxGraphModel>` XML string.
///
/// Layout:
/// - Non-grouped nodes are placed on a simple 4-column grid.
/// - Grouped nodes share a container `<mxCell>` and their positions are relative
///   to the container.  Containers are stacked below the ungrouped nodes.
///
/// Provider fill colours:
/// - AWS        → `#FF9900` (orange)
/// - GCP        → `#4285F4` (blue)
/// - Cloudflare → `#F38020` (flame)
/// - Other      → `#E0E0E0` (grey)
pub struct DrawIoRenderer;

impl ArchitectureRenderer for DrawIoRenderer {
    fn format_name(&self) -> &'static str {
        "drawio"
    }

    fn render(&self, cost: &ArchitectureCost) -> Result<String, RenderError> {
        let topology = &cost.topology;

        // Assign stable integer IDs.  Cell 0 and 1 are reserved for mxGraph.
        // We start node/group cells at 2.
        let mut next_id: u32 = 2;

        // Map from LogicalId → cell integer id.
        let mut node_cell_ids: HashMap<&LogicalId, u32> = HashMap::new();
        // Map from group LogicalId → container cell integer id.
        let mut group_cell_ids: HashMap<&LogicalId, u32> = HashMap::new();

        // Collect groups and ungrouped nodes (preserve Vec order for determinism).
        let mut group_order: Vec<&LogicalId> = Vec::new();
        let mut groups: HashMap<&LogicalId, Vec<&TopologyNode>> = HashMap::new();
        let mut ungrouped: Vec<&TopologyNode> = Vec::new();

        for node in &topology.nodes {
            if let Some(group_id) = &node.group {
                if !groups.contains_key(group_id) {
                    group_order.push(group_id);
                }
                groups.entry(group_id).or_default().push(node);
            } else {
                ungrouped.push(node);
            }
        }

        // --- Assign cell IDs for group containers first, then nodes ---
        for group_id in &group_order {
            group_cell_ids.insert(group_id, next_id);
            next_id += 1;
        }
        for node in &topology.nodes {
            node_cell_ids.insert(&node.logical_id, next_id);
            next_id += 1;
        }
        // Edge cells start after all node cells.
        let mut edge_id = next_id;

        // --- Build XML lines ---
        let mut xml = String::new();
        xml.push_str("<mxGraphModel><root>\n");
        xml.push_str("  <mxCell id=\"0\"/>\n");
        xml.push_str("  <mxCell id=\"1\" parent=\"0\"/>\n");

        // Ungrouped nodes on a simple grid (parent = cell 1).
        for (index, node) in ungrouped.iter().enumerate() {
            let index = index as u32;
            let col = index % GRID_COLUMNS;
            let row = index / GRID_COLUMNS;
            let x = col * CELL_COL_STEP;
            let y = row * CELL_ROW_STEP;

            let cell_id = node_cell_ids[&node.logical_id];
            let value = xml_escape(node_label(node).as_str());
            let style = node_style(node.provider);

            let _ = writeln!(
                xml,
                "  <mxCell id=\"{cell_id}\" value=\"{value}\" style=\"{style}\" vertex=\"1\" parent=\"1\">\
                    <mxGeometry x=\"{x}\" y=\"{y}\" width=\"{CELL_WIDTH}\" height=\"{CELL_HEIGHT}\" as=\"geometry\"/>\
                  </mxCell>"
            );
        }

        // Grouped nodes: first emit a container cell, then member cells relative to it.
        let ungrouped_rows = (ungrouped.len() as u32).div_ceil(GRID_COLUMNS);
        let container_base_y = ungrouped_rows * CELL_ROW_STEP + if ungrouped.is_empty() { 0 } else { CELL_ROW_STEP };

        for (group_index, group_id) in group_order.iter().enumerate() {
            let members = &groups[group_id];
            let container_id = group_cell_ids[group_id];

            // Container dimensions: wrap the member grid plus padding.
            let member_cols = GRID_COLUMNS.min(members.len() as u32);
            let member_rows = (members.len() as u32).div_ceil(GRID_COLUMNS);
            let container_w = member_cols * CELL_COL_STEP + GROUP_PADDING * 2;
            let container_h = GROUP_HEADER + member_rows * CELL_ROW_STEP + GROUP_PADDING;
            let container_x: u32 = 0;
            let container_y = container_base_y + group_index as u32 * (container_h + CELL_ROW_STEP);

            let container_label = xml_escape(group_id.as_str());
            let _ = writeln!(
                xml,
                "  <mxCell id=\"{container_id}\" value=\"{container_label}\" \
                    style=\"swimlane;\" vertex=\"1\" parent=\"1\">\
                    <mxGeometry x=\"{container_x}\" y=\"{container_y}\" \
                                width=\"{container_w}\" height=\"{container_h}\" as=\"geometry\"/>\
                  </mxCell>"
            );

            // Member cells parented to the container.
            for (member_index, node) in members.iter().enumerate() {
                let member_index = member_index as u32;
                let col = member_index % GRID_COLUMNS;
                let row = member_index / GRID_COLUMNS;
                let x = GROUP_PADDING + col * CELL_COL_STEP;
                let y = GROUP_HEADER + GROUP_PADDING + row * CELL_ROW_STEP;

                let cell_id = node_cell_ids[&node.logical_id];
                let value = xml_escape(node_label(node).as_str());
                let style = node_style(node.provider);

                let _ = writeln!(
                    xml,
                    "  <mxCell id=\"{cell_id}\" value=\"{value}\" style=\"{style}\" vertex=\"1\" parent=\"{container_id}\">\
                        <mxGeometry x=\"{x}\" y=\"{y}\" width=\"{CELL_WIDTH}\" height=\"{CELL_HEIGHT}\" as=\"geometry\"/>\
                      </mxCell>"
                );
            }
        }

        // Edge cells.
        for conn in &topology.connections {
            let src_id = node_cell_ids
                .get(&conn.source)
                .copied()
                .unwrap_or(0);
            let dst_id = node_cell_ids
                .get(&conn.target)
                .copied()
                .unwrap_or(0);
            let label = xml_escape(connection_type_label(&conn.connection_type));
            let _ = writeln!(
                xml,
                "  <mxCell id=\"{edge_id}\" value=\"{label}\" style=\"endArrow=block;\" \
                    edge=\"1\" source=\"{src_id}\" target=\"{dst_id}\" parent=\"1\">\
                    <mxGeometry relative=\"1\" as=\"geometry\"/>\
                  </mxCell>"
            );
            edge_id += 1;
        }

        xml.push_str("</root></mxGraphModel>");
        Ok(xml)
    }
}

/// Escape XML special characters: `& < > " '`.
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

/// Build the display label for a node (`"<name> (<resource_type>)"`).
fn node_label(node: &TopologyNode) -> String {
    let name = node
        .label
        .as_deref()
        .unwrap_or_else(|| node.logical_id.as_str());
    format!("{} ({})", name, node.resource_type.as_str())
}

/// Return a draw.io style string for a given provider.
///
/// Each provider gets a distinct `fillColor` so nodes are visually grouped by cloud.
fn node_style(provider: Provider) -> &'static str {
    match provider {
        Provider::Aws => {
            "rounded=1;whiteSpace=wrap;html=1;fillColor=#FF9900;fontColor=#000000;strokeColor=#d6b656;"
        }
        Provider::Gcp => {
            "rounded=1;whiteSpace=wrap;html=1;fillColor=#4285F4;fontColor=#ffffff;strokeColor=#1a5cb3;"
        }
        Provider::Cloudflare => {
            "rounded=1;whiteSpace=wrap;html=1;fillColor=#F38020;fontColor=#ffffff;strokeColor=#c06010;"
        }
        Provider::Other => {
            "rounded=1;whiteSpace=wrap;html=1;fillColor=#E0E0E0;fontColor=#000000;strokeColor=#909090;"
        }
    }
}

/// Human-readable label for a connection type (used as draw.io edge label).
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
    fn xml_escape_handles_all_special_chars() {
        assert_eq!(xml_escape("a&b"), "a&amp;b");
        assert_eq!(xml_escape("<tag>"), "&lt;tag&gt;");
        assert_eq!(xml_escape("\"quoted\""), "&quot;quoted&quot;");
        assert_eq!(xml_escape("it's"), "it&apos;s");
        assert_eq!(xml_escape("plain"), "plain");
    }
}
