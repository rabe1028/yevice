//! Pre-checks the MILP backend runs before encoding.
//!
//! These checks turn would-be encoder panics or silent wrong answers into
//! actionable errors before a single backend call is made.
//!
//! 1. `expr_is_linearizable` on the (bindings-expanded) objective and each
//!    constraint LHS — catches `var * var`, `var / var`, etc.
//! 2. `classify_ceil_context(problem)` — ADR-0002 Ceil safety classifier.
//! 3. `expr_bounds(..)` on every sub-expression that needs a big-M, surfacing
//!    `SolverError::UnboundedExpression` if no finite bound can be derived.

use std::collections::BTreeSet;

use yevice_core::expr::Expr;
use yevice_core::expr_introspect::{
    CeilContextResult, VarRanges, classify_ceil_context, expr_bounds, expr_is_linearizable,
    substitute_bindings,
};
use yevice_core::optimize::OptimizationProblem;
use yevice_core::types::VariableName;

use crate::error::SolverError;

pub(crate) fn pre_check(problem: &OptimizationProblem) -> Result<(), SolverError> {
    // Decision-variable name set (used by linearizability checks).
    let decision_vars: BTreeSet<VariableName> = problem
        .decision_variables
        .iter()
        .map(|dv| dv.name.clone())
        .collect();

    // 1. Linearizability of objective + every constraint LHS (expanded).
    let objective_expanded = substitute_bindings(&problem.objective, &problem.bindings);
    if !expr_is_linearizable(&objective_expanded, &decision_vars) {
        return Err(SolverError::Nonlinear {
            expr: format!("{objective_expanded:?}"),
        });
    }
    for c in &problem.constraints {
        let lhs = substitute_bindings(&c.lhs, &problem.bindings);
        if !expr_is_linearizable(&lhs, &decision_vars) {
            return Err(SolverError::Nonlinear {
                expr: format!("{lhs:?}"),
            });
        }
    }

    // 2. Ceil context safety.
    match classify_ceil_context(problem) {
        CeilContextResult::Ok => {}
        CeilContextResult::Reject { expr_repr, reason } => {
            return Err(SolverError::UnsupportedCeilContext {
                expr: expr_repr,
                reason,
            });
        }
    }

    // 3. Walk the (expanded) objective + constraints and verify every Max/Min
    //    sub-expression has finite big-M bounds. Tiered with unbounded usage
    //    is also caught here.
    let ranges = build_ranges(problem);
    check_big_m_bounds(&objective_expanded, &ranges)?;
    for c in &problem.constraints {
        let lhs = substitute_bindings(&c.lhs, &problem.bindings);
        check_big_m_bounds(&lhs, &ranges)?;
    }

    Ok(())
}

fn build_ranges(problem: &OptimizationProblem) -> VarRanges {
    let mut ranges = VarRanges::default();
    for dv in &problem.decision_variables {
        if dv.domain.is_empty() {
            continue;
        }
        let lo = dv.domain.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = dv.domain.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        ranges.decision_var_ranges.insert(dv.name.clone(), (lo, hi));
    }
    for (name, &v) in &problem.fixed_params {
        ranges.fixed_params.insert(name.clone(), v);
    }
    ranges
}

fn check_big_m_bounds(expr: &Expr, ranges: &VarRanges) -> Result<(), SolverError> {
    match expr {
        Expr::Max { expr: inner, .. }
        | Expr::Min { expr: inner, .. }
        | Expr::Tiered { var: inner, .. } => {
            let b = expr_bounds(inner, ranges);
            if !b.is_finite() {
                return Err(SolverError::UnboundedExpression {
                    expr: format!("{inner:?}"),
                });
            }
            check_big_m_bounds(inner, ranges)?;
        }
        Expr::Linear { var, .. } | Expr::Ceil { expr: var } => {
            check_big_m_bounds(var, ranges)?;
        }
        Expr::Sum { exprs } | Expr::Product { exprs } => {
            for e in exprs {
                check_big_m_bounds(e, ranges)?;
            }
        }
        Expr::Div {
            numerator,
            denominator,
        } => {
            check_big_m_bounds(numerator, ranges)?;
            check_big_m_bounds(denominator, ranges)?;
        }
        Expr::Constant { .. } | Expr::Variable { .. } => {}
        // Expr is #[non_exhaustive]; any future variant must be considered
        // unsupported here (and would also fail `expr_is_linearizable`).
        _ => {
            return Err(SolverError::Nonlinear {
                expr: format!("{expr:?}"),
            });
        }
    }
    Ok(())
}
