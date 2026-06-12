//! Generic expression AST for numeric computations.
//!
//! Used for both cost calculations and capacity requirement derivations.

use serde::{Deserialize, Serialize};

use crate::types::VariableName;

/// A single tier in a tiered pricing model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Tier {
    /// Upper limit of this tier (exclusive). `None` means no limit (final tier).
    pub upper_limit: Option<f64>,
    /// Price per unit in this tier.
    pub unit_price: f64,
}

/// Expression AST node.
///
/// A composable numeric expression evaluated with variable bindings.
/// Used for cost calculations, capacity requirement derivations, and
/// constraint definitions.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum Expr {
    /// A constant value.
    Constant { value: f64 },

    /// A named variable to be supplied at evaluation time.
    Variable { name: VariableName },

    /// Linear: `coeff * var + offset`.
    Linear {
        coeff: f64,
        var: Box<Expr>,
        offset: f64,
    },

    /// Tiered (piecewise linear): each tier has an upper limit and per-unit price.
    Tiered { tiers: Vec<Tier>, var: Box<Expr> },

    /// Sum of expressions.
    Sum { exprs: Vec<Expr> },

    /// Product of expressions.
    Product { exprs: Vec<Expr> },

    /// Maximum of expression and a floor value.
    Max { expr: Box<Expr>, floor: f64 },

    /// Minimum of expression and a ceiling value.
    Min { expr: Box<Expr>, ceiling: f64 },

    /// Ceiling (round up).
    Ceil { expr: Box<Expr> },

    /// Division: `numerator / denominator`.
    Div {
        numerator: Box<Expr>,
        denominator: Box<Expr>,
    },
}

impl Expr {
    pub fn constant(value: f64) -> Self {
        Self::Constant { value }
    }

    pub fn variable(name: impl Into<VariableName>) -> Self {
        Self::Variable { name: name.into() }
    }

    pub fn linear(coeff: f64, var: Self, offset: f64) -> Self {
        Self::Linear {
            coeff,
            var: Box::new(var),
            offset,
        }
    }

    pub fn tiered(tiers: Vec<Tier>, var: Self) -> Self {
        Self::Tiered {
            tiers,
            var: Box::new(var),
        }
    }

    pub fn sum(exprs: Vec<Self>) -> Self {
        Self::Sum { exprs }
    }

    pub fn product(exprs: Vec<Self>) -> Self {
        Self::Product { exprs }
    }

    pub fn ceil(expr: Self) -> Self {
        Self::Ceil {
            expr: Box::new(expr),
        }
    }

    pub fn div(numerator: Self, denominator: Self) -> Self {
        Self::Div {
            numerator: Box::new(numerator),
            denominator: Box::new(denominator),
        }
    }
}
