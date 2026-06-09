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

        // 2 reserved + 1 container + 2 nodes + 0 edges = 5 mxCell elements.
        let cell_count = output.matches("<mxCell").count();
        assert_eq!(
            cell_count, 5,
            "expected 5 mxCell elements (2 reserved + 1 container + 2 nodes); got {cell_count}"
        );
    }
}
