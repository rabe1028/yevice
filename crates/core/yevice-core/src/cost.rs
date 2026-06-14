//! Cost model types built on top of [`Expr`].

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use crate::expr::{Expr, Tier};
use crate::parse_policy::IacParseDiagnostic;
use crate::topology::Topology;
use crate::types::{ArchitectureName, LogicalId, Region, ResourceType, VariableName};

/// A named sub-component of a resource's cost.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostComponent {
    /// Human-readable name (e.g., "Compute (Fargate)", "Storage (EBS gp3)").
    pub name: String,
    pub expr: Expr,
    /// Currency override for this component. When `None`, the parent
    /// `ResourceCost.currency` (or fallback to `"USD"`) is used. See ADR-0001
    /// for the priority resolution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
}

impl CostComponent {
    /// Construct a component with no currency override (inherits from parent).
    pub fn new(name: impl Into<String>, expr: Expr) -> Self {
        Self {
            name: name.into(),
            expr,
            currency: None,
        }
    }

    /// Construct a component with an explicit currency override.
    pub fn with_currency(name: impl Into<String>, expr: Expr, currency: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            expr,
            currency: Some(currency.into()),
        }
    }
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
    /// Resource-level default currency. `None` triggers the USD fallback at
    /// evaluation time (with a `tracing::warn!`). See ADR-0001 schema-side
    /// "案 (b+)" — two-tier currency persistence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
}

impl ResourceCost {
    /// Construct a `ResourceCost`, validating that all currency overrides
    /// agree (per ADR-0001 §"ResourceCost 構築時の Validation 契約").
    ///
    /// Errors with [`CostBuildError::ComponentCurrencyMismatch`] when any
    /// component carries a `Some(currency)` that disagrees with another
    /// component's `Some(currency)` or with the resource-level `currency`.
    /// All-None or unanimous-Some(same) is accepted.
    pub fn new(
        logical_id: LogicalId,
        resource_type: ResourceType,
        label: impl Into<String>,
        expr: Expr,
        components: Vec<CostComponent>,
        required_variables: Vec<VariableInfo>,
        currency: Option<String>,
    ) -> Result<Self, CostBuildError> {
        validate_currency(&logical_id, currency.as_deref(), &components)?;
        Ok(Self {
            logical_id,
            resource_type,
            label: label.into(),
            expr,
            components,
            required_variables,
            currency,
        })
    }

    /// Re-run the currency-mismatch validation after `serde` deserialization.
    ///
    /// `Deserialize` cannot run custom logic on a struct literal, so callers
    /// loading `cost_model.json` should invoke this on the freshly-decoded
    /// `ArchitectureCost.resources` to catch hand-written corrupt files.
    pub fn validate(&self) -> Result<(), CostBuildError> {
        validate_currency(&self.logical_id, self.currency.as_deref(), &self.components)
    }
}

fn validate_currency(
    logical_id: &LogicalId,
    resource_currency: Option<&str>,
    components: &[CostComponent],
) -> Result<(), CostBuildError> {
    use std::collections::BTreeSet;

    let mut seen: BTreeSet<String> = BTreeSet::new();
    if let Some(rc) = resource_currency {
        seen.insert(rc.to_string());
    }
    for c in components {
        if let Some(cc) = &c.currency {
            seen.insert(cc.clone());
        }
    }
    if seen.len() > 1 {
        return Err(CostBuildError::ComponentCurrencyMismatch {
            resource_id: logical_id.to_string(),
            currencies: seen.into_iter().collect(),
        });
    }
    Ok(())
}

/// Errors raised while assembling a [`ResourceCost`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CostBuildError {
    #[error("resource {resource_id} has inconsistent component currencies: {currencies:?}")]
    ComponentCurrencyMismatch {
        resource_id: String,
        currencies: Vec<String>,
    },
}

/// Indicates whether a variable is a usage (observed) input or a decision
/// variable that the optimizer may choose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum VariableKind {
    /// Observed usage input supplied by the user (default).
    #[default]
    Usage,
    /// Decision variable: the optimizer selects its value from a domain.
    Decision,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_usage(k: &VariableKind) -> bool {
    *k == VariableKind::Usage
}

/// Metadata about a variable used in a cost expression.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariableInfo {
    pub name: VariableName,
    pub description: String,
    pub unit: String,
    /// Whether this variable is a usage input or a decision variable.
    #[serde(default, skip_serializing_if = "is_usage")]
    pub kind: VariableKind,
}

impl VariableInfo {
    pub fn new(id: &LogicalId, suffix: &str, description: &str, unit: &str) -> Self {
        Self {
            name: id.var(suffix),
            description: description.into(),
            unit: unit.into(),
            kind: VariableKind::Usage,
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
    /// Architecture topology (all nodes + connections), persisted so diagram
    /// and optimization consumers need not re-parse the source IaC.
    #[serde(default)]
    pub topology: Topology,
    /// IaC parse diagnostics collected while building this cost model.
    ///
    /// Always emitted in JSON (even when empty) per ADR-0003 so downstream
    /// consumers can distinguish "no diagnostics emitted" from "field missing
    /// because the producer predates the schema bump".
    #[serde(default)]
    pub diagnostics: Vec<IacParseDiagnostic>,
}

impl ArchitectureCost {
    /// Validate currency consistency for all [`ResourceCost`] entries.
    ///
    /// Iterates every resource and calls [`ResourceCost::validate`]. Returns
    /// the first [`CostBuildError`] encountered, or `Ok(())` when all resources
    /// pass. Call this after `serde` deserialization or any path that bypasses
    /// [`ResourceCost::new`] to ensure invariants hold at evaluation boundaries.
    pub fn validate(&self) -> Result<(), CostBuildError> {
        for rc in &self.resources {
            rc.validate()?;
        }
        Ok(())
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn architecture_cost_without_topology_field_deserializes() {
        // JSON that was produced before the `topology` field existed.
        // Deserializing it must succeed and yield an empty (default) topology.
        let json = r#"{
            "name": "arch",
            "resources": [],
            "bindings": [],
            "region": "ap-northeast-1"
        }"#;

        let cost: ArchitectureCost = serde_json::from_str(json).expect("deserialize");
        assert_eq!(cost.name, ArchitectureName::new("arch"));
        assert!(cost.topology.is_empty());
    }

    #[test]
    fn resource_cost_new_accepts_unanimous_currencies() {
        let rc = ResourceCost::new(
            LogicalId::new("R"),
            ResourceType::new("AWS::Foo::Bar"),
            "label",
            Expr::constant(0.0),
            vec![
                CostComponent::with_currency("a", Expr::constant(0.0), "USD"),
                CostComponent::with_currency("b", Expr::constant(0.0), "USD"),
            ],
            vec![],
            Some("USD".into()),
        );
        assert!(rc.is_ok());
    }

    #[test]
    fn resource_cost_new_rejects_mismatched_currencies() {
        let err = ResourceCost::new(
            LogicalId::new("R"),
            ResourceType::new("AWS::Foo::Bar"),
            "label",
            Expr::constant(0.0),
            vec![
                CostComponent::with_currency("a", Expr::constant(0.0), "USD"),
                CostComponent::with_currency("b", Expr::constant(0.0), "JPY"),
            ],
            vec![],
            Some("USD".into()),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            CostBuildError::ComponentCurrencyMismatch { .. }
        ));
    }

    #[test]
    fn resource_cost_validate_catches_corrupt_deserialized_struct() {
        // Directly build the struct (bypassing `new`) to simulate
        // deserialization of a hand-edited cost_model.json.
        let rc = ResourceCost {
            logical_id: LogicalId::new("R"),
            resource_type: ResourceType::new("AWS::Foo::Bar"),
            label: "x".into(),
            expr: Expr::constant(0.0),
            components: vec![
                CostComponent::with_currency("a", Expr::constant(0.0), "USD"),
                CostComponent::with_currency("b", Expr::constant(0.0), "EUR"),
            ],
            required_variables: vec![],
            currency: Some("USD".into()),
        };
        let err = rc.validate().unwrap_err();
        assert!(matches!(
            err,
            CostBuildError::ComponentCurrencyMismatch { .. }
        ));
    }
}
