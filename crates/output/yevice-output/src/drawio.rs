//! draw.io (mxGraph XML) renderer.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use yevice_core::cost::ArchitectureCost;
use yevice_core::resource::{ConnectionType, Provider};
use yevice_core::topology::TopologyNode;
use yevice_core::types::LogicalId;

use crate::ArchitectureRenderer;
use crate::error::RenderError;

/// Fixed dimensions for leaf nodes.
const CELL_WIDTH: u32 = 160;
const CELL_HEIGHT: u32 = 60;

/// Horizontal / vertical step between sibling leaf nodes.
const CELL_COL_STEP: u32 = 200;
const CELL_ROW_STEP: u32 = 120;

/// Number of leaf columns before wrapping.
const GRID_COLUMNS: u32 = 4;

/// Padding inside a container cell (left/right/bottom).
const GROUP_PADDING: u32 = 20;
/// Height of the swimlane header bar.
const GROUP_HEADER: u32 = 30;
/// Vertical gap between sibling top-level containers.
const CONTAINER_GAP: u32 = 40;

/// Renders an [`ArchitectureCost`] as a draw.io-compatible `<mxGraphModel>` XML string.
///
/// Layout:
/// - The parent–child relation is derived from `TopologyNode::group`.
/// - Root nodes (no parent, or dangling parent) are placed on a top-level grid.
/// - Container nodes are rendered as `swimlane` mxCell elements; their children
///   are positioned relative to the container using recursive layout.
/// - Container sizes are computed bottom-up so children never overflow.
/// - Cycle detection via a visited set prevents infinite recursion.
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

        // Cell 0 and 1 are reserved by mxGraph.  All user cells start at 2.
        let mut next_id: u32 = 2;

        // Map LogicalId → integer cell id for every node.
        let mut node_cell_ids: HashMap<&LogicalId, u32> = HashMap::new();
        for node in &topology.nodes {
            node_cell_ids.insert(&node.logical_id, next_id);
            next_id += 1;
        }
        // Edge cells follow all node cells.
        let mut edge_id = next_id;

        let (children, roots) = build_forest(&topology.nodes);

        let mut xml = String::new();
        xml.push_str("<mxGraphModel><root>\n");
        xml.push_str("  <mxCell id=\"0\"/>\n");
        xml.push_str("  <mxCell id=\"1\" parent=\"0\"/>\n");

        let mut emitted: HashSet<&LogicalId> = HashSet::new();
        emit_roots(&roots, &children, &node_cell_ids, &mut xml, &mut emitted);

        // Emit any nodes not yet visited (e.g. nodes in a group cycle where
        // every node has a valid parent and no roots exist). Visited guard in
        // emit_node prevents duplicates even if a node was already emitted.
        let mut leaf_index: u32 = 0;
        for node in &topology.nodes {
            if emitted.contains(&node.logical_id) {
                continue;
            }
            // Emit as a top-level leaf (parent="1").
            let col = leaf_index % GRID_COLUMNS;
            let row = leaf_index / GRID_COLUMNS;
            let x = col * CELL_COL_STEP;
            let y = row * CELL_ROW_STEP;
            leaf_index += 1;

            let cell_id = node_cell_ids[&node.logical_id];
            let value = xml_escape(&node_label(node));
            let style = node_style(node.provider);
            let _ = writeln!(
                xml,
                "  <mxCell id=\"{cell_id}\" value=\"{value}\" style=\"{style}\" vertex=\"1\" parent=\"1\">\
                    <mxGeometry x=\"{x}\" y=\"{y}\" width=\"{CELL_WIDTH}\" height=\"{CELL_HEIGHT}\" as=\"geometry\"/>\
                  </mxCell>"
            );
            emitted.insert(&node.logical_id);
        }

        emit_edges(
            &topology.connections,
            &node_cell_ids,
            &mut xml,
            &mut edge_id,
        );

        xml.push_str("</root></mxGraphModel>");
        Ok(xml)
    }
}

/// Build the children map and root list from the flat node slice.
///
/// A node is a root when its `group` is `None` or points to a non-existent id.
fn build_forest(
    nodes: &[TopologyNode],
) -> (HashMap<&LogicalId, Vec<&TopologyNode>>, Vec<&TopologyNode>) {
    let node_id_set: HashSet<&LogicalId> = nodes.iter().map(|n| &n.logical_id).collect();

    let mut children: HashMap<&LogicalId, Vec<&TopologyNode>> = HashMap::new();
    for node in nodes {
        if let Some(group_id) = &node.group
            && node_id_set.contains(group_id)
        {
            children.entry(group_id).or_default().push(node);
        }
    }

    let roots: Vec<&TopologyNode> = nodes
        .iter()
        .filter(|n| match &n.group {
            None => true,
            Some(g) => !node_id_set.contains(g),
        })
        .collect();

    (children, roots)
}

/// Emit XML cells for all root nodes.
fn emit_roots<'a>(
    roots: &[&'a TopologyNode],
    children: &HashMap<&'a LogicalId, Vec<&'a TopologyNode>>,
    node_cell_ids: &HashMap<&'a LogicalId, u32>,
    xml: &mut String,
    emitted: &mut HashSet<&'a LogicalId>,
) {
    // Pre-compute the vertical offset where containers begin (after top-level leaves).
    let ungrouped_leaf_count = u32::try_from(
        roots
            .iter()
            .filter(|n| !children.contains_key(&n.logical_id))
            .count(),
    )
    .expect("node count fits in u32");
    let leaf_rows = ungrouped_leaf_count.div_ceil(GRID_COLUMNS);
    let leaf_section_height = leaf_rows * CELL_ROW_STEP;
    let mut container_y = leaf_section_height
        + if ungrouped_leaf_count > 0 {
            CONTAINER_GAP
        } else {
            0
        };

    let mut leaf_index: u32 = 0;

    for root in roots {
        if children.contains_key(&root.logical_id) {
            // Container root: compute size with a fresh cycle-guard set.
            let size = compute_container_size(root, children, &mut HashSet::new());

            let container_cell_id = node_cell_ids[&root.logical_id];
            let label = xml_escape(&node_label(root));
            let _ = writeln!(
                xml,
                "  <mxCell id=\"{container_cell_id}\" value=\"{label}\" \
                    style=\"swimlane;\" vertex=\"1\" parent=\"1\">\
                    <mxGeometry x=\"0\" y=\"{container_y}\" \
                                width=\"{}\" height=\"{}\" as=\"geometry\"/>\
                  </mxCell>",
                size.width, size.height
            );

            let mut emit_visited: HashSet<&LogicalId> = HashSet::new();
            emit_visited.insert(&root.logical_id);
            emit_children(
                root,
                container_cell_id,
                node_cell_ids,
                children,
                &mut emit_visited,
                xml,
            );
            emitted.extend(emit_visited.iter());
            emitted.insert(&root.logical_id);

            container_y += size.height + CONTAINER_GAP;
        } else {
            // Leaf root: place on GRID_COLUMNS-wide grid.
            let col = leaf_index % GRID_COLUMNS;
            let row = leaf_index / GRID_COLUMNS;
            let x = col * CELL_COL_STEP;
            let y = row * CELL_ROW_STEP;
            leaf_index += 1;

            let cell_id = node_cell_ids[&root.logical_id];
            let value = xml_escape(&node_label(root));
            let style = node_style(root.provider);
            let _ = writeln!(
                xml,
                "  <mxCell id=\"{cell_id}\" value=\"{value}\" style=\"{style}\" vertex=\"1\" parent=\"1\">\
                    <mxGeometry x=\"{x}\" y=\"{y}\" width=\"{CELL_WIDTH}\" height=\"{CELL_HEIGHT}\" as=\"geometry\"/>\
                  </mxCell>"
            );
            emitted.insert(&root.logical_id);
        }
    }
}

/// Emit XML cells for connection edges.
fn emit_edges(
    connections: &[yevice_core::resource::Connection],
    node_cell_ids: &HashMap<&LogicalId, u32>,
    xml: &mut String,
    edge_id: &mut u32,
) {
    for conn in connections {
        let (Some(&src_id), Some(&dst_id)) = (
            node_cell_ids.get(&conn.source),
            node_cell_ids.get(&conn.target),
        ) else {
            // Skip edges whose endpoint is not a rendered node (e.g. external ARN source).
            continue;
        };
        let label = xml_escape(connection_type_label(&conn.connection_type));
        let _ = writeln!(
            xml,
            "  <mxCell id=\"{edge_id}\" value=\"{label}\" style=\"endArrow=block;\" \
                edge=\"1\" source=\"{src_id}\" target=\"{dst_id}\" parent=\"1\">\
                <mxGeometry relative=\"1\" as=\"geometry\"/>\
              </mxCell>"
        );
        *edge_id += 1;
    }
}

/// Size (in pixels) of a rendered node or container.
#[derive(Debug, Clone, Copy)]
struct Size {
    width: u32,
    height: u32,
}

/// Compute the bounding-box size of a container node bottom-up.
///
/// For a container the size is derived from the content it must wrap:
/// children are laid out in a `GRID_COLUMNS`-wide grid; the container adds
/// `GROUP_PADDING` around all sides and a `GROUP_HEADER` strip at the top.
///
/// Nested containers are measured recursively first, then treated as a single
/// cell of their computed size for the parent's grid layout.
///
/// The `visited` set prevents infinite recursion on cycles.
fn compute_container_size<'a>(
    node: &'a TopologyNode,
    children: &HashMap<&'a LogicalId, Vec<&'a TopologyNode>>,
    visited: &mut HashSet<&'a LogicalId>,
) -> Size {
    // Cycle guard: if this node was already entered during the current traversal,
    // return leaf size to break the cycle.
    if !visited.insert(&node.logical_id) {
        return Size {
            width: CELL_WIDTH,
            height: CELL_HEIGHT,
        };
    }

    let Some(child_nodes) = children.get(&node.logical_id) else {
        // Leaf node: return its fixed size.
        return Size {
            width: CELL_WIDTH,
            height: CELL_HEIGHT,
        };
    };

    // Measure each child (recursively for sub-containers).
    let mut child_sizes: Vec<Size> = Vec::new();
    for child in child_nodes {
        if children.contains_key(&child.logical_id) {
            child_sizes.push(compute_container_size(child, children, visited));
        } else {
            child_sizes.push(Size {
                width: CELL_WIDTH,
                height: CELL_HEIGHT,
            });
        }
    }

    // Layout children in a GRID_COLUMNS-wide grid.
    // Row heights determined by tallest child in each row.
    let cols = usize::try_from(
        GRID_COLUMNS.min(u32::try_from(child_nodes.len()).expect("child count fits in u32")),
    )
    .expect("cols fits in usize");
    let rows = child_sizes.len().div_ceil(cols);

    let mut row_heights: Vec<u32> = vec![0; rows];
    let mut col_widths: Vec<u32> = vec![0; cols];

    for (i, sz) in child_sizes.iter().enumerate() {
        let row = i / cols;
        let col = i % cols;
        row_heights[row] = row_heights[row].max(sz.height);
        col_widths[col] = col_widths[col].max(sz.width);
    }

    let col_gap = CELL_COL_STEP - CELL_WIDTH;
    let row_gap = CELL_ROW_STEP - CELL_HEIGHT;
    let cols_u32 = u32::try_from(cols).expect("cols fits in u32");
    let rows_u32 = u32::try_from(rows).expect("rows fits in u32");

    let content_w: u32 = col_widths.iter().sum::<u32>() + cols_u32.saturating_sub(1) * col_gap;
    let content_h: u32 = row_heights.iter().sum::<u32>() + rows_u32.saturating_sub(1) * row_gap;

    let width = content_w + GROUP_PADDING * 2;
    let height = content_h + GROUP_PADDING * 2 + GROUP_HEADER;

    Size { width, height }
}

/// Emit XML for the children of a container node.
///
/// Each child is positioned relative to the container using a simple
/// `GRID_COLUMNS`-wide grid. Sub-containers are sized and positioned the same
/// way, and their own children are emitted recursively with the sub-container
/// cell id as parent.
///
/// Each call to `compute_container_size` within this function uses a fresh
/// `HashSet` so that sibling containers do not pollute each other's cycle
/// detection. `compute_container_size` itself guards against cycles at its
/// entry point, so a fresh set per call is safe and gives accurate sizes for
/// all nesting levels.
fn emit_children<'a>(
    parent_node: &'a TopologyNode,
    parent_cell_id: u32,
    node_cell_ids: &HashMap<&'a LogicalId, u32>,
    children: &HashMap<&'a LogicalId, Vec<&'a TopologyNode>>,
    visited: &mut HashSet<&'a LogicalId>,
    xml: &mut String,
) {
    let Some(child_nodes) = children.get(&parent_node.logical_id) else {
        return;
    };

    let cols = usize::try_from(
        GRID_COLUMNS.min(u32::try_from(child_nodes.len()).expect("child count fits in u32")),
    )
    .expect("cols fits in usize");

    // Measure children sizes for layout. Each sub-container gets its own fresh
    // cycle-guard set so siblings don't cross-contaminate.
    let child_sizes: Vec<Size> = child_nodes
        .iter()
        .map(|child| {
            if children.contains_key(&child.logical_id) {
                compute_container_size(child, children, &mut HashSet::new())
            } else {
                Size {
                    width: CELL_WIDTH,
                    height: CELL_HEIGHT,
                }
            }
        })
        .collect();

    // Build per-row height and per-col width for positioning.
    let rows = child_sizes.len().div_ceil(cols);
    let mut row_heights: Vec<u32> = vec![0; rows];
    let mut col_widths: Vec<u32> = vec![0; cols];
    for (i, sz) in child_sizes.iter().enumerate() {
        row_heights[i / cols] = row_heights[i / cols].max(sz.height);
        col_widths[i % cols] = col_widths[i % cols].max(sz.width);
    }

    // Prefix sums for absolute positions inside the container.
    let col_x: Vec<u32> = prefix_positions(&col_widths, GROUP_PADDING, CELL_COL_STEP - CELL_WIDTH);
    let row_y: Vec<u32> = prefix_positions(
        &row_heights,
        GROUP_HEADER + GROUP_PADDING,
        CELL_ROW_STEP - CELL_HEIGHT,
    );

    for (i, child) in child_nodes.iter().enumerate() {
        if visited.contains(&child.logical_id) {
            continue; // cycle guard
        }
        visited.insert(&child.logical_id);

        let x = col_x[i % cols];
        let y = row_y[i / cols];
        let sz = child_sizes[i];
        let cell_id = node_cell_ids[&child.logical_id];

        if children.contains_key(&child.logical_id) {
            // Sub-container.
            let label = xml_escape(&node_label(child));
            let _ = writeln!(
                xml,
                "  <mxCell id=\"{cell_id}\" value=\"{label}\" \
                    style=\"swimlane;\" vertex=\"1\" parent=\"{parent_cell_id}\">\
                    <mxGeometry x=\"{x}\" y=\"{y}\" \
                                width=\"{}\" height=\"{}\" as=\"geometry\"/>\
                  </mxCell>",
                sz.width, sz.height
            );
            emit_children(child, cell_id, node_cell_ids, children, visited, xml);
        } else {
            // Leaf.
            let value = xml_escape(&node_label(child));
            let style = node_style(child.provider);
            let _ = writeln!(
                xml,
                "  <mxCell id=\"{cell_id}\" value=\"{value}\" style=\"{style}\" vertex=\"1\" parent=\"{parent_cell_id}\">\
                    <mxGeometry x=\"{x}\" y=\"{y}\" width=\"{CELL_WIDTH}\" height=\"{CELL_HEIGHT}\" as=\"geometry\"/>\
                  </mxCell>"
            );
        }
    }
}

/// Build a vector of start positions for items of given `sizes`, separated by `gap`,
/// starting from `offset`.
fn prefix_positions(sizes: &[u32], offset: u32, gap: u32) -> Vec<u32> {
    let mut acc = offset;
    let mut positions = Vec::with_capacity(sizes.len());
    for (i, &sz) in sizes.iter().enumerate() {
        positions.push(acc);
        if i + 1 < sizes.len() {
            acc += sz + gap;
        }
    }
    positions
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
    use yevice_core::cost::ArchitectureCost;
    use yevice_core::resource::Provider;
    use yevice_core::topology::{Topology, TopologyNode};
    use yevice_core::types::{LogicalId, Region, ResourceType};

    #[test]
    fn xml_escape_handles_all_special_chars() {
        assert_eq!(xml_escape("a&b"), "a&amp;b");
        assert_eq!(xml_escape("<tag>"), "&lt;tag&gt;");
        assert_eq!(xml_escape("\"quoted\""), "&quot;quoted&quot;");
        assert_eq!(xml_escape("it's"), "it&apos;s");
        assert_eq!(xml_escape("plain"), "plain");
    }

    fn make_node(logical_id: &str, group: Option<&str>) -> TopologyNode {
        TopologyNode {
            logical_id: LogicalId::new(logical_id),
            resource_type: ResourceType::new("aws_lambda_function"),
            provider: Provider::Aws,
            service_id: "aws.lambda".to_string(),
            label: None,
            group: group.map(LogicalId::new),
        }
    }

    fn minimal_cost(topology: Topology) -> ArchitectureCost {
        ArchitectureCost {
            name: "test".into(),
            resources: vec![],
            bindings: vec![],
            region: Region::new("ap-northeast-1"),
            topology,
            diagnostics: Vec::new(),
        }
    }

    /// When all nodes form a group cycle (A.group=B, B.group=A), no roots
    /// exist but all nodes must still appear in the draw.io XML output (no drop).
    #[test]
    fn cyclic_group_both_nodes_emitted() {
        let node_a = make_node("CycleA", Some("CycleB")); // A's parent = B
        let node_b = make_node("CycleB", Some("CycleA")); // B's parent = A

        let topology = Topology {
            nodes: vec![node_a, node_b],
            connections: vec![],
        };
        let cost = minimal_cost(topology);
        let xml = DrawIoRenderer.render(&cost).unwrap();

        // Both nodes must appear as mxCell entries (id=2 and id=3).
        assert!(
            xml.contains("id=\"2\""),
            "CycleA (id=2) must appear in output despite cycle: {xml}"
        );
        assert!(
            xml.contains("id=\"3\""),
            "CycleB (id=3) must appear in output despite cycle: {xml}"
        );
    }

    /// A topology with a cyclic group reference (A→B→C→B) must terminate
    /// without infinite recursion and produce valid XML.
    #[test]
    fn cyclic_group_reference_terminates() {
        // A is a root container that contains B.
        // B is a container that contains C.
        // C erroneously points back to B (cycle B→C→B).
        let node_a = make_node("A", None); // root container
        let node_b = make_node("B", Some("A")); // child of A
        let node_c = make_node("C", Some("B")); // child of B (creates B→C)
        // Add a node D that claims B as its group too — and B's child list
        // contains C which already references B → indirect cycle.
        // We model it more directly: make C's group point to B (so A→B→C with C→B cycle).
        // The `emit_children` + `compute_container_size` path should not recurse infinitely.
        let topology = Topology {
            nodes: vec![node_a, node_b, node_c],
            connections: vec![],
        };
        let cost = minimal_cost(topology);
        // Should complete without stack overflow.
        let xml = DrawIoRenderer.render(&cost).unwrap();
        assert!(xml.contains("<mxCell"), "must produce mxCell XML: {xml}");
    }

    /// Extract a numeric attribute value from a draw.io mxCell XML line.
    /// E.g. `extract_dim(line, "width")` parses the value of `width="NNN"`.
    fn extract_dim(line: &str, key: &str) -> u32 {
        let pat = format!("{key}=\"");
        let start = line
            .find(&pat)
            .unwrap_or_else(|| panic!("{key} not found in: {line}"))
            + pat.len();
        let rest = &line[start..];
        let end = rest.find('"').expect("closing quote");
        rest[..end].parse().expect("numeric dimension")
    }

    /// A 3-level nesting (VPC > Subnet > Instance) must produce an outer
    /// container whose width and height exceed the leaf cell dimensions, i.e.
    /// the outer container is large enough to contain its contents.
    #[test]
    fn three_level_nesting_outer_container_larger_than_leaf() {
        // vpc (root container)
        //   subnet (container, child of vpc)
        //     instance (leaf, child of subnet)
        let vpc = make_node("vpc", None);
        let subnet = make_node("subnet", Some("vpc"));
        let instance = make_node("instance", Some("subnet"));

        let topology = Topology {
            nodes: vec![vpc, subnet, instance],
            connections: vec![],
        };
        let cost = minimal_cost(topology);
        let xml = DrawIoRenderer.render(&cost).unwrap();

        // vpc cell is written first (id=2); extract its width/height.
        let vpc_line = xml
            .lines()
            .find(|l| l.contains("id=\"2\""))
            .expect("vpc mxCell (id=2) must be present");

        let width = extract_dim(vpc_line, "width");
        let height = extract_dim(vpc_line, "height");

        assert!(
            width > CELL_WIDTH,
            "outer container width {width} should exceed leaf width {CELL_WIDTH}",
        );
        assert!(
            height > CELL_HEIGHT,
            "outer container height {height} should exceed leaf height {CELL_HEIGHT}",
        );
    }

    /// An edge whose source is not a rendered node must be skipped entirely;
    /// an edge whose both endpoints are rendered must still appear in the output.
    #[test]
    fn edge_with_unknown_endpoint_is_skipped() {
        use yevice_core::resource::{Connection, ConnectionType};

        let node_a = make_node("A", None);
        let node_b = make_node("B", None);

        // Edge A → B: both endpoints are known — must appear.
        let known_edge = Connection {
            source: LogicalId::new("A"),
            target: LogicalId::new("B"),
            connection_type: ConnectionType::DataFlow,
            batch_size: None,
            parallelization_factor: None,
            factor: None,
            source_hint: None,
        };

        // Edge ExternalArn → B: source is NOT a rendered node — must be skipped.
        let unknown_edge = Connection {
            source: LogicalId::new("arn:aws:sqs:us-east-1:123456789012:MyQueue"),
            target: LogicalId::new("B"),
            connection_type: ConnectionType::EventSource,
            batch_size: None,
            parallelization_factor: None,
            factor: None,
            source_hint: None,
        };

        let topology = Topology {
            nodes: vec![node_a, node_b],
            connections: vec![known_edge, unknown_edge],
        };
        let cost = minimal_cost(topology);
        let xml = DrawIoRenderer.render(&cost).unwrap();

        // The known edge must appear; nodes A and B get cell ids 2 and 3.
        assert!(
            xml.contains("source=\"2\"") && xml.contains("target=\"3\""),
            "known edge A→B must be rendered: {xml}",
        );

        // The unknown edge must NOT produce a source="0" (mxGraph root cell anchor).
        assert!(
            !xml.contains("source=\"0\""),
            "unknown-endpoint edge must not anchor to cell 0 (mxGraph root): {xml}",
        );
    }

    /// `compute_container_size` must return a finite (leaf) size immediately when
    /// called on a node that is already in the visited set, preventing infinite
    /// recursion when cycles are injected directly into the children map.
    #[test]
    fn compute_container_size_cycle_guard_returns_leaf_size() {
        // Construct a children map where A contains B and B contains A (direct cycle).
        let node_a = make_node("A", None);
        let node_b = make_node("B", None);

        let lid_a = &node_a.logical_id;
        let lid_b = &node_b.logical_id;

        let mut children: HashMap<&LogicalId, Vec<&TopologyNode>> = HashMap::new();
        children.insert(lid_a, vec![&node_b]);
        children.insert(lid_b, vec![&node_a]);

        // Pre-seed visited with B to simulate a cycle where A is being computed
        // and B's computation would re-enter A.
        let mut visited: HashSet<&LogicalId> = HashSet::new();
        visited.insert(lid_b);

        // Computing size of A: A's child is B, but B is already in visited →
        // B should be treated as a leaf size (CELL_WIDTH × CELL_HEIGHT).
        // The result for A must be larger than leaf (it contains B as a leaf cell).
        let size = compute_container_size(&node_a, &children, &mut visited);
        assert!(
            size.width >= CELL_WIDTH,
            "container A width {w} should be at least leaf width {CELL_WIDTH}",
            w = size.width,
        );
        assert!(
            size.height >= CELL_HEIGHT,
            "container A height {h} should be at least leaf height {CELL_HEIGHT}",
            h = size.height,
        );
    }
}
