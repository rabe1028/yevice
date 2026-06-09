//! Output-agnostic, provider-agnostic architecture topology.
//!
//! A self-contained node + edge graph extracted from an `Architecture`.
//! Persisted alongside the cost model so downstream consumers (diagram
//! emitters, optimizers) need not re-parse the original IaC.

use serde::{Deserialize, Serialize};

use crate::resource::{Connection, Provider};
use crate::types::{LogicalId, ResourceType};

/// A node in the architecture topology graph.
///
/// Carries the minimum provider-agnostic metadata needed to render or
/// reason about a resource, independent of whether it has a cost model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TopologyNode {
    pub logical_id: LogicalId,
    pub resource_type: ResourceType,
    pub provider: Provider,
    /// Service identifier, e.g. `"aws.lambda"`, `"gcp.cloud_run"`, `"other"`.
    pub service_id: String,
    /// Human-readable label (resource name), if known. Populated by parsers later.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Containment parent (VPC / subnet / cluster), if any. `None` until parsers capture grouping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<LogicalId>,
}

/// The complete architecture topology: every resource as a node plus the
/// connection graph. Unlike `ArchitectureCost::resources`, this includes
/// non-costed / unsupported nodes so edges never dangle.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Topology {
    pub nodes: Vec<TopologyNode>,
    pub connections: Vec<Connection>,
}

impl Topology {
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.connections.is_empty()
    }

    pub fn find_node(&self, id: &LogicalId) -> Option<&TopologyNode> {
        self.nodes.iter().find(|n| &n.logical_id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::{Architecture, ConnectionType, Resource, ResourceShell};
    use crate::types::Region;

    fn make_resource(logical_id: &str, resource_type: &str, shell: ResourceShell) -> Resource {
        Resource {
            logical_id: LogicalId::new(logical_id),
            resource_type: ResourceType::new(resource_type),
            shell,
            group: None,
        }
    }

    #[test]
    fn architecture_topology_includes_all_nodes_and_edges() {
        let lambda_shell = ResourceShell::new("aws.lambda", Provider::Aws, &serde_json::json!({}));
        let dynamo_shell =
            ResourceShell::new("aws.dynamodb", Provider::Aws, &serde_json::json!({}));
        let other_shell = ResourceShell::other("Custom::MyResource");

        let lambda = make_resource("MyFunction", "AWS::Lambda::Function", lambda_shell);
        let dynamo = make_resource("MyTable", "AWS::DynamoDB::Table", dynamo_shell);
        let other = make_resource("MyCustom", "Custom::MyResource", other_shell);

        let connection = Connection {
            source: LogicalId::new("MyFunction"),
            target: LogicalId::new("MyTable"),
            connection_type: ConnectionType::DataFlow,
            batch_size: None,
            parallelization_factor: None,
            factor: None,
            source_hint: None,
        };

        let arch = Architecture {
            name: "test-arch".to_string(),
            region: Region::new("ap-northeast-1"),
            resources: vec![lambda, dynamo, other],
            connections: vec![connection.clone()],
        };

        let topology = arch.topology();

        // All 3 nodes must be present, including the "other" resource
        assert_eq!(topology.nodes.len(), 3);
        // The single connection must be present
        assert_eq!(topology.connections.len(), 1);

        let lambda_node = topology
            .find_node(&LogicalId::new("MyFunction"))
            .expect("MyFunction node");
        assert_eq!(lambda_node.provider, Provider::Aws);
        assert_eq!(lambda_node.service_id, "aws.lambda");

        let other_node = topology
            .find_node(&LogicalId::new("MyCustom"))
            .expect("MyCustom node");
        assert_eq!(other_node.provider, Provider::Other);
        assert_eq!(other_node.service_id, "other");

        assert_eq!(topology.connections[0].source, LogicalId::new("MyFunction"));
        assert_eq!(topology.connections[0].target, LogicalId::new("MyTable"));
    }

    #[test]
    fn topology_serde_roundtrip() {
        let node = TopologyNode {
            logical_id: LogicalId::new("MyFunction"),
            resource_type: ResourceType::new("AWS::Lambda::Function"),
            provider: Provider::Aws,
            service_id: "aws.lambda".to_string(),
            label: Some("My Lambda".to_string()),
            group: None,
        };
        let connection = Connection {
            source: LogicalId::new("MyFunction"),
            target: LogicalId::new("MyTable"),
            connection_type: ConnectionType::DataFlow,
            batch_size: None,
            parallelization_factor: None,
            factor: None,
            source_hint: None,
        };
        let topology = Topology {
            nodes: vec![node],
            connections: vec![connection],
        };

        let json = serde_json::to_string(&topology).expect("serialize");
        let roundtripped: Topology = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(topology, roundtripped);
    }
}
