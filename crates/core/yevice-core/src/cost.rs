//! Cost model types built on top of [`Expr`].

use serde::{Deserialize, Serialize};

pub use crate::expr::{Expr, Tier};
use crate::types::{ArchitectureName, LogicalId, Region, ResourceType, VariableName};

/// A named sub-component of a resource's cost.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostComponent {
    /// Human-readable name (e.g., "Compute (Fargate)", "Storage (EBS gp3)").
    pub name: String,
    pub expr: Expr,
}

/// Cost model for a single resource.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceCost {
    pub logical_id: LogicalId,
    pub resource_type: ResourceType,
    pub label: String,
    pub expr: Expr,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<CostComponent>,
    pub required_variables: Vec<VariableInfo>,
}

/// Metadata about a variable used in a cost expression.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariableInfo {
    pub name: VariableName,
    pub description: String,
    pub unit: String,
}

impl VariableInfo {
    pub fn new(id: &LogicalId, suffix: &str, description: &str, unit: &str) -> Self {
        Self {
            name: id.var(suffix),
            description: description.into(),
            unit: unit.into(),
        }
    }
}

/// A derived variable binding.
///
/// Expresses that a variable's value can be computed from other variables.
/// Users can override a bound variable by providing an explicit value in usage params.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariableBinding {
    /// The variable being derived.
    pub target: VariableName,
    /// The expression to compute the derived value.
    pub expr: Expr,
    /// Human-readable description of the relationship.
    pub description: String,
    /// Source label (e.g., "SQS -> Lambda", "user-defined").
    pub source: String,
}

/// Top-level cost model for an entire architecture.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchitectureCost {
    /// Name or identifier for this architecture.
    pub name: ArchitectureName,
    /// Individual resource costs.
    pub resources: Vec<ResourceCost>,
    /// Derived variable bindings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<VariableBinding>,
    /// Region.
    pub region: Region,
}

impl ArchitectureCost {
    /// Returns the total cost expression (sum of all resource costs).
    pub fn total_expr(&self) -> Expr {
        Expr::sum(self.resources.iter().map(|r| r.expr.clone()).collect())
    }

    /// Collects all required variables across all resources,
    /// excluding those that have bindings.
    pub fn all_variables(&self) -> Vec<&VariableInfo> {
        let bound_names: std::collections::HashSet<&VariableName> =
            self.bindings.iter().map(|b| &b.target).collect();
        self.resources
            .iter()
            .flat_map(|r| r.required_variables.iter())
            .filter(|v| !bound_names.contains(&v.name))
            .collect()
    }

    /// Collects all variable bindings.
    pub fn all_bindings(&self) -> &[VariableBinding] {
        &self.bindings
    }
}
