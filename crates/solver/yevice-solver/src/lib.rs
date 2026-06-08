//! Solver backends for yevice cost-optimization problems.
//!
//! The primary entry point is the [`Solver`] trait, which takes an
//! [`OptimizationProblem`] and returns a [`Solution`].  The only solver
//! provided here is [`EnumerationSolver`], which tries every element of the
//! Cartesian product of all decision-variable domains.

pub mod error;

use std::collections::HashMap;

pub use error::SolverError;
use yevice_core::evaluate::{self, Params};
use yevice_core::optimize::{ObjectiveDirection, OptimizationProblem, Relation};
use yevice_core::types::VariableName;

/// Maximum number of combinations the [`EnumerationSolver`] will attempt.
const MAX_COMBINATIONS: u64 = 1_000_000;

/// Floating-point tolerance for constraint satisfaction checks.
const CONSTRAINT_TOLERANCE: f64 = 1e-9;

/// The result of a solve attempt.
#[derive(Debug, Clone)]
pub struct Solution {
    /// Chosen value for each decision variable in the optimal (or any feasible)
    /// assignment.  Empty when `feasible` is false.
    pub assignments: HashMap<VariableName, f64>,
    /// Objective value at the optimal assignment.  [`f64::NAN`] when infeasible.
    pub objective_value: f64,
    /// True iff at least one feasible assignment was found.
    pub feasible: bool,
}

/// Interface for optimization backends.
pub trait Solver {
    /// Solve the given problem and return the best [`Solution`] found.
    fn solve(&self, problem: &OptimizationProblem) -> Result<Solution, SolverError>;
}

// ---------------------------------------------------------------------------
// EnumerationSolver
// ---------------------------------------------------------------------------

/// A dependency-free solver that enumerates the full Cartesian product of
/// decision-variable domains.
///
/// # Complexity
///
/// Exponential in the number of decision variables and the sizes of their
/// domains.  The solver rejects problems whose combination count exceeds
/// [`MAX_COMBINATIONS`] with [`SolverError::TooManyCombinations`].
///
/// # Determinism
///
/// The enumeration order follows the order of `decision_variables` in the
/// problem, and within each variable the order of elements in `domain`.
/// When multiple assignments yield the same objective value, the first one
/// encountered (in enumeration order) is kept.
pub struct EnumerationSolver;

impl Solver for EnumerationSolver {
    fn solve(&self, problem: &OptimizationProblem) -> Result<Solution, SolverError> {
        // Guard against combinatorial explosion.
        let combination_count = combination_count(problem)?;

        if combination_count == 0 {
            // No decision variables: treat as a single empty combination.
            return solve_single(problem, HashMap::new());
        }

        let vars = &problem.decision_variables;
        // Start with a single empty partial assignment.
        let mut candidates: Vec<HashMap<VariableName, f64>> = vec![HashMap::new()];

        for dv in vars {
            let mut next = Vec::with_capacity(candidates.len() * dv.domain.len());
            for partial in candidates {
                for &val in &dv.domain {
                    let mut extended = partial.clone();
                    extended.insert(dv.name.clone(), val);
                    next.push(extended);
                }
            }
            candidates = next;
        }

        debug_assert_eq!(candidates.len() as u64, combination_count);

        let mut best: Option<Solution> = None;

        for assignment in candidates {
            let params = build_params(problem, &assignment);

            // Evaluate and check all constraints; skip this combination on any
            // evaluation error (treat as infeasible).
            if !is_feasible(problem, &params) {
                continue;
            }

            // Evaluate the objective; skip on error.
            let obj = match evaluate::evaluate(&problem.objective, &params) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let better = match &best {
                None => true,
                Some(current) => match problem.direction {
                    ObjectiveDirection::Minimize => obj < current.objective_value,
                    ObjectiveDirection::Maximize => obj > current.objective_value,
                },
            };

            if better {
                best = Some(Solution {
                    assignments: assignment,
                    objective_value: obj,
                    feasible: true,
                });
            }
        }

        Ok(best.unwrap_or(Solution {
            assignments: HashMap::new(),
            objective_value: f64::NAN,
            feasible: false,
        }))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compute the total number of combinations (product of domain sizes).
///
/// Returns [`SolverError::TooManyCombinations`] if the count exceeds
/// [`MAX_COMBINATIONS`].
fn combination_count(problem: &OptimizationProblem) -> Result<u64, SolverError> {
    let mut count: u64 = 1;
    for dv in &problem.decision_variables {
        let len = dv.domain.len() as u64;
        count = count.saturating_mul(len);
        if count > MAX_COMBINATIONS {
            return Err(SolverError::TooManyCombinations {
                count,
                limit: MAX_COMBINATIONS,
            });
        }
    }
    Ok(count)
}

/// Build an evaluation `Params` map by merging `fixed_params` with an
/// assignment of decision variables.  Decision variable values take
/// precedence over fixed params with the same name.
fn build_params(problem: &OptimizationProblem, assignment: &HashMap<VariableName, f64>) -> Params {
    let mut params: Params = problem.fixed_params.clone();
    for (name, &val) in assignment {
        params.insert(name.clone(), val);
    }
    params
}

/// Return `true` iff every constraint in the problem is satisfied by `params`.
///
/// An evaluation error on any constraint's LHS is treated as a failure (the
/// combination is skipped, not propagated as an error).
fn is_feasible(problem: &OptimizationProblem, params: &Params) -> bool {
    for constraint in &problem.constraints {
        let lhs = match evaluate::evaluate(&constraint.lhs, params) {
            Ok(v) => v,
            Err(_) => return false,
        };
        let satisfied = match constraint.relation {
            Relation::Le => lhs <= constraint.rhs + CONSTRAINT_TOLERANCE,
            Relation::Ge => lhs >= constraint.rhs - CONSTRAINT_TOLERANCE,
            Relation::Eq => (lhs - constraint.rhs).abs() <= CONSTRAINT_TOLERANCE,
        };
        if !satisfied {
            return false;
        }
    }
    true
}

/// Solve the degenerate case of zero decision variables (single combination).
fn solve_single(
    problem: &OptimizationProblem,
    assignment: HashMap<VariableName, f64>,
) -> Result<Solution, SolverError> {
    let params = build_params(problem, &assignment);
    if !is_feasible(problem, &params) {
        return Ok(Solution {
            assignments: HashMap::new(),
            objective_value: f64::NAN,
            feasible: false,
        });
    }
    let obj = evaluate::evaluate(&problem.objective, &params).map_err(SolverError::Eval)?;
    Ok(Solution {
        assignments: assignment,
        objective_value: obj,
        feasible: true,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use yevice_core::optimize::{
        DecisionVariable, ObjectiveDirection, OptimizationConstraint, OptimizationProblem, Relation,
    };
    use yevice_core::expr::Expr;
    use yevice_core::types::VariableName;

    fn var(name: &str) -> VariableName {
        VariableName::new(name)
    }

    fn dv(name: &str, domain: Vec<f64>) -> DecisionVariable {
        DecisionVariable {
            name: var(name),
            domain,
        }
    }

    fn problem_with(
        objective: Expr,
        direction: ObjectiveDirection,
        decision_variables: Vec<DecisionVariable>,
        constraints: Vec<OptimizationConstraint>,
        fixed_params: HashMap<VariableName, f64>,
    ) -> OptimizationProblem {
        OptimizationProblem {
            objective,
            direction,
            decision_variables,
            constraints,
            fixed_params,
        }
    }

    /// `minimizes_unconstrained`: objective = `x`, domain {1, 2, 3} → x=1.
    #[test]
    fn minimizes_unconstrained() {
        let problem = problem_with(
            Expr::variable("x"),
            ObjectiveDirection::Minimize,
            vec![dv("x", vec![1.0, 2.0, 3.0])],
            vec![],
            HashMap::new(),
        );

        let sol = EnumerationSolver.solve(&problem).unwrap();
        assert!(sol.feasible);
        assert_eq!(sol.assignments[&var("x")], 1.0);
        assert_eq!(sol.objective_value, 1.0);
    }

    /// `respects_constraint_picks_cheapest_feasible`:
    /// objective = `price_per_unit * x`, constraint `x >= 2`, domain {1,2,3} → x=2.
    #[test]
    fn respects_constraint_picks_cheapest_feasible() {
        // objective = 10.0 * x
        let objective = Expr::product(vec![Expr::constant(10.0), Expr::variable("x")]);
        // constraint: x >= 2
        let constraint = OptimizationConstraint {
            lhs: Expr::variable("x"),
            relation: Relation::Ge,
            rhs: 2.0,
            label: Some("min_x".into()),
        };

        let problem = problem_with(
            objective,
            ObjectiveDirection::Minimize,
            vec![dv("x", vec![1.0, 2.0, 3.0])],
            vec![constraint],
            HashMap::new(),
        );

        let sol = EnumerationSolver.solve(&problem).unwrap();
        assert!(sol.feasible, "expected a feasible solution");
        assert_eq!(
            sol.assignments[&var("x")],
            2.0,
            "x=1 is infeasible (violates x>=2), so x=2 must be chosen"
        );
        assert_eq!(sol.objective_value, 20.0);
    }

    /// `maximize_direction`: objective = `x`, domain {1,2,3} → x=3.
    #[test]
    fn maximize_direction() {
        let problem = problem_with(
            Expr::variable("x"),
            ObjectiveDirection::Maximize,
            vec![dv("x", vec![1.0, 2.0, 3.0])],
            vec![],
            HashMap::new(),
        );

        let sol = EnumerationSolver.solve(&problem).unwrap();
        assert!(sol.feasible);
        assert_eq!(sol.assignments[&var("x")], 3.0);
        assert_eq!(sol.objective_value, 3.0);
    }

    /// `infeasible_when_no_combo_satisfies`:
    /// constraint `x >= 10`, domain {1, 2} → infeasible.
    #[test]
    fn infeasible_when_no_combo_satisfies() {
        let constraint = OptimizationConstraint {
            lhs: Expr::variable("x"),
            relation: Relation::Ge,
            rhs: 10.0,
            label: None,
        };

        let problem = problem_with(
            Expr::variable("x"),
            ObjectiveDirection::Minimize,
            vec![dv("x", vec![1.0, 2.0])],
            vec![constraint],
            HashMap::new(),
        );

        let sol = EnumerationSolver.solve(&problem).unwrap();
        assert!(!sol.feasible, "expected infeasible result");
        assert!(sol.objective_value.is_nan());
        assert!(sol.assignments.is_empty());
    }

    /// `too_many_combinations_errors`:
    /// Domain product above `MAX_COMBINATIONS` → `Err(TooManyCombinations)`.
    #[test]
    fn too_many_combinations_errors() {
        // 101 decision variables each with domain size 10 → 10^101 combinations.
        let dvs: Vec<DecisionVariable> = (0..101_u32)
            .map(|i| dv(&format!("x{i}"), vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0]))
            .collect();

        let problem = problem_with(
            Expr::constant(0.0),
            ObjectiveDirection::Minimize,
            dvs,
            vec![],
            HashMap::new(),
        );

        let result = EnumerationSolver.solve(&problem);
        assert!(
            matches!(result, Err(SolverError::TooManyCombinations { .. })),
            "expected TooManyCombinations, got {result:?}"
        );
    }

    /// Determinism: when two assignments give the same objective, the one that
    /// appears first in enumeration order is kept.
    #[test]
    fn determinism_picks_first_on_tie() {
        // objective = 0 (constant) — every assignment is equally good.
        let problem = problem_with(
            Expr::constant(0.0),
            ObjectiveDirection::Minimize,
            vec![dv("x", vec![5.0, 3.0, 1.0])],
            vec![],
            HashMap::new(),
        );

        let sol = EnumerationSolver.solve(&problem).unwrap();
        assert!(sol.feasible);
        // The first element of the domain (5.0) should be chosen.
        assert_eq!(sol.assignments[&var("x")], 5.0);
    }

    /// Two decision variables — verifies the full Cartesian product is explored.
    #[test]
    fn two_decision_variables_minimize_sum() {
        // minimize x + y, x ∈ {3, 1}, y ∈ {4, 2}  →  x=1, y=2
        let objective = Expr::sum(vec![Expr::variable("x"), Expr::variable("y")]);

        let problem = problem_with(
            objective,
            ObjectiveDirection::Minimize,
            vec![dv("x", vec![3.0, 1.0]), dv("y", vec![4.0, 2.0])],
            vec![],
            HashMap::new(),
        );

        let sol = EnumerationSolver.solve(&problem).unwrap();
        assert!(sol.feasible);
        assert_eq!(sol.assignments[&var("x")], 1.0);
        assert_eq!(sol.assignments[&var("y")], 2.0);
        assert_eq!(sol.objective_value, 3.0);
    }

    /// Fixed params are correctly merged with decision variable assignments.
    #[test]
    fn fixed_params_are_used_in_objective() {
        // objective = price * x,  price is a fixed param = 5.0
        // domain {1, 2, 3}, minimize → x=1, objective = 5.0
        let objective = Expr::product(vec![Expr::variable("price"), Expr::variable("x")]);
        let mut fixed = HashMap::new();
        fixed.insert(var("price"), 5.0);

        let problem = problem_with(
            objective,
            ObjectiveDirection::Minimize,
            vec![dv("x", vec![1.0, 2.0, 3.0])],
            vec![],
            fixed,
        );

        let sol = EnumerationSolver.solve(&problem).unwrap();
        assert!(sol.feasible);
        assert_eq!(sol.assignments[&var("x")], 1.0);
        assert_eq!(sol.objective_value, 5.0);
    }

    /// Eq constraint: only the assignment where x == 2.0 is feasible.
    #[test]
    fn eq_constraint_satisfied() {
        let constraint = OptimizationConstraint {
            lhs: Expr::variable("x"),
            relation: Relation::Eq,
            rhs: 2.0,
            label: None,
        };

        let problem = problem_with(
            Expr::variable("x"),
            ObjectiveDirection::Maximize,
            vec![dv("x", vec![1.0, 2.0, 3.0])],
            vec![constraint],
            HashMap::new(),
        );

        let sol = EnumerationSolver.solve(&problem).unwrap();
        assert!(sol.feasible);
        assert_eq!(sol.assignments[&var("x")], 2.0);
    }
}
