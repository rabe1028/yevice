//! Architecture diagram renderers for the yevice cost-model toolkit.
//!
//! This crate provides a [`ArchitectureRenderer`] trait and three implementations:
//!
//! - [`MermaidRenderer`] — Mermaid `flowchart LR` syntax
//! - [`DrawIoRenderer`] — draw.io / mxGraph XML
//! - [`JsonRenderer`] — JSON serialization of the topology
//!
//! All renderers consume [`yevice_core::cost::ArchitectureCost`] and read the
//! embedded [`yevice_core::topology::Topology`] to produce a diagram string.

pub mod drawio;
pub mod error;
pub mod json;
pub mod mermaid;

pub use drawio::DrawIoRenderer;
pub use error::RenderError;
pub use json::JsonRenderer;
pub use mermaid::MermaidRenderer;

use yevice_core::cost::ArchitectureCost;

/// Common interface for all architecture diagram renderers.
pub trait ArchitectureRenderer {
    /// Short format name, e.g. `"drawio"`, `"mermaid"`, `"json"`.
    fn format_name(&self) -> &'static str;

    /// Render the architecture using `cost.topology` for nodes and connections.
    fn render(&self, cost: &ArchitectureCost) -> Result<String, RenderError>;
}

#[cfg(test)]
mod tests {
    use yevice_core::cost::{ArchitectureCost, ResourceCost};
    use yevice_core::expr::Expr;
    use yevice_core::resource::{Connection, ConnectionType, Provider};
    use yevice_core::topology::{Topology, TopologyNode};
    use yevice_core::types::{ArchitectureName, LogicalId, Region, ResourceType};

    use super::*;

    /// Build a small [`ArchitectureCost`] with a mixed-provider topology
    /// for use in renderer tests.
    fn make_test_cost() -> ArchitectureCost {
        let lambda_node = TopologyNode {
            logical_id: LogicalId::new("MyFunction"),
            resource_type: ResourceType::new("AWS::Lambda::Function"),
            provider: Provider::Aws,
            service_id: "aws.lambda".to_string(),
            label: Some("My Lambda".to_string()),
            group: None,
        };
        let table_node = TopologyNode {
            logical_id: LogicalId::new("MyTable"),
            resource_type: ResourceType::new("AWS::DynamoDB::Table"),
            provider: Provider::Aws,
            service_id: "aws.dynamodb".to_string(),
            label: None,
            group: None,
        };
        let gcp_node = TopologyNode {
            logical_id: LogicalId::new("RunService"),
            resource_type: ResourceType::new("google_cloud_run_v2_service"),
            provider: Provider::Gcp,
            service_id: "gcp.cloud_run".to_string(),
            label: Some("Cloud Run".to_string()),
            group: None,
        };

        let conn1 = Connection {
            source: LogicalId::new("MyFunction"),
            target: LogicalId::new("MyTable"),
            connection_type: ConnectionType::DataFlow,
            batch_size: None,
            parallelization_factor: None,
            factor: None,
            source_hint: None,
        };
        let conn2 = Connection {
            source: LogicalId::new("RunService"),
            target: LogicalId::new("MyTable"),
            connection_type: ConnectionType::Invocation,
            batch_size: None,
            parallelization_factor: None,
            factor: None,
            source_hint: None,
        };

        let topology = Topology {
            nodes: vec![lambda_node, table_node, gcp_node],
            connections: vec![conn1, conn2],
        };

        ArchitectureCost {
            name: ArchitectureName::new("test-arch"),
            resources: vec![ResourceCost {
                logical_id: LogicalId::new("MyFunction"),
                resource_type: ResourceType::new("AWS::Lambda::Function"),
                label: "My Lambda".to_string(),
                expr: Expr::constant(0.0),
                components: vec![],
                required_variables: vec![],
            }],
            bindings: vec![],
            region: Region::new("ap-northeast-1"),
            topology,
        }
    }

    // ---- MermaidRenderer ----

    #[test]
    fn mermaid_output_contains_node_ids_and_edges() {
        let cost = make_test_cost();
        let renderer = MermaidRenderer;
        let output = renderer.render(&cost).expect("render");

        // Should start with flowchart directive
        assert!(output.starts_with("flowchart LR"), "missing flowchart LR header");

        // Node IDs must be sanitized versions of logical IDs
        assert!(output.contains("MyFunction"), "missing MyFunction node");
        assert!(output.contains("MyTable"), "missing MyTable node");
        assert!(output.contains("RunService"), "missing RunService node");

        // Edge syntax
        assert!(output.contains("-->|"), "missing edge syntax");

        // Connection type labels
        assert!(output.contains("DataFlow"), "missing DataFlow label");
        assert!(output.contains("Invocation"), "missing Invocation label");
    }

    #[test]
    fn mermaid_uses_label_when_present_and_falls_back_to_logical_id() {
        let cost = make_test_cost();
        let output = MermaidRenderer.render(&cost).expect("render");

        // MyFunction has label "My Lambda" — should appear in the quoted node string
        assert!(
            output.contains("My Lambda"),
            "label 'My Lambda' should appear in mermaid output"
        );
        // MyTable has no label — its logical_id should appear as the name part
        assert!(
            output.contains("MyTable"),
            "logical_id 'MyTable' should appear as fallback label"
        );
    }

    #[test]
    fn mermaid_includes_resource_type_in_node_label() {
        let cost = make_test_cost();
        let output = MermaidRenderer.render(&cost).expect("render");

        assert!(
            output.contains("AWS::Lambda::Function"),
            "resource type should appear in mermaid node label"
        );
    }

    // ---- DrawIoRenderer ----

    #[test]
    fn drawio_output_contains_mxgraphmodel_wrapper() {
        let cost = make_test_cost();
        let output = DrawIoRenderer.render(&cost).expect("render");

        assert!(
            output.contains("<mxGraphModel>"),
            "missing <mxGraphModel> wrapper"
        );
        assert!(output.contains("</root>"), "missing </root>");
        assert!(output.contains("</mxGraphModel>"), "missing </mxGraphModel>");
    }

    #[test]
    fn drawio_has_reserved_cells_0_and_1() {
        let cost = make_test_cost();
        let output = DrawIoRenderer.render(&cost).expect("render");

        assert!(
            output.contains("id=\"0\""),
            "missing cell id=0"
        );
        assert!(
            output.contains("id=\"1\" parent=\"0\""),
            "missing cell id=1 with parent=0"
        );
    }

    #[test]
    fn drawio_cell_count_matches_nodes_plus_edges() {
        let cost = make_test_cost();
        let output = DrawIoRenderer.render(&cost).expect("render");

        // 3 nodes + 2 edges + 2 reserved = 7 total <mxCell occurrences
        let cell_count = output.matches("<mxCell").count();
        assert_eq!(cell_count, 7, "expected 7 mxCell elements (2 reserved + 3 nodes + 2 edges)");
    }

    #[test]
    fn drawio_xml_escapes_special_characters() {
        let mut cost = make_test_cost();
        // Inject a label with XML special characters
        cost.topology.nodes[0].label = Some("A & B <test>".to_string());

        let output = DrawIoRenderer.render(&cost).expect("render");

        assert!(
            output.contains("A &amp; B &lt;test&gt;"),
            "XML special chars must be escaped; output was: {output}"
        );
        assert!(
            !output.contains("A & B"),
            "raw '&' must not appear in XML output"
        );
    }

    #[test]
    fn drawio_edge_cells_reference_correct_source_and_target() {
        let cost = make_test_cost();
        let output = DrawIoRenderer.render(&cost).expect("render");

        // Edges must have source and target attributes
        assert!(output.contains("source="), "edge must have source attr");
        assert!(output.contains("target="), "edge must have target attr");

        // Connection type labels must be present
        assert!(output.contains("DataFlow"), "missing DataFlow edge label");
        assert!(output.contains("Invocation"), "missing Invocation edge label");
    }

    // ---- JsonRenderer ----

    #[test]
    fn json_output_round_trips_topology() {
        use yevice_core::topology::Topology;

        let cost = make_test_cost();
        let original_topology = cost.topology.clone();
        let output = JsonRenderer.render(&cost).expect("render");

        let parsed: Topology =
            serde_json::from_str(&output).expect("json output must parse back into Topology");

        assert_eq!(
            parsed, original_topology,
            "topology must round-trip through JSON"
        );
    }

    #[test]
    fn json_output_is_valid_json() {
        let cost = make_test_cost();
        let output = JsonRenderer.render(&cost).expect("render");
        let _: serde_json::Value =
            serde_json::from_str(&output).expect("json renderer must produce valid JSON");
    }

    // ---- format_name ----

    #[test]
    fn format_names_are_correct() {
        assert_eq!(MermaidRenderer.format_name(), "mermaid");
        assert_eq!(DrawIoRenderer.format_name(), "drawio");
        assert_eq!(JsonRenderer.format_name(), "json");
    }

    // ---- Group / containment rendering ----

    /// Build an [`ArchitectureCost`] where one node is grouped under another.
    fn make_grouped_cost() -> ArchitectureCost {
        let vpc_node = TopologyNode {
            logical_id: LogicalId::new("MyVpc"),
            resource_type: ResourceType::new("AWS::EC2::VPC"),
            provider: Provider::Aws,
            service_id: "other".to_string(),
            label: None,
            group: None,
        };
        let subnet_node = TopologyNode {
            logical_id: LogicalId::new("MySubnet"),
            resource_type: ResourceType::new("AWS::EC2::Subnet"),
            provider: Provider::Aws,
            service_id: "other".to_string(),
            label: None,
            group: Some(LogicalId::new("MyVpc")),
        };

        let topology = Topology {
            nodes: vec![vpc_node, subnet_node],
            connections: vec![],
        };

        ArchitectureCost {
            name: ArchitectureName::new("grouped-arch"),
            resources: vec![],
            bindings: vec![],
            region: Region::new("ap-northeast-1"),
            topology,
        }
    }

    #[test]
    fn mermaid_grouped_node_produces_subgraph() {
        let cost = make_grouped_cost();
        let output = MermaidRenderer.render(&cost).expect("render");

        // A subgraph block keyed by the group logical ID must appear.
        assert!(
            output.contains("subgraph MyVpc"),
            "mermaid output must contain 'subgraph MyVpc'; got:\n{output}"
        );
        // The grouped node must appear inside the subgraph block.
        assert!(
            output.contains("MySubnet"),
            "mermaid output must contain MySubnet node; got:\n{output}"
        );
        // The subgraph must be closed.
        assert!(
            output.contains("    end"),
            "mermaid output must contain 'end' to close the subgraph; got:\n{output}"
        );
    }

    #[test]
    fn drawio_grouped_node_produces_swimlane_container() {
        let cost = make_grouped_cost();
        let output = DrawIoRenderer.render(&cost).expect("render");

        // The container cell must use swimlane style.
        assert!(
            output.contains("swimlane"),
            "drawio output must contain a swimlane container; got:\n{output}"
        );
        // The grouped node must be parented to the container (not to cell 1).
        // The container cell id is 2 (first assigned after reserved 0,1).
        assert!(
            output.contains("parent=\"2\""),
            "grouped node must have parent=\"2\" (the container cell); got:\n{output}"
        );
    }

    #[test]
    fn drawio_cell_count_with_group_matches_expected() {
        let cost = make_grouped_cost();
        let output = DrawIoRenderer.render(&cost).expect("render");

        // 2 reserved + 1 container node (MyVpc as swimlane) + 1 child node (MySubnet) + 0 edges = 4.
        // In the new design the container node itself is the swimlane cell — no separate
        // phantom container cell is created, so the total is one less than in the old design.
        let cell_count = output.matches("<mxCell").count();
        assert_eq!(
            cell_count, 4,
            "expected 4 mxCell elements (2 reserved + 1 container/swimlane + 1 child); got {cell_count}"
        );
    }

    // ---- 3-level nesting tests ----

    /// Build a 3-level nested topology: VPC (root) > Subnet (child of VPC) > Instance (child of Subnet).
    fn make_three_level_cost() -> ArchitectureCost {
        let vpc_node = TopologyNode {
            logical_id: LogicalId::new("MyVpc"),
            resource_type: ResourceType::new("AWS::EC2::VPC"),
            provider: Provider::Aws,
            service_id: "other".to_string(),
            label: Some("My VPC".to_string()),
            group: None,
        };
        let subnet_node = TopologyNode {
            logical_id: LogicalId::new("MySubnet"),
            resource_type: ResourceType::new("AWS::EC2::Subnet"),
            provider: Provider::Aws,
            service_id: "other".to_string(),
            label: Some("My Subnet".to_string()),
            group: Some(LogicalId::new("MyVpc")),
        };
        let instance_node = TopologyNode {
            logical_id: LogicalId::new("MyInstance"),
            resource_type: ResourceType::new("AWS::EC2::Instance"),
            provider: Provider::Aws,
            service_id: "other".to_string(),
            label: Some("My Instance".to_string()),
            group: Some(LogicalId::new("MySubnet")),
        };

        let topology = Topology {
            nodes: vec![vpc_node, subnet_node, instance_node],
            connections: vec![],
        };

        ArchitectureCost {
            name: ArchitectureName::new("three-level-arch"),
            resources: vec![],
            bindings: vec![],
            region: Region::new("ap-northeast-1"),
            topology,
        }
    }

    #[test]
    fn mermaid_three_level_nesting_produces_nested_subgraphs() {
        let cost = make_three_level_cost();
        let output = MermaidRenderer.render(&cost).expect("render");

        // All three nodes must appear exactly once each.
        assert_eq!(
            output.matches("MyVpc").count(),
            1,
            "MyVpc must appear exactly once; got:\n{output}"
        );
        assert_eq!(
            output.matches("MySubnet").count(),
            1,
            "MySubnet must appear exactly once; got:\n{output}"
        );
        assert_eq!(
            output.matches("MyInstance").count(),
            1,
            "MyInstance must appear exactly once; got:\n{output}"
        );

        // Subgraph headers must use the node's own label.
        assert!(
            output.contains(r#"subgraph MyVpc["My VPC (AWS::EC2::VPC)"]"#),
            "VPC container title must use node label; got:\n{output}"
        );
        assert!(
            output.contains(r#"subgraph MySubnet["My Subnet (AWS::EC2::Subnet)"]"#),
            "Subnet container title must use node label; got:\n{output}"
        );

        // Instance must appear as a plain leaf node inside the nested subgraph.
        assert!(
            output.contains(r#"MyInstance["My Instance (AWS::EC2::Instance)"]"#),
            "Instance must appear as a leaf node; got:\n{output}"
        );

        // The Subnet subgraph must be nested inside the VPC subgraph.
        // Check positional ordering: VPC subgraph opens before Subnet subgraph,
        // and the Subnet `end` comes before the VPC `end`.
        let vpc_open = output
            .find("subgraph MyVpc")
            .expect("subgraph MyVpc must be present");
        let subnet_open = output
            .find("subgraph MySubnet")
            .expect("subgraph MySubnet must be present");
        let instance_pos = output
            .find("MyInstance[")
            .expect("MyInstance leaf must be present");

        // Find the two `end` keywords (in reverse position order).
        // The innermost `end` closes MySubnet; the outer closes MyVpc.
        let mut end_positions: Vec<usize> = output.match_indices("end").map(|(i, _)| i).collect();
        end_positions.sort_unstable();
        assert!(
            end_positions.len() >= 2,
            "at least two `end` tokens required for nested subgraphs; got:\n{output}"
        );
        let first_end = end_positions[0];
        let second_end = end_positions[1];

        // Structure: VPC_open < Subnet_open < Instance < first_end (closes Subnet) < second_end (closes VPC).
        assert!(vpc_open < subnet_open, "VPC subgraph must open before Subnet subgraph");
        assert!(subnet_open < instance_pos, "Subnet subgraph must open before Instance leaf");
        assert!(instance_pos < first_end, "Instance must appear before first `end`");
        assert!(first_end < second_end, "first `end` must precede second `end`");
    }

    #[test]
    fn drawio_three_level_nesting_produces_correct_parent_chain() {
        let cost = make_three_level_cost();
        let output = DrawIoRenderer.render(&cost).expect("render");

        // Node cell IDs: MyVpc=2, MySubnet=3, MyInstance=4 (topology.nodes Vec order).
        // MyVpc is the root container → parent="1".
        // MySubnet is a container inside MyVpc → parent="2".
        // MyInstance is a leaf inside MySubnet → parent="3".

        // MyVpc container: parent="1" and swimlane style.
        assert!(
            output.contains("id=\"2\"") && output.contains("swimlane"),
            "MyVpc (id=2) must be a swimlane; got:\n{output}"
        );
        assert!(
            output.contains("id=\"2\""),
            "cell id 2 (MyVpc) must appear; got:\n{output}"
        );

        // Verify parent chain via substring matching in the relevant cells.
        // MyVpc → parent="1"
        assert!(
            output.contains("id=\"2\"") && output.contains("parent=\"1\""),
            "MyVpc must have parent=1 somewhere in output; got:\n{output}"
        );
        // MySubnet → parent="2"
        assert!(
            output.contains("id=\"3\"") && output.contains("parent=\"2\""),
            "MySubnet must have parent=2; got:\n{output}"
        );
        // MyInstance → parent="3"
        assert!(
            output.contains("id=\"4\"") && output.contains("parent=\"3\""),
            "MyInstance must have parent=3; got:\n{output}"
        );

        // Well-formed XML: must open and close mxGraphModel/root.
        assert!(output.contains("<mxGraphModel><root>"), "must open mxGraphModel");
        assert!(output.contains("</root></mxGraphModel>"), "must close mxGraphModel");

        // Cell count: 2 reserved + 3 nodes + 0 edges = 5.
        let cell_count = output.matches("<mxCell").count();
        assert_eq!(
            cell_count, 5,
            "expected 5 mxCell elements (2 reserved + 3 nodes); got {cell_count}"
        );
    }

    // ---- Dangling parent test ----

    /// A node whose `group` points to a non-existent ID is treated as a root.
    #[test]
    fn mermaid_dangling_parent_treated_as_root() {
        let orphan_node = TopologyNode {
            logical_id: LogicalId::new("Orphan"),
            resource_type: ResourceType::new("AWS::EC2::Instance"),
            provider: Provider::Aws,
            service_id: "other".to_string(),
            label: None,
            group: Some(LogicalId::new("NonExistentVpc")),
        };

        let topology = Topology {
            nodes: vec![orphan_node],
            connections: vec![],
        };

        let cost = ArchitectureCost {
            name: ArchitectureName::new("dangling-arch"),
            resources: vec![],
            bindings: vec![],
            region: Region::new("ap-northeast-1"),
            topology,
        };

        let output = MermaidRenderer.render(&cost).expect("render");

        // Orphan must appear exactly once as a top-level leaf node definition.
        assert!(
            output.contains("Orphan"),
            "Orphan node must appear in output; got:\n{output}"
        );
        // No subgraph should be created for the dangling parent.
        assert!(
            !output.contains("subgraph"),
            "no subgraph expected for dangling parent; got:\n{output}"
        );
        // The leaf node definition `Orphan[` must appear exactly once.
        assert_eq!(
            output.matches("Orphan[").count(),
            1,
            "Orphan node definition must appear exactly once; got:\n{output}"
        );
    }

    #[test]
    fn drawio_dangling_parent_treated_as_root() {
        let orphan_node = TopologyNode {
            logical_id: LogicalId::new("Orphan"),
            resource_type: ResourceType::new("AWS::EC2::Instance"),
            provider: Provider::Aws,
            service_id: "other".to_string(),
            label: None,
            group: Some(LogicalId::new("NonExistentVpc")),
        };

        let topology = Topology {
            nodes: vec![orphan_node],
            connections: vec![],
        };

        let cost = ArchitectureCost {
            name: ArchitectureName::new("dangling-arch"),
            resources: vec![],
            bindings: vec![],
            region: Region::new("ap-northeast-1"),
            topology,
        };

        let output = DrawIoRenderer.render(&cost).expect("render");

        // Orphan must appear as a regular node parented to cell 1 (top-level).
        assert!(
            output.contains("parent=\"1\"") && output.contains("id=\"2\""),
            "Orphan must be a top-level cell (parent=1); got:\n{output}"
        );
        // No swimlane should be created for the dangling parent.
        assert!(
            !output.contains("swimlane"),
            "no swimlane expected for dangling parent; got:\n{output}"
        );
        // Cell count: 2 reserved + 1 node + 0 edges = 3.
        let cell_count = output.matches("<mxCell").count();
        assert_eq!(
            cell_count, 3,
            "expected 3 mxCell elements (2 reserved + 1 orphan node); got {cell_count}"
        );
    }
}
