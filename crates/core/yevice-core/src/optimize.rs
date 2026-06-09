//! Optimization problem model for FinOps cost minimization.
//!
//! Defines the types needed to express a discrete optimization problem over a
//! cost expression: decision variables with finite domains, linear or nonlinear
//! constraints, and a direction (minimize / maximize).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::cost::VariableBinding;
use crate::expr::Expr;
use crate::types::VariableName;

/// Direction of the optimization objective.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ObjectiveDirection {
    /// Find the assignment that minimizes the objective (default).
    #[default]
    Minimize,
    /// Find the assignment that maximizes the objective.
    Maximize,
}

/// Comparison relation used in a constraint (`lhs <rel> rhs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Relation {
    /// Less than or equal (`≤`).
    Le,
    /// Greater than or equal (`≥`).
    Ge,
    /// Exactly equal (`=`).
    Eq,
}

/// A decision variable the optimizer may choose from a discrete domain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionVariable {
    /// Name of the variable, must match a variable referenced in the objective
    /// or constraints.
    pub name: VariableName,
    /// Finite set of candidate values. The solver will try every element.
    pub domain: Vec<f64>,
}

/// A constraint of the form `lhs <relation> rhs`.
///
/// `lhs` is an expression over decision and fixed variables; `rhs` is a
/// constant right-hand side.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationConstraint {
    /// Left-hand side expression.
    pub lhs: Expr,
    /// Comparison relation.
    pub relation: Relation,
    /// Right-hand side constant.
    pub rhs: f64,
    /// Optional human-readable label for debugging and reporting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// A fully specified discrete optimization problem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationProblem {
    /// The expression to optimize.
    pub objective: Expr,
    /// Optimization direction (minimize by default).
    #[serde(default)]
    pub direction: ObjectiveDirection,
    /// Decision variables with their candidate domains.
    pub decision_variables: Vec<DecisionVariable>,
    /// Constraints that every feasible solution must satisfy.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<OptimizationConstraint>,
    /// Fixed (non-decision) variable values, e.g. usage parameters.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub fixed_params: HashMap<VariableName, f64>,
    /// Variable bindings (derived-variable relationships) to resolve before
    /// evaluating the objective/constraints, mirroring `evaluate_architecture`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<VariableBinding>,
}
