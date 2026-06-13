//! Solver backends for yevice cost-optimization problems.
//!
//! The primary entry point is the [`Solver`] trait, which takes an
//! [`OptimizationProblem`] and returns a [`Solution`].  The only solver
//! provided here is [`EnumerationSolver`], which tries every element of the
//! Cartesian product of all decision-variable domains.

pub mod error;

use std::collections::HashMap;

pub use error::SolverError;
use yevice_core::evaluate::{self, Params, resolve_bindings_into};
use yevice_core::optimize::{
    DecisionVariable, ObjectiveDirection, OptimizationConstraint, OptimizationProblem, Relation,
};
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
    ///
    /// Kept as a `std::collections::HashMap` (rather than the `FxHashMap`-based
    /// `evaluate::Params`) to keep this public field's type stable; the
    /// solver's per-combination hot-path maps use `Params` internally.
    pub assignments: HashMap<VariableName, f64>,
    /// Objective value at the optimal assignment.  [`f64::NAN`] when infeasible.
    pub objective_value: f64,
    /// True iff at least one feasible assignment was found.
    pub feasible: bool,
    /// Number of combinations that were skipped because objective evaluation
    /// returned an error (e.g. division by zero, undefined variable).  When
    /// this equals `total_combinations` and `feasible` is false, all
    /// combinations failed to evaluate rather than genuinely being infeasible.
    pub evaluation_failures: u64,
    /// Total number of combinations in the Cartesian product of all decision
    /// variable domains.  Zero when the solver returned early (empty domain).
    pub total_combinations: u64,
    /// The formatted error message from the first objective-evaluation failure,
    /// if any occurred.
    pub first_evaluation_error: Option<String>,
}

/// Interface for optimization backends.
pub trait Solver {
    /// Solve the given problem and return the best [`Solution`] found.
    fn solve(&self, problem: &OptimizationProblem) -> Result<Solution, SolverError>;
}

/// Verify that every variable referenced by the problem's objective is bound.
///
/// A variable counts as bound when it is a fixed parameter, a decision
/// variable, or the target of a binding whose own source variables are all
/// (transitively) bound. The bound set is computed as a fixed-point closure so
/// that a binding whose source variable is missing does not silently mask an
/// unbound variable in the objective.
///
/// Returns [`SolverError::UnboundVariables`] naming the offending variables
/// (in sorted order) when the objective cannot be fully evaluated.
///
/// All solver backends should call this before solving; [`EnumerationSolver`]
/// does so at the start of [`Solver::solve`], turning a would-be
/// "all combinations failed to evaluate" outcome into an actionable error.
pub fn validate_bindings(problem: &OptimizationProblem) -> Result<(), SolverError> {
    let mut bound: std::collections::HashSet<VariableName> =
        problem.fixed_params.keys().cloned().collect();
    for dv in &problem.decision_variables {
        bound.insert(dv.name.clone());
    }

    // Fixed-point closure: a binding propagates its target into the bound set
    // only once all of its source variables are themselves bound.
    loop {
        let mut progressed = false;
        for b in &problem.bindings {
            if bound.contains(&b.target) {
                continue;
            }
            if b.expr.variables().iter().all(|v| bound.contains(v)) {
                bound.insert(b.target.clone());
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }

    let unbound: Vec<String> = problem
        .objective
        .variables()
        .into_iter()
        .filter(|v| !bound.contains(v))
        .map(|v| v.to_string())
        .collect();
    if unbound.is_empty() {
        Ok(())
    } else {
        Err(SolverError::UnboundVariables { variables: unbound })
    }
}

/// Prune each decision variable's domain by applying constraints that depend
/// on exactly one decision variable.
///
/// For every constraint whose left-hand expression references **exactly one**
/// decision variable (and no other unbound variables), this function evaluates
/// the constraint for each candidate value of that variable's domain and
/// discards values that violate the constraint. Multi-variable constraints are
/// left untouched — they are checked during enumeration as before. This
/// preserves correctness while reducing the Cartesian product the enumerator
/// has to walk.
///
/// Returns the pruned list of decision variables. If any decision variable's
/// domain becomes empty after pruning, [`SolverError::Infeasible`] is returned
/// (no assignment can satisfy that single-variable constraint).
///
/// Values for which the constraint LHS fails to evaluate (e.g. division by
/// zero) are also dropped — they cannot be feasible.
///
/// Fixed parameters and `bindings` are taken into account when evaluating the
/// constraint LHS so that derived values referencing the single decision
/// variable resolve correctly (e.g. `usage = x * factor`).
pub fn prune_domains(problem: &OptimizationProblem) -> Result<Vec<DecisionVariable>, SolverError> {
    let mut pruned: Vec<DecisionVariable> = problem.decision_variables.clone();
    if pruned.is_empty() || problem.constraints.is_empty() {
        return Ok(pruned);
    }

    // Detect duplicate decision-variable names.  When the same name appears
    // more than once in `decision_variables`, the enumerator uses last-write-wins
    // semantics (the last slot's domain value is always written last into the
    // scratch map).  Pruning each slot independently could wrongly empty an
    // earlier slot's domain even when the later slot contains feasible values,
    // producing a spurious Infeasible result.  Guard against this by collecting
    // all names that appear more than once and skipping pruning for them.
    let mut name_count: std::collections::HashMap<VariableName, usize> =
        std::collections::HashMap::new();
    for dv in &pruned {
        *name_count.entry(dv.name.clone()).or_insert(0) += 1;
    }
    let duplicate_names: std::collections::HashSet<VariableName> = name_count
        .into_iter()
        .filter_map(|(name, count)| if count > 1 { Some(name) } else { None })
        .collect();
    if !duplicate_names.is_empty() {
        for name in &duplicate_names {
            tracing::warn!(
                variable = %name,
                "duplicate decision-variable name detected; skipping domain pruning \
                 for this variable to preserve enumerator last-write-wins semantics",
            );
        }
    }

    // Decision-variable name set, used to detect "this constraint references
    // exactly one decision variable" robustly even when bindings introduce
    // other variable names.
    let decision_names: std::collections::HashSet<VariableName> =
        pruned.iter().map(|dv| dv.name.clone()).collect();

    // Pre-compute, for each constraint, the set of *decision* variables it
    // depends on (transitively through bindings).  We treat fixed-param-only
    // dependencies as "no decision dependency".
    let single_var_constraints: Vec<(&OptimizationConstraint, VariableName)> = problem
        .constraints
        .iter()
        .filter_map(|c| {
            let depends_on = constraint_decision_dependencies(c, problem, &decision_names);
            if depends_on.len() == 1 {
                let name = depends_on.into_iter().next().unwrap();
                Some((c, name))
            } else {
                None
            }
        })
        .collect();

    if single_var_constraints.is_empty() {
        return Ok(pruned);
    }

    // Base params: fixed parameters only. Per-variable filtering writes the
    // candidate domain value into a scratch clone of this base.
    let base: Params = problem
        .fixed_params
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect();

    let before: u64 = pruned
        .iter()
        .map(|dv| dv.domain.len() as u64)
        .fold(1u64, u64::saturating_mul);

    for dv in &mut pruned {
        // Skip pruning for variables whose name appears more than once:
        // the enumerator's last-write-wins semantics would make pruning an
        // earlier slot incorrect (see duplicate_names detection above).
        if duplicate_names.contains(&dv.name) {
            continue;
        }

        // Collect the constraints that target this specific decision variable.
        let relevant: Vec<&OptimizationConstraint> = single_var_constraints
            .iter()
            .filter_map(|(c, name)| if name == &dv.name { Some(*c) } else { None })
            .collect();
        if relevant.is_empty() {
            continue;
        }

        let original_len = dv.domain.len();
        // Names whose slots in `scratch` are "protected" from being cleared
        // between iterations:
        //   - fixed_params (already in `base`)
        //   - other decision variables (pre-seeded to 0.0 below)
        //   - the variable we're filtering for (set per-iteration)
        // Anything else is a binding-target whose stale value MUST be cleared
        // before re-resolving for the next candidate value, otherwise
        // `resolve_bindings_into`'s `contains_key` skip would reuse the
        // previous iteration's result.
        let other_decision_names: std::collections::HashSet<&VariableName> = problem
            .decision_variables
            .iter()
            .map(|d| &d.name)
            .filter(|n| **n != dv.name)
            .collect();

        let mut scratch: Params = base.clone();
        for &name in &other_decision_names {
            scratch.entry(name.clone()).or_insert(0.0);
        }

        dv.domain.retain(|&value| {
            scratch.insert(dv.name.clone(), value);
            // Resolve bindings before evaluating the constraint LHS.
            resolve_bindings_into(&mut scratch, &problem.bindings);
            let keep = relevant.iter().all(|c| {
                let lhs = match evaluate::evaluate(&c.lhs, &scratch) {
                    Ok(v) => v,
                    Err(_) => return false,
                };
                match c.relation {
                    Relation::Le => lhs <= c.rhs + CONSTRAINT_TOLERANCE,
                    Relation::Ge => lhs >= c.rhs - CONSTRAINT_TOLERANCE,
                    Relation::Eq => (lhs - c.rhs).abs() <= CONSTRAINT_TOLERANCE,
                }
            });
            // Clear binding-derived values so the next iteration's binding
            // resolution sees a fresh slate. Preserve fixed params, the
            // current decision variable, and any other decision-variable
            // slots.
            for b in &problem.bindings {
                if !base.contains_key(&b.target)
                    && b.target != dv.name
                    && !other_decision_names.contains(&b.target)
                {
                    scratch.remove(&b.target);
                }
            }
            keep
        });

        if dv.domain.len() < original_len {
            tracing::debug!(
                variable = %dv.name,
                before = original_len,
                after = dv.domain.len(),
                "pruned single-variable constraint domain",
            );
        }

        if dv.domain.is_empty() {
            return Err(SolverError::Infeasible);
        }
    }

    let after: u64 = pruned
        .iter()
        .map(|dv| dv.domain.len() as u64)
        .fold(1u64, u64::saturating_mul);

    if after < before {
        tracing::debug!(
            combinations_before = before,
            combinations_after = after,
            "domain pruning reduced combination count",
        );
    }

    Ok(pruned)
}

/// Identify the **decision-variable** names that the given constraint depends
/// on, walking through `bindings` transitively. Fixed-parameter references are
/// ignored. The returned set contains only names from the problem's decision
/// variables.
fn constraint_decision_dependencies(
    constraint: &OptimizationConstraint,
    problem: &OptimizationProblem,
    decision_names: &std::collections::HashSet<VariableName>,
) -> std::collections::HashSet<VariableName> {
    // Standard frontier BFS through bindings: start with the LHS's variables,
    // expand any binding target to its source variables, until no new names are
    // discovered. Then keep only decision-variable names.
    let mut visited: std::collections::HashSet<VariableName> = std::collections::HashSet::new();
    let mut frontier: Vec<VariableName> = constraint.lhs.variables().into_iter().collect();

    while let Some(name) = frontier.pop() {
        if !visited.insert(name.clone()) {
            continue;
        }
        // If this name is the target of a binding, expand it.
        for b in &problem.bindings {
            if b.target == name {
                for v in b.expr.variables() {
                    if !visited.contains(&v) {
                        frontier.push(v);
                    }
                }
            }
        }
    }

    visited
        .into_iter()
        .filter(|v| decision_names.contains(v))
        .collect()
}

/// Construct a solver instance by name.
///
/// This is the public factory used by the CLI's `--solver` option. New
/// backends (LP/MIP) will plug in here behind feature gates.
///
/// Returns a descriptive error naming the allowed values when `name` is
/// unknown.
pub fn solver_from_name(name: &str) -> Result<Box<dyn Solver>, SolverError> {
    match name {
        "enumeration" => Ok(Box::new(EnumerationSolver)),
        other => Err(SolverError::UnknownSolver {
            requested: other.to_string(),
            allowed: vec!["enumeration".to_string()],
        }),
    }
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
/// `MAX_COMBINATIONS` with [`SolverError::TooManyCombinations`].
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
        // Reject objectives that reference unbound variables up-front: every
        // combination would fail to evaluate anyway, and an explicit error
        // names the missing variables instead of reporting INFEASIBLE.
        validate_bindings(problem)?;

        // Guard: if any decision variable has an empty domain, no assignment
        // can ever be constructed — immediately return infeasible.
        // This check must come before combination_count so that a problem
        // with a huge leading domain followed by an empty domain returns
        // infeasible rather than TooManyCombinations.
        if problem
            .decision_variables
            .iter()
            .any(|dv| dv.domain.is_empty())
        {
            return Ok(Solution {
                assignments: HashMap::new(),
                objective_value: f64::NAN,
                feasible: false,
                evaluation_failures: 0,
                total_combinations: 0,
                first_evaluation_error: None,
            });
        }

        // Domain pre-filtering: drop values that violate any single-variable
        // constraint up-front to shrink the Cartesian product.  Returns
        // SolverError::Infeasible when any variable's domain becomes empty.
        let pruned_vars = match prune_domains(problem) {
            Ok(v) => v,
            Err(SolverError::Infeasible) => {
                return Ok(Solution {
                    assignments: HashMap::new(),
                    objective_value: f64::NAN,
                    feasible: false,
                    evaluation_failures: 0,
                    total_combinations: 0,
                    first_evaluation_error: None,
                });
            }
            Err(e) => return Err(e),
        };

        // Construct a thin shallow copy of the problem with the pruned
        // decision variables so combination_count and the enumeration loop
        // walk the reduced domains.  We avoid cloning the whole problem
        // (which can be large) — only the decision-variable list is replaced.
        let pruned_problem = OptimizationProblem {
            objective: problem.objective.clone(),
            direction: problem.direction,
            decision_variables: pruned_vars,
            constraints: problem.constraints.clone(),
            fixed_params: problem.fixed_params.clone(),
            bindings: problem.bindings.clone(),
        };
        let problem = &pruned_problem;

        // Guard against combinatorial explosion.
        let combination_count = combination_count(problem)?;

        let vars = &problem.decision_variables;
        let n = vars.len();

        // Precompute mixed-radix weights: weights[k] is the number of
        // combinations contributed by variables k+1..n.  This lets us decode
        // any flat index i into a per-variable selection index in O(n).
        //
        //   weights[n-1] = 1
        //   weights[k]   = weights[k+1] * vars[k+1].domain.len()   (k = n-2 ..= 0)
        //
        // vars[0] is the most-significant "digit" (slowest-changing), matching
        // the original nested-loop order exactly.
        let mut weights = vec![1u64; n];
        for k in (0..n.saturating_sub(1)).rev() {
            weights[k] = weights[k + 1].saturating_mul(vars[k + 1].domain.len() as u64);
        }

        // Build the base params map: fixed_params + decision var slot pre-seed.
        //
        // Decision slots are pre-inserted so that the inner loop can update
        // values via get_mut without a fresh allocation each iteration.
        // Fixed params are pre-inserted so resolve_bindings_into's contains_key
        // guard preserves fixed_param values against any colliding binding target.
        let mut base: Params = problem
            .fixed_params
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        for var in vars {
            base.entry(var.name.clone()).or_insert(0.0);
        }

        // Scratch buffer: reused every iteration via clone_from.
        let mut scratch: Params = Params::default();

        let mut best: Option<Solution> = None;
        let mut evaluation_failures: u64 = 0;
        let mut first_evaluation_error: Option<String> = None;

        for i in 0..combination_count {
            // Restore scratch to the base state (fixed_params + decision var slots).
            scratch.clone_from(&base);

            // Write decision variable assignments into their pre-seeded slots.
            for k in 0..n {
                let idx = ((i / weights[k]) as usize) % vars[k].domain.len();
                *scratch.get_mut(&vars[k].name).expect(
                    "decision variable slot pre-seeded into base before the enumeration loop",
                ) = vars[k].domain[idx];
            }

            // Resolve ALL bindings inside the loop. resolve_bindings_into is
            // fixed-point and uses contains_key skip, which faithfully reproduces
            // the documented semantics: fixed_param values and decision var slot
            // values win; for remaining targets, the first resolvable binding
            // (across passes) wins.
            resolve_bindings_into(&mut scratch, &problem.bindings);

            // Evaluate and check all constraints; skip this combination on any
            // evaluation error (treat as infeasible).
            if !is_feasible(problem, &scratch) {
                continue;
            }

            // Evaluate the objective; skip on error.
            let obj = match evaluate::evaluate(&problem.objective, &scratch) {
                Ok(v) => v,
                Err(e) => {
                    evaluation_failures += 1;
                    if first_evaluation_error.is_none() {
                        first_evaluation_error = Some(format!("{e}"));
                    }
                    continue;
                }
            };

            let better = match &best {
                None => true,
                Some(current) => match problem.direction {
                    ObjectiveDirection::Minimize => obj < current.objective_value,
                    ObjectiveDirection::Maximize => obj > current.objective_value,
                },
            };

            if better {
                // Build the assignments map only when a new best is found
                // (this path is taken at most O(combination_count) times but
                // typically far fewer).
                let assignments: HashMap<VariableName, f64> = vars
                    .iter()
                    .map(|dv| {
                        (
                            dv.name.clone(),
                            *scratch
                                .get(&dv.name)
                                .expect("decision variable slot pre-seeded into base before the enumeration loop"),
                        )
                    })
                    .collect();
                best = Some(Solution {
                    assignments,
                    objective_value: obj,
                    feasible: true,
                    // Diagnostic counters are finalized after the loop; these
                    // placeholder values are overwritten before the function returns.
                    evaluation_failures: 0,
                    total_combinations: combination_count,
                    first_evaluation_error: None,
                });
            }
        }

        // Finalize diagnostic counters now that the full loop is complete.
        Ok(match best {
            Some(mut sol) => {
                sol.evaluation_failures = evaluation_failures;
                sol.first_evaluation_error = first_evaluation_error;
                sol
            }
            None => Solution {
                assignments: HashMap::new(),
                objective_value: f64::NAN,
                feasible: false,
                evaluation_failures,
                total_combinations: combination_count,
                first_evaluation_error,
            },
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compute the total number of combinations (product of domain sizes).
///
/// Returns [`SolverError::TooManyCombinations`] if the count exceeds
/// `MAX_COMBINATIONS`.
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use yevice_core::expr::Expr;
    use yevice_core::optimize::{
        DecisionVariable, ObjectiveDirection, OptimizationConstraint, OptimizationProblem, Relation,
    };
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
            bindings: vec![],
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
            .map(|i| {
                dv(
                    &format!("x{i}"),
                    vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0],
                )
            })
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

    /// Empty domain is immediately infeasible — even with no constraints.
    #[test]
    fn empty_domain_is_infeasible() {
        let problem = problem_with(
            Expr::variable("x"),
            ObjectiveDirection::Minimize,
            vec![dv("x", vec![])],
            vec![],
            HashMap::new(),
        );

        let sol = EnumerationSolver.solve(&problem).unwrap();
        assert!(!sol.feasible, "empty domain must be infeasible");
        assert!(sol.objective_value.is_nan());
        assert!(sol.assignments.is_empty());
    }

    /// Mixed: one variable with domain, one with empty domain → infeasible.
    #[test]
    fn mixed_empty_domain_is_infeasible() {
        let problem = problem_with(
            Expr::sum(vec![Expr::variable("x"), Expr::variable("y")]),
            ObjectiveDirection::Minimize,
            vec![dv("x", vec![1.0, 2.0]), dv("y", vec![])],
            vec![],
            HashMap::new(),
        );

        let sol = EnumerationSolver.solve(&problem).unwrap();
        assert!(
            !sol.feasible,
            "empty domain on any variable must be infeasible"
        );
        assert!(sol.objective_value.is_nan());
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

    /// Binding: `derived = source * 2.0`.
    /// `source` is provided via `fixed_params`; `derived` is referenced in the
    /// objective.  Without binding resolution the solver would fail to evaluate
    /// the objective and return infeasible.
    #[test]
    fn binding_target_resolved_from_fixed_source() {
        use yevice_core::cost::VariableBinding;

        // objective = derived  (binding: derived = source * 2)
        // fixed_params: source = 5.0  →  derived = 10.0
        let binding = VariableBinding {
            target: var("derived"),
            expr: Expr::product(vec![Expr::variable("source"), Expr::constant(2.0)]),
            description: "derived = source * 2".into(),
            source: "test".into(),
        };

        let mut fixed = HashMap::new();
        fixed.insert(var("source"), 5.0);

        let problem = OptimizationProblem {
            objective: Expr::variable("derived"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![],
            constraints: vec![],
            fixed_params: fixed,
            bindings: vec![binding],
        };

        let sol = EnumerationSolver.solve(&problem).unwrap();
        assert!(sol.feasible, "expected feasible: binding must be resolved");
        assert_eq!(sol.objective_value, 10.0);
    }

    /// Huge leading domain (exceeds MAX_COMBINATIONS alone) followed by an
    /// empty-domain variable must return infeasible, not TooManyCombinations.
    ///
    /// This validates that the empty-domain guard runs before combination_count.
    #[test]
    fn huge_leading_domain_plus_empty_domain_is_infeasible() {
        // First variable: domain size 10^101 (would exceed MAX_COMBINATIONS alone).
        let huge_dvs: Vec<DecisionVariable> = (0..101_u32)
            .map(|i| {
                dv(
                    &format!("x{i}"),
                    vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0],
                )
            })
            .collect();
        // Append a final variable with an empty domain.
        let mut dvs = huge_dvs;
        dvs.push(dv("empty_var", vec![]));

        let problem = problem_with(
            Expr::constant(0.0),
            ObjectiveDirection::Minimize,
            dvs,
            vec![],
            HashMap::new(),
        );

        let result = EnumerationSolver.solve(&problem);
        assert!(
            matches!(
                result,
                Ok(Solution {
                    feasible: false,
                    ..
                })
            ),
            "expected infeasible (empty domain), got {result:?}"
        );
    }

    /// Binding where the source is a decision variable.
    /// `x` is a decision variable; `cost = x * price_per_unit` where
    /// `price_per_unit` is derived via a binding from the fixed param `rate`.
    #[test]
    fn binding_source_is_decision_variable() {
        use yevice_core::cost::VariableBinding;

        // objective = x * price_per_unit
        // binding: price_per_unit = x * rate   (price scales with x)
        // fixed_params: rate = 3.0
        // domain for x: {1.0, 2.0, 4.0}
        // effective costs: x=1 → price=3, cost=3; x=2 → price=6, cost=12; x=4 → price=12, cost=48
        // minimize → x=1
        let binding = VariableBinding {
            target: var("price_per_unit"),
            expr: Expr::product(vec![Expr::variable("x"), Expr::variable("rate")]),
            description: "price_per_unit = x * rate".into(),
            source: "test".into(),
        };

        let mut fixed = HashMap::new();
        fixed.insert(var("rate"), 3.0);

        let problem = OptimizationProblem {
            objective: Expr::product(vec![Expr::variable("x"), Expr::variable("price_per_unit")]),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![dv("x", vec![1.0, 2.0, 4.0])],
            constraints: vec![],
            fixed_params: fixed,
            bindings: vec![binding],
        };

        let sol = EnumerationSolver.solve(&problem).unwrap();
        assert!(
            sol.feasible,
            "expected feasible with decision-variable binding source"
        );
        assert_eq!(sol.assignments[&var("x")], 1.0, "x=1 minimises x*x*rate");
        assert_eq!(sol.objective_value, 3.0); // 1 * (1 * 3)
    }

    // -----------------------------------------------------------------------
    // Tests for partition-based refactor correctness
    // -----------------------------------------------------------------------

    /// Mixed bindings: one fixed-only binding and one decision-dependent binding.
    ///
    /// `base_cost` is a fixed binding (`fixed_rate * 2`).
    /// `total_cost` is a decision-dependent binding (`x * base_cost`).
    ///
    /// Objective = `total_cost`. Minimize over x ∈ {1, 3, 5}.
    ///
    /// Expected: x=1, total_cost = 1 * (fixed_rate * 2) = 1 * 10 = 10.
    #[test]
    fn mixed_fixed_and_decision_bindings_correct_optimal() {
        use yevice_core::cost::VariableBinding;

        // fixed_rate = 5.0 (fixed param)
        // base_cost  = fixed_rate * 2   (fixed binding — no decision dependency)
        // total_cost = x * base_cost    (decision-dependent binding)
        let fixed_binding = VariableBinding {
            target: var("base_cost"),
            expr: Expr::product(vec![Expr::variable("fixed_rate"), Expr::constant(2.0)]),
            description: "base_cost = fixed_rate * 2".into(),
            source: "test".into(),
        };
        let decision_binding = VariableBinding {
            target: var("total_cost"),
            expr: Expr::product(vec![Expr::variable("x"), Expr::variable("base_cost")]),
            description: "total_cost = x * base_cost".into(),
            source: "test".into(),
        };

        let mut fixed = HashMap::new();
        fixed.insert(var("fixed_rate"), 5.0);

        let problem = OptimizationProblem {
            objective: Expr::variable("total_cost"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![dv("x", vec![1.0, 3.0, 5.0])],
            constraints: vec![],
            fixed_params: fixed,
            // fixed_binding listed second (adversarial order for partition correctness)
            bindings: vec![decision_binding, fixed_binding],
        };

        let sol = EnumerationSolver.solve(&problem).unwrap();
        assert!(sol.feasible);
        assert_eq!(
            sol.assignments[&var("x")],
            1.0,
            "x=1 gives total_cost=10 (minimum)"
        );
        assert_eq!(sol.objective_value, 10.0); // 1 * (5 * 2)
    }

    /// Full exhaustive equivalence check: for a small problem containing both
    /// fixed-only and decision-dependent bindings, verify that the refactored
    /// `EnumerationSolver` produces the same (solution, objective) as a naive
    /// reference computation that re-evaluates every combination from scratch.
    ///
    /// Problem setup:
    ///   x ∈ {1, 2, 3},  y ∈ {10, 20}
    ///   fixed param:  scale = 3.0
    ///   fixed binding:  offset = scale * 4   (= 12, decision-independent)
    ///   decision binding: result = x * y + offset
    ///   objective: result   (minimize)
    ///   constraint: result >= 30
    ///
    /// Feasible combos where result >= 30 (enumeration order: x slow, y fast):
    ///   x=1, y=10 → result = 10 + 12 = 22  (infeasible)
    ///   x=1, y=20 → result = 20 + 12 = 32  (feasible, first minimum at result=32)
    ///   x=2, y=10 → result = 20 + 12 = 32  (feasible, ties with above)
    ///   x=2, y=20 → result = 40 + 12 = 52  (feasible)
    ///   x=3, y=10 → result = 30 + 12 = 42  (feasible)
    ///   x=3, y=20 → result = 60 + 12 = 72  (feasible)
    ///
    /// Minimum feasible result = 32, first occurrence at x=1, y=20 (x is the
    /// slow-moving variable in enumeration order, so x=1 is visited first).
    #[test]
    fn exhaustive_equivalence_with_naive_reference() {
        use yevice_core::cost::VariableBinding;
        use yevice_core::evaluate::resolve_bindings;

        let fixed_binding = VariableBinding {
            target: var("offset"),
            expr: Expr::product(vec![Expr::variable("scale"), Expr::constant(4.0)]),
            description: "offset = scale * 4".into(),
            source: "test".into(),
        };
        let decision_binding = VariableBinding {
            target: var("result"),
            expr: Expr::sum(vec![
                Expr::product(vec![Expr::variable("x"), Expr::variable("y")]),
                Expr::variable("offset"),
            ]),
            description: "result = x * y + offset".into(),
            source: "test".into(),
        };
        let constraint = OptimizationConstraint {
            lhs: Expr::variable("result"),
            relation: Relation::Ge,
            rhs: 30.0,
            label: Some("result_ge_30".into()),
        };

        let mut fixed = HashMap::new();
        fixed.insert(var("scale"), 3.0);

        let problem = OptimizationProblem {
            objective: Expr::variable("result"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![dv("x", vec![1.0, 2.0, 3.0]), dv("y", vec![10.0, 20.0])],
            constraints: vec![constraint],
            fixed_params: fixed.clone(),
            bindings: vec![fixed_binding.clone(), decision_binding.clone()],
        };

        // --- Naive reference computation ---
        let x_domain = [1.0_f64, 2.0, 3.0];
        let y_domain = [10.0_f64, 20.0];
        let mut naive_best: Option<(f64, f64, f64)> = None; // (obj, x, y)
        for &xv in &x_domain {
            for &yv in &y_domain {
                let mut params: Params = fixed.iter().map(|(k, v)| (k.clone(), *v)).collect();
                params.insert(var("x"), xv);
                params.insert(var("y"), yv);
                let all_bindings = vec![fixed_binding.clone(), decision_binding.clone()];
                let resolved = resolve_bindings(&all_bindings, &params).unwrap();
                let result_val = match resolved.get(&var("result")) {
                    Some(&v) => v,
                    None => continue,
                };
                if result_val < 30.0 - 1e-9 {
                    continue;
                }
                match naive_best {
                    None => naive_best = Some((result_val, xv, yv)),
                    Some((best_obj, _, _)) if result_val < best_obj => {
                        naive_best = Some((result_val, xv, yv));
                    }
                    _ => {}
                }
            }
        }

        let (naive_obj, naive_x, naive_y) = naive_best.expect("naive must find a solution");

        let sol = EnumerationSolver.solve(&problem).unwrap();
        assert!(sol.feasible, "solver must find a feasible solution");

        // Golden assertion (independent oracle): minimum feasible result = 32,
        // first occurrence in enumeration order at x=1, y=20.
        assert_eq!(sol.objective_value, 32.0, "expected objective = 32");
        assert_eq!(sol.assignments[&var("x")], 1.0, "expected x = 1");
        assert_eq!(sol.assignments[&var("y")], 20.0, "expected y = 20");

        // Equivalence with naive reference.
        assert!(
            (sol.objective_value - naive_obj).abs() < 1e-9,
            "objective mismatch: solver={} naive={}",
            sol.objective_value,
            naive_obj
        );
        assert_eq!(
            sol.assignments[&var("x")],
            naive_x,
            "x mismatch: solver={} naive={}",
            sol.assignments[&var("x")],
            naive_x
        );
        assert_eq!(
            sol.assignments[&var("y")],
            naive_y,
            "y mismatch: solver={} naive={}",
            sol.assignments[&var("y")],
            naive_y
        );
    }

    /// Chained decision-dependent bindings: `a = x * 2`, `b = a + y`.
    /// Objective = `b`, minimize. x ∈ {1, 2}, y ∈ {0, 5}.
    ///
    /// Values: (x=1,y=0)→b=2, (x=1,y=5)→b=7, (x=2,y=0)→b=4, (x=2,y=5)→b=9
    /// Minimum: x=1, y=0, b=2.
    #[test]
    fn chained_decision_bindings_resolve_correctly() {
        use yevice_core::cost::VariableBinding;

        let binding_a = VariableBinding {
            target: var("a"),
            expr: Expr::product(vec![Expr::variable("x"), Expr::constant(2.0)]),
            description: "a = x * 2".into(),
            source: "test".into(),
        };
        let binding_b = VariableBinding {
            target: var("b"),
            expr: Expr::sum(vec![Expr::variable("a"), Expr::variable("y")]),
            description: "b = a + y".into(),
            source: "test".into(),
        };

        let problem = OptimizationProblem {
            objective: Expr::variable("b"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![dv("x", vec![1.0, 2.0]), dv("y", vec![0.0, 5.0])],
            constraints: vec![],
            fixed_params: HashMap::new(),
            // Adversarial order: b before a (a must be resolved first)
            bindings: vec![binding_b, binding_a],
        };

        let sol = EnumerationSolver.solve(&problem).unwrap();
        assert!(sol.feasible);
        assert_eq!(sol.assignments[&var("x")], 1.0);
        assert_eq!(sol.assignments[&var("y")], 0.0);
        assert_eq!(sol.objective_value, 2.0); // 1*2 + 0
    }

    // -----------------------------------------------------------------------
    // Regression tests for binding priority (bug-1 and bug-2 fixes)
    // -----------------------------------------------------------------------

    /// Regression (bug-1): when a fixed_param name collides with the target of a
    /// decision-dependent binding, the fixed_param value must win — the binding
    /// must NOT overwrite the explicit user-supplied value.
    ///
    /// Setup:
    ///   fixed_params: x = 99.0      (explicit user override)
    ///   decision variable: y ∈ {1, 2, 3}
    ///   binding: x = y + 100         (decision-dependent — target collides with fixed_param)
    ///   objective: x                 (minimize)
    ///
    /// Expected: objective = 99.0 (fixed_param wins; binding never overwrites it).
    /// Buggy behaviour: objective = 101.0 (binding overwrites fixed_param for y=1).
    #[test]
    fn fixed_param_overrides_colliding_decision_binding() {
        use yevice_core::cost::VariableBinding;

        // binding: x = y + 100  (x is also a fixed_param)
        let binding = VariableBinding {
            target: var("x"),
            expr: Expr::sum(vec![Expr::variable("y"), Expr::constant(100.0)]),
            description: "x = y + 100".into(),
            source: "test".into(),
        };

        let mut fixed = HashMap::new();
        fixed.insert(var("x"), 99.0); // explicit fixed_param — must win

        let problem = OptimizationProblem {
            objective: Expr::variable("x"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![dv("y", vec![1.0, 2.0, 3.0])],
            constraints: vec![],
            fixed_params: fixed,
            bindings: vec![binding],
        };

        let sol = EnumerationSolver.solve(&problem).unwrap();
        assert!(sol.feasible, "expected feasible solution");
        // Fixed param x=99.0 must be used as objective, not x = y + 100 = 101..103.
        assert_eq!(
            sol.objective_value, 99.0,
            "fixed_param x=99 must override the colliding decision binding (expected 99, got {})",
            sol.objective_value
        );
    }

    /// Regression (bug-2): when a decision variable name collides with the target
    /// of a binding whose expression references another decision variable, the
    /// decision variable's own domain value must be used — the binding must NOT
    /// overwrite the decision value.
    ///
    /// Setup:
    ///   decision variables: x ∈ {1, 2, 3},  y ∈ {10, 20}
    ///   binding: x = y + 100            (target collides with decision variable x)
    ///   objective: x                    (minimize)
    ///
    /// Expected: x=1, objective=1 (decision value wins; binding never overwrites).
    /// Buggy behaviour: x=110, objective=110 (binding overwrites decision value).
    #[test]
    fn decision_var_not_overwritten_by_colliding_binding() {
        use yevice_core::cost::VariableBinding;

        // binding: x = y + 100  (x is also a decision variable)
        let binding = VariableBinding {
            target: var("x"),
            expr: Expr::sum(vec![Expr::variable("y"), Expr::constant(100.0)]),
            description: "x = y + 100".into(),
            source: "test".into(),
        };

        let problem = OptimizationProblem {
            objective: Expr::variable("x"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![dv("x", vec![1.0, 2.0, 3.0]), dv("y", vec![10.0, 20.0])],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![binding],
        };

        let sol = EnumerationSolver.solve(&problem).unwrap();
        assert!(sol.feasible, "expected feasible solution");
        // Decision variable x must keep its own domain value (1), not y + 100 (110 or 120).
        assert_eq!(
            sol.assignments[&var("x")],
            1.0,
            "decision variable x must not be overwritten by binding (expected x=1, got {})",
            sol.assignments[&var("x")]
        );
        assert_eq!(
            sol.objective_value, 1.0,
            "objective must be decision x=1, not binding result (expected 1, got {})",
            sol.objective_value
        );
    }

    // -----------------------------------------------------------------------
    // Regression tests for contested-target (binding-order) fix
    // -----------------------------------------------------------------------

    /// Regression: when the SAME target `T` appears in both a decision-dependent
    /// binding and a fixed binding, and the decision-dependent binding is listed
    /// FIRST in `problem.bindings`, the decision-dependent value must win.
    ///
    /// Setup:
    ///   decision variable: y ∈ {1, 2, 3}
    ///   binding (first):  T = y + 0    (decision-dependent — listed first)
    ///   binding (second): T = 100      (fixed literal — listed second)
    ///   objective: T                   (minimize)
    ///
    /// Old single-pass semantics: first resolvable binding wins → T = y + 0 wins.
    /// Expected: optimal T = min(y) = 1.
    /// Buggy behaviour (before this fix): T = 100 always wins (fixed partition).
    #[test]
    fn binding_order_decision_dependent_wins_over_fixed_binding() {
        use yevice_core::cost::VariableBinding;

        // First binding: T = y + 0  (decision-dependent; y is a decision variable)
        let binding_decision = VariableBinding {
            target: var("T"),
            expr: Expr::sum(vec![Expr::variable("y"), Expr::constant(0.0)]),
            description: "T = y + 0".into(),
            source: "test".into(),
        };
        // Second binding: T = 100  (fixed literal; no decision-variable reference)
        let binding_fixed = VariableBinding {
            target: var("T"),
            expr: Expr::constant(100.0),
            description: "T = 100".into(),
            source: "test".into(),
        };

        let problem = OptimizationProblem {
            objective: Expr::variable("T"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![dv("y", vec![1.0, 2.0, 3.0])],
            constraints: vec![],
            fixed_params: HashMap::new(),
            // Decision-dependent binding is listed first → it must win.
            bindings: vec![binding_decision, binding_fixed],
        };

        let sol = EnumerationSolver.solve(&problem).unwrap();
        assert!(sol.feasible, "expected feasible solution");
        assert_eq!(
            sol.objective_value, 1.0,
            "decision-dependent binding (T = y + 0) must win over fixed binding (T = 100); \
             expected T = min(y) = 1, got {}",
            sol.objective_value
        );
    }

    /// Sanity: when only the fixed binding for `T` is present (no contested
    /// target), the fixed value is still resolved correctly.
    ///
    /// Setup:
    ///   decision variable: y ∈ {1, 2, 3}   (y is NOT bound to T)
    ///   binding: T = 100                    (fixed literal, only binding for T)
    ///   objective: T                        (minimize)
    ///
    /// Expected: T = 100 (the fixed binding is the sole source).
    #[test]
    fn binding_order_fixed_wins_when_no_decision_binding() {
        use yevice_core::cost::VariableBinding;

        let binding_fixed = VariableBinding {
            target: var("T"),
            expr: Expr::constant(100.0),
            description: "T = 100".into(),
            source: "test".into(),
        };

        let problem = OptimizationProblem {
            objective: Expr::variable("T"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![dv("y", vec![1.0, 2.0, 3.0])],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![binding_fixed],
        };

        let sol = EnumerationSolver.solve(&problem).unwrap();
        assert!(sol.feasible, "expected feasible solution");
        assert_eq!(
            sol.objective_value, 100.0,
            "sole fixed binding T=100 must be resolved correctly; expected 100, got {}",
            sol.objective_value
        );
    }

    /// Regression (Scenario A): when a contested target T equals a fixed_params
    /// key, the fixed_param value must win — the decision-dependent binding must
    /// NOT overwrite it.
    ///
    /// Setup:
    ///   fixed_params: x = 99.0
    ///   decision variable: y ∈ {1, 2, 3}
    ///   binding (first):  x = 50           (fixed literal — listed first)
    ///   binding (second): x = y + 100      (decision-dependent — listed second)
    ///   objective: x                       (minimize)
    ///
    /// Expected: feasible=true, objective=99.0, assignments\["x"\]=99.0.
    /// Buggy behaviour (old patch): scratch.remove("x") deleted the fixed_param
    /// value; decision binding wrote x = y+100 = 101..103.
    #[test]
    fn contested_target_equals_fixed_param_uses_fixed_param_value() {
        use yevice_core::cost::VariableBinding;

        let binding_fixed = VariableBinding {
            target: var("x"),
            expr: Expr::constant(50.0),
            description: "x = 50".into(),
            source: "test".into(),
        };
        let binding_decision = VariableBinding {
            target: var("x"),
            expr: Expr::sum(vec![Expr::variable("y"), Expr::constant(100.0)]),
            description: "x = y + 100".into(),
            source: "test".into(),
        };

        let mut fixed = HashMap::new();
        fixed.insert(var("x"), 99.0);

        let problem = OptimizationProblem {
            objective: Expr::variable("x"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![dv("y", vec![1.0, 2.0, 3.0])],
            constraints: vec![],
            fixed_params: fixed,
            bindings: vec![binding_fixed, binding_decision],
        };

        let sol = EnumerationSolver.solve(&problem).unwrap();
        assert!(sol.feasible, "expected feasible solution");
        assert_eq!(
            sol.objective_value, 99.0,
            "fixed_param x=99 must win over contested bindings; expected 99, got {}",
            sol.objective_value
        );
    }

    /// Regression (Scenario B): when a contested target T equals a decision
    /// variable name, the decision variable's own domain value must win — the
    /// binding must NOT overwrite the slot value.
    ///
    /// Setup:
    ///   decision variables: x ∈ {1, 2, 3},  y ∈ {1, 2, 3}
    ///   binding (first):  x = 50            (fixed literal — listed first)
    ///   binding (second): x = y + 100       (decision-dependent — listed second)
    ///   objective: x                        (minimize)
    ///
    /// Expected: feasible=true, objective=1.0, assignments\["x"\]=1.0 (NOT 101+).
    /// Buggy behaviour (old patch): scratch.remove("x") deleted the decision
    /// slot value; decision binding wrote x = y+100.
    #[test]
    fn contested_target_equals_decision_var_uses_decision_value() {
        use yevice_core::cost::VariableBinding;

        let binding_fixed = VariableBinding {
            target: var("x"),
            expr: Expr::constant(50.0),
            description: "x = 50".into(),
            source: "test".into(),
        };
        let binding_decision = VariableBinding {
            target: var("x"),
            expr: Expr::sum(vec![Expr::variable("y"), Expr::constant(100.0)]),
            description: "x = y + 100".into(),
            source: "test".into(),
        };

        let problem = OptimizationProblem {
            objective: Expr::variable("x"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![dv("x", vec![1.0, 2.0, 3.0]), dv("y", vec![1.0, 2.0, 3.0])],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![binding_fixed, binding_decision],
        };

        let sol = EnumerationSolver.solve(&problem).unwrap();
        assert!(sol.feasible, "expected feasible solution");
        assert_eq!(
            sol.assignments[&var("x")],
            1.0,
            "decision variable x must keep its domain value; expected x=1, got {}",
            sol.assignments[&var("x")]
        );
        assert_eq!(
            sol.objective_value, 1.0,
            "objective must be decision x=1, not binding result; expected 1, got {}",
            sol.objective_value
        );
    }

    /// Regression (Scenario C): when the SAME target T appears in both a fixed
    /// binding and a decision-dependent binding, and the FIXED binding is listed
    /// FIRST, the fixed value must win — list-order priority must not be reversed.
    ///
    /// Setup:
    ///   decision variable: y ∈ {1, 2, 3}
    ///   binding (first):  T = 100          (fixed literal — listed first)
    ///   binding (second): T = y + 0        (decision-dependent — listed second)
    ///   objective: T                       (minimize)
    ///
    /// Expected: feasible=true, objective=100.0.
    /// Buggy behaviour (old patch): scratch.remove(T) stripped the pre-seeded
    /// value for every contested target, so the decision binding always won
    /// regardless of order → objective=1 instead of 100.
    #[test]
    fn binding_order_fixed_wins_over_decision_dependent_when_listed_first() {
        use yevice_core::cost::VariableBinding;

        let binding_fixed = VariableBinding {
            target: var("T"),
            expr: Expr::constant(100.0),
            description: "T = 100".into(),
            source: "test".into(),
        };
        let binding_decision = VariableBinding {
            target: var("T"),
            expr: Expr::sum(vec![Expr::variable("y"), Expr::constant(0.0)]),
            description: "T = y + 0".into(),
            source: "test".into(),
        };

        let problem = OptimizationProblem {
            objective: Expr::variable("T"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![dv("y", vec![1.0, 2.0, 3.0])],
            constraints: vec![],
            fixed_params: HashMap::new(),
            // Fixed binding listed first → fixed value must win.
            bindings: vec![binding_fixed, binding_decision],
        };

        let sol = EnumerationSolver.solve(&problem).unwrap();
        assert!(sol.feasible, "expected feasible solution");
        assert_eq!(
            sol.objective_value, 100.0,
            "fixed binding (T = 100) listed first must win over decision binding (T = y+0); \
             expected T=100, got {}",
            sol.objective_value
        );
    }

    /// Regression: when a contested-first target's decision-dependent binding is
    /// UNRESOLVABLE (references an undefined variable), the solver must fall back
    /// to the fixed binding's value rather than leaving the target absent.
    ///
    /// Setup:
    ///   decision variable: x ∈ {1, 2, 3}
    ///   binding (first):  z = x + missing_param   (decision-dep, UNRESOLVABLE)
    ///   binding (second): z = 5                   (fixed fallback)
    ///   objective: z                              (minimize)
    ///
    /// OLD single-pass semantics: first resolvable binding wins → z = 5.
    /// Expected: feasible=true, objective≈5.0, assignments\["z"\] is absent
    ///   (z is not a decision variable, so it won't be in assignments) but
    ///   the objective evaluates via scratch to 5.0.
    /// Buggy behaviour (before this fix): z absent after unresolvable decision
    ///   binding; objective eval fails; evaluation_failures=3/3, feasible=false.
    #[test]
    fn contested_decision_first_falls_back_to_fixed_when_decision_unresolvable() {
        use yevice_core::cost::VariableBinding;

        // First binding: z = x + missing_param  (decision-dep; missing_param undefined)
        let binding_unresolvable = VariableBinding {
            target: var("z"),
            expr: Expr::sum(vec![Expr::variable("x"), Expr::variable("missing_param")]),
            description: "z = x + missing_param".into(),
            source: "test".into(),
        };
        // Second binding: z = 5  (fixed literal fallback)
        let binding_fixed = VariableBinding {
            target: var("z"),
            expr: Expr::constant(5.0),
            description: "z = 5".into(),
            source: "test".into(),
        };

        let problem = OptimizationProblem {
            objective: Expr::variable("z"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![dv("x", vec![1.0, 2.0, 3.0])],
            constraints: vec![],
            fixed_params: HashMap::new(),
            // Decision-dep binding listed first, fixed fallback listed second.
            bindings: vec![binding_unresolvable, binding_fixed],
        };

        let sol = EnumerationSolver.solve(&problem).unwrap();
        assert!(
            sol.feasible,
            "expected feasible: fixed fallback z=5 must apply when decision binding is unresolvable; \
             got evaluation_failures={}/{}",
            sol.evaluation_failures, sol.total_combinations,
        );
        assert!(
            (sol.objective_value - 5.0).abs() < 1e-9,
            "expected objective=5.0 (from fixed fallback z=5), got {}",
            sol.objective_value
        );
    }

    // -----------------------------------------------------------------------
    // Multi-data table-driven test: priority-collision space
    // -----------------------------------------------------------------------

    /// Table-driven test covering 8 binding-priority scenarios.
    ///
    /// For each case the output of [`EnumerationSolver`] is validated against a
    /// naive brute-force oracle (`solve_naive`) that re-evaluates every
    /// combination from scratch using the single-pass [`resolve_bindings`]
    /// semantics, and also against a hard-coded expected objective value that
    /// was computed by hand (reasoning documented inline).
    #[test]
    fn multi_data_priority_collision_table() {
        use yevice_core::cost::VariableBinding;
        use yevice_core::evaluate::resolve_bindings;

        /// Brute-force oracle: enumerate every combination of decision-variable
        /// values, build the full params map (`fixed_params` + chosen values),
        /// call `resolve_bindings` (single-pass, source of truth), check
        /// feasibility, evaluate objective, track best.
        fn solve_naive(problem: &OptimizationProblem) -> Solution {
            let vars = &problem.decision_variables;
            if vars.iter().any(|dv| dv.domain.is_empty()) {
                return Solution {
                    assignments: HashMap::new(),
                    objective_value: f64::NAN,
                    feasible: false,
                    evaluation_failures: 0,
                    total_combinations: 0,
                    first_evaluation_error: None,
                };
            }

            // Build enumeration indices.
            let n = vars.len();
            let mut indices = vec![0usize; n];
            let mut best: Option<Solution> = None;

            'outer: loop {
                // Build params: fixed_params + current decision-variable selection.
                let mut params: evaluate::Params = problem
                    .fixed_params
                    .iter()
                    .map(|(k, v)| (k.clone(), *v))
                    .collect();
                for k in 0..n {
                    params.insert(vars[k].name.clone(), vars[k].domain[indices[k]]);
                }

                // Resolve all bindings in one pass using the oracle.
                let resolved = resolve_bindings(&problem.bindings, &params).unwrap();

                // Check feasibility.
                if !is_feasible(problem, &resolved) {
                    // Advance indices.
                    let mut carry = 1;
                    for k in (0..n).rev() {
                        if carry == 0 {
                            break;
                        }
                        indices[k] += carry;
                        if indices[k] < vars[k].domain.len() {
                            carry = 0;
                        } else {
                            indices[k] = 0;
                            carry = 1;
                        }
                    }
                    if carry == 1 {
                        break 'outer;
                    }
                    continue;
                }

                // Evaluate objective.
                if let Ok(obj) = evaluate::evaluate(&problem.objective, &resolved) {
                    let better = match &best {
                        None => true,
                        Some(cur) => match problem.direction {
                            ObjectiveDirection::Minimize => obj < cur.objective_value,
                            ObjectiveDirection::Maximize => obj > cur.objective_value,
                        },
                    };
                    if better {
                        let assignments: HashMap<VariableName, f64> = vars
                            .iter()
                            .map(|dv| {
                                (
                                    dv.name.clone(),
                                    vars[vars.iter().position(|v| v.name == dv.name).unwrap()]
                                        .domain[indices
                                        [vars.iter().position(|v| v.name == dv.name).unwrap()]],
                                )
                            })
                            .collect();
                        best = Some(Solution {
                            assignments,
                            objective_value: obj,
                            feasible: true,
                            evaluation_failures: 0,
                            total_combinations: 0,
                            first_evaluation_error: None,
                        });
                    }
                }

                // Advance indices.
                let mut carry = 1;
                for k in (0..n).rev() {
                    if carry == 0 {
                        break;
                    }
                    indices[k] += carry;
                    if indices[k] < vars[k].domain.len() {
                        carry = 0;
                    } else {
                        indices[k] = 0;
                        carry = 1;
                    }
                }
                if carry == 1 {
                    break 'outer;
                }
            }

            best.unwrap_or(Solution {
                assignments: HashMap::new(),
                objective_value: f64::NAN,
                feasible: false,
                evaluation_failures: 0,
                total_combinations: 0,
                first_evaluation_error: None,
            })
        }

        // Helper to build a VariableBinding concisely.
        let binding = |target: &str, expr: Expr| VariableBinding {
            target: var(target),
            expr,
            description: String::new(),
            source: "test".into(),
        };

        // -----------------------------------------------------------------------
        // Case definitions: (name, problem, expected_objective)
        // -----------------------------------------------------------------------

        // Case 1: empty_bindings
        // y ∈ {1,2,3}, no bindings, minimize y.
        // No bindings — objective is directly y. min(y) = 1.
        let case1 = (
            "empty_bindings",
            OptimizationProblem {
                objective: Expr::variable("y"),
                direction: ObjectiveDirection::Minimize,
                decision_variables: vec![dv("y", vec![1.0, 2.0, 3.0])],
                constraints: vec![],
                fixed_params: HashMap::new(),
                bindings: vec![],
            },
            1.0_f64, // min(y domain) = 1
        );

        // Case 2: same_target_fixed_first_decision_second
        // y ∈ {1,2,3}; bindings: [(T=100, fixed), (T=y+0, decision)]; minimize T.
        // First binding for T is T=100 (fixed, no decision-var refs).
        // T=100 is pre-seeded into base; decision binding skipped (T already in scratch).
        // resolve_bindings oracle: T=100 wins (contains_key skip on second binding).
        // Expected obj = 100.
        let case2 = (
            "same_target_fixed_first_decision_second",
            OptimizationProblem {
                objective: Expr::variable("T"),
                direction: ObjectiveDirection::Minimize,
                decision_variables: vec![dv("y", vec![1.0, 2.0, 3.0])],
                constraints: vec![],
                fixed_params: HashMap::new(),
                bindings: vec![
                    binding("T", Expr::constant(100.0)),
                    binding(
                        "T",
                        Expr::sum(vec![Expr::variable("y"), Expr::constant(0.0)]),
                    ),
                ],
            },
            100.0, // fixed T=100 wins (listed first)
        );

        // Case 3: same_target_decision_first_fixed_second
        // y ∈ {1,2,3}; bindings: [(T=y+0, decision), (T=100, fixed)]; minimize T.
        // T is in contested_decision_first (first binding is decision-dependent,
        // T is neither a fixed_param nor a decision var). T is removed from scratch
        // before decision_bindings run → T = y + 0 = y. min(y) = 1.
        // resolve_bindings oracle: first resolvable wins → for each y value, T=y
        // (decision binding is first; y is in params so it resolves immediately).
        // Expected obj = 1.
        let case3 = (
            "same_target_decision_first_fixed_second",
            OptimizationProblem {
                objective: Expr::variable("T"),
                direction: ObjectiveDirection::Minimize,
                decision_variables: vec![dv("y", vec![1.0, 2.0, 3.0])],
                constraints: vec![],
                fixed_params: HashMap::new(),
                bindings: vec![
                    binding(
                        "T",
                        Expr::sum(vec![Expr::variable("y"), Expr::constant(0.0)]),
                    ),
                    binding("T", Expr::constant(100.0)),
                ],
            },
            1.0, // decision-dependent binding (T=y+0) listed first wins; min(y)=1
        );

        // Case 4: three_same_target_bindings_mixed
        // y ∈ {1,2,3}; bindings: [(T=100,fixed), (T=y*10,decision), (T=50,fixed)]; minimize T.
        // First binding for T is T=100 (fixed), so T=100 is pre-seeded into base.
        // Subsequent bindings for T are skipped (contains_key). T=100 always.
        // resolve_bindings oracle: T=100 wins (first resolvable; 100 is constant, resolves first).
        // Expected obj = 100.
        let case4 = (
            "three_same_target_bindings_mixed",
            OptimizationProblem {
                objective: Expr::variable("T"),
                direction: ObjectiveDirection::Minimize,
                decision_variables: vec![dv("y", vec![1.0, 2.0, 3.0])],
                constraints: vec![],
                fixed_params: HashMap::new(),
                bindings: vec![
                    binding("T", Expr::constant(100.0)),
                    binding(
                        "T",
                        Expr::product(vec![Expr::variable("y"), Expr::constant(10.0)]),
                    ),
                    binding("T", Expr::constant(50.0)),
                ],
            },
            100.0, // first binding (T=100) wins; all subsequent T-bindings skipped
        );

        // Case 5: contested_target_equals_fixed_param
        // fixed_params={x:99.0}; y ∈ {1,2,3}; bindings: [(x=50,fixed),(x=y+100,decision)]; minimize x.
        // x is a fixed_param key → excluded from contested_decision_first.
        // base already holds x=99 from fixed_params. Both bindings skipped (x already present).
        // resolve_bindings oracle: x=99 (in base_params; contains_key skips both bindings).
        // Expected obj = 99.
        let case5_fixed = {
            let mut fp = HashMap::new();
            fp.insert(var("x"), 99.0);
            fp
        };
        let case5 = (
            "contested_target_equals_fixed_param",
            OptimizationProblem {
                objective: Expr::variable("x"),
                direction: ObjectiveDirection::Minimize,
                decision_variables: vec![dv("y", vec![1.0, 2.0, 3.0])],
                constraints: vec![],
                fixed_params: case5_fixed,
                bindings: vec![
                    binding("x", Expr::constant(50.0)),
                    binding(
                        "x",
                        Expr::sum(vec![Expr::variable("y"), Expr::constant(100.0)]),
                    ),
                ],
            },
            99.0, // fixed_param x=99 wins; both bindings skipped
        );

        // Case 6: contested_target_equals_decision_variable
        // decision_vars: x ∈ {1,2,3}, y ∈ {1,2,3}; bindings: [(x=50,fixed),(x=y+100,decision)]; minimize x.
        // x is a decision variable → hardening moves x=50 binding to decision_bindings.
        // x slot value (written by inner loop) is pre-seeded into scratch; resolve_bindings_into
        // skips x (already present). Decision slot x wins.
        // resolve_bindings oracle: x already in params (decision value); both bindings skipped.
        // min(x domain) = 1. Expected obj = 1, assignments['x'] = 1.0.
        let case6 = (
            "contested_target_equals_decision_variable",
            OptimizationProblem {
                objective: Expr::variable("x"),
                direction: ObjectiveDirection::Minimize,
                decision_variables: vec![
                    dv("x", vec![1.0, 2.0, 3.0]),
                    dv("y", vec![1.0, 2.0, 3.0]),
                ],
                constraints: vec![],
                fixed_params: HashMap::new(),
                bindings: vec![
                    binding("x", Expr::constant(50.0)),
                    binding(
                        "x",
                        Expr::sum(vec![Expr::variable("y"), Expr::constant(100.0)]),
                    ),
                ],
            },
            1.0, // decision variable x slot wins; min(x domain)=1
        );

        // Case 7: chained_decision_bindings
        // y ∈ {1,2,3}; bindings: [(T2=y,decision),(T1=T2+1,decision)]; minimize T1.
        // T2=y (decision-dependent), T1=T2+1 (decision-dependent, chains through T2).
        // At y=1: T2=1, T1=2; at y=2: T2=2, T1=3; at y=3: T2=3, T1=4.
        // min T1 = 2 (at y=1).
        // resolve_bindings oracle: T2=y, then T1=T2+1=y+1. min at y=1 → T1=2.
        // Expected obj = 2.
        let case7 = (
            "chained_decision_bindings",
            OptimizationProblem {
                objective: Expr::variable("T1"),
                direction: ObjectiveDirection::Minimize,
                decision_variables: vec![dv("y", vec![1.0, 2.0, 3.0])],
                constraints: vec![],
                fixed_params: HashMap::new(),
                bindings: vec![
                    binding("T2", Expr::variable("y")),
                    binding(
                        "T1",
                        Expr::sum(vec![Expr::variable("T2"), Expr::constant(1.0)]),
                    ),
                ],
            },
            2.0, // T1 = y + 1; min at y=1 gives T1=2
        );

        // Case 8: empty_expr_vars_binding_targets_decision_var
        // x ∈ {5,10,15}; bindings: [(x=50, fixed)]; minimize x.
        // x is a decision variable → hardening moves x=50 binding to decision_bindings.
        // x slot written by inner loop is in scratch; resolve_bindings_into skips x.
        // Decision slot wins. min(x domain) = 5.
        // resolve_bindings oracle: x already in params (decision value 5/10/15); binding skipped.
        // Expected obj = 5, assignments['x'] = 5.0.
        let case8 = (
            "empty_expr_vars_binding_targets_decision_var",
            OptimizationProblem {
                objective: Expr::variable("x"),
                direction: ObjectiveDirection::Minimize,
                decision_variables: vec![dv("x", vec![5.0, 10.0, 15.0])],
                constraints: vec![],
                fixed_params: HashMap::new(),
                bindings: vec![binding("x", Expr::constant(50.0))],
            },
            5.0, // decision variable x slot wins; min(x domain)=5
        );

        let cases: Vec<(&str, OptimizationProblem, f64)> =
            vec![case1, case2, case3, case4, case5, case6, case7, case8];

        let solver = EnumerationSolver;

        for (name, problem, expected_obj) in &cases {
            let solver_sol = solver
                .solve(problem)
                .unwrap_or_else(|e| panic!("case '{name}': solver returned error: {e:?}"));
            let naive_sol = solve_naive(problem);

            // Feasibility must agree.
            assert!(
                solver_sol.feasible == naive_sol.feasible,
                "case '{name}': feasibility mismatch — solver={} naive={}",
                solver_sol.feasible,
                naive_sol.feasible
            );

            if solver_sol.feasible {
                // Objective must match naive oracle within epsilon.
                assert!(
                    (solver_sol.objective_value - naive_sol.objective_value).abs() < 1e-9,
                    "case '{name}': objective mismatch vs naive — solver={} naive={}",
                    solver_sol.objective_value,
                    naive_sol.objective_value
                );

                // Objective must match hand-computed expected value within epsilon.
                assert!(
                    (solver_sol.objective_value - expected_obj).abs() < 1e-9,
                    "case '{name}': objective mismatch vs expected — solver={} expected={}",
                    solver_sol.objective_value,
                    expected_obj
                );

                // Assignment map must agree with naive on every decision variable.
                for dv_var in &problem.decision_variables {
                    let solver_val = solver_sol
                        .assignments
                        .get(&dv_var.name)
                        .copied()
                        .unwrap_or_else(|| {
                            panic!(
                                "case '{name}': solver missing assignment for '{}'",
                                dv_var.name
                            )
                        });
                    let naive_val = naive_sol
                        .assignments
                        .get(&dv_var.name)
                        .copied()
                        .unwrap_or_else(|| {
                            panic!(
                                "case '{name}': naive missing assignment for '{}'",
                                dv_var.name
                            )
                        });
                    assert!(
                        (solver_val - naive_val).abs() < 1e-9,
                        "case '{name}': assignment mismatch for '{}' — solver={} naive={}",
                        dv_var.name,
                        solver_val,
                        naive_val
                    );
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // MAX_COMBINATIONS boundary tests
    // -----------------------------------------------------------------------

    /// Domain product exactly equal to MAX_COMBINATIONS must succeed (Ok).
    #[test]
    fn max_combinations_boundary_exact_ok() {
        // 1000 * 1000 = 1_000_000 = MAX_COMBINATIONS → must not error.
        let dvs = vec![
            dv("x", (0..1000).map(f64::from).collect()),
            dv("y", (0..1000).map(f64::from).collect()),
        ];

        let problem = problem_with(
            Expr::constant(0.0),
            ObjectiveDirection::Minimize,
            dvs,
            vec![],
            HashMap::new(),
        );

        let result = EnumerationSolver.solve(&problem);
        assert!(
            result.is_ok(),
            "exactly MAX_COMBINATIONS must be Ok, got {result:?}"
        );
        assert!(result.unwrap().feasible);
    }

    /// Domain product of MAX_COMBINATIONS + 1 must return TooManyCombinations.
    #[test]
    fn max_combinations_boundary_plus_one_errors() {
        // 1000 * 1001 = 1_001_000 > MAX_COMBINATIONS → must error.
        let dvs = vec![
            dv("x", (0..1000).map(f64::from).collect()),
            dv("y", (0..1001).map(f64::from).collect()),
        ];

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
            "MAX_COMBINATIONS+1 must be TooManyCombinations, got {result:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Binding error skip / stale-value leak test
    // -----------------------------------------------------------------------

    /// When a decision-dependent binding evaluates to an error for some domain
    /// values (e.g. division by zero), those rows are treated as infeasible and
    /// skipped.  The stale binding result from a previous row must not leak into
    /// the evaluation of the next row.
    ///
    /// Setup:
    ///   decision variable: x ∈ {1, 2, 3}
    ///   binding: safe = 10.0 / (x - 2)     (undefined / error at x=2)
    ///   objective: safe                     (minimize)
    ///
    /// At x=1: safe = 10 / (1-2) = -10  (feasible)
    /// At x=2: safe = 10 / 0     → error (infeasible/skipped)
    /// At x=3: safe = 10 / (3-2) = 10   (feasible)
    ///
    /// Minimize: x=1 gives safe=-10 (global minimum), so solver must return
    /// objective=-10 and x=1.  If a stale value from x=3 leaked into x=1's
    /// evaluation the objective would be 10 instead.
    #[test]
    fn binding_error_skip_no_stale_leak() {
        use yevice_core::cost::VariableBinding;

        // binding: safe = 10 / (x - 2)
        let binding = VariableBinding {
            target: var("safe"),
            expr: Expr::div(
                Expr::constant(10.0),
                Expr::sum(vec![Expr::variable("x"), Expr::constant(-2.0)]),
            ),
            description: "safe = 10 / (x - 2)".into(),
            source: "test".into(),
        };

        let problem = OptimizationProblem {
            objective: Expr::variable("safe"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![dv("x", vec![1.0, 2.0, 3.0])],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![binding],
        };

        let sol = EnumerationSolver.solve(&problem).unwrap();
        assert!(
            sol.feasible,
            "expected feasible: x=1 and x=3 are valid rows"
        );
        // x=1 gives safe=-10 (minimum); x=3 gives safe=10.
        // x=2 is skipped (division by zero in binding).
        assert_eq!(
            sol.assignments[&var("x")],
            1.0,
            "expected x=1 (gives minimum safe=-10), got x={}",
            sol.assignments[&var("x")]
        );
        assert!(
            (sol.objective_value - (-10.0)).abs() < 1e-9,
            "expected objective=-10, got {}",
            sol.objective_value
        );
    }

    // -----------------------------------------------------------------------
    // Diagnostic field tests (Fix 3)
    // -----------------------------------------------------------------------

    /// When the objective fails to evaluate for every combination (here:
    /// division by zero — the denominator `x - x` is always 0), the solver
    /// must return:
    ///   - feasible = false
    ///   - evaluation_failures == total_combinations
    ///   - first_evaluation_error = Some(..)
    ///
    /// This distinguishes "all evaluations errored" from genuine infeasibility.
    /// (An objective referencing an *unbound* variable is rejected up-front by
    /// `validate_bindings` instead — see the validate_bindings tests.)
    #[test]
    fn all_evaluations_fail_reports_diagnostic_fields() {
        // objective = 1 / (x + (-1 * x)) = 1 / 0 for every x
        // domain x ∈ {1.0, 2.0, 3.0} → 3 combinations, all fail to evaluate
        let objective = Expr::div(
            Expr::constant(1.0),
            Expr::sum(vec![
                Expr::variable("x"),
                Expr::product(vec![Expr::constant(-1.0), Expr::variable("x")]),
            ]),
        );
        let problem = problem_with(
            objective,
            ObjectiveDirection::Minimize,
            vec![dv("x", vec![1.0, 2.0, 3.0])],
            vec![],
            HashMap::new(),
        );

        let sol = EnumerationSolver.solve(&problem).unwrap();
        assert!(
            !sol.feasible,
            "expected infeasible when all objective evaluations fail"
        );
        assert_eq!(
            sol.evaluation_failures, 3,
            "expected 3 evaluation failures (one per combination), got {}",
            sol.evaluation_failures
        );
        assert_eq!(
            sol.total_combinations, 3,
            "expected total_combinations=3, got {}",
            sol.total_combinations
        );
        assert!(
            sol.first_evaluation_error.is_some(),
            "expected first_evaluation_error to be Some(..) when all evaluations fail"
        );
    }

    // -----------------------------------------------------------------------
    // Regression test for proptest counterexample: transitive dependency
    // -----------------------------------------------------------------------

    /// Regression: discovered by proptest — a fixed binding `b = 2` referenced
    /// by an earlier decision-dependent binding `T1 = c * b` must NOT be hoisted
    /// before the earlier binding has had a chance to be skipped for
    /// undefined-variable.
    ///
    /// Per resolve_bindings's fixed-point + contains_key semantics:
    ///   pass 1: T1=c*b skipped (b undefined), T1=1 inserts, b=2 inserts
    ///   pass 2: all targets already set, no progress
    ///   final T1 = 1.
    ///
    /// The old hoist optimization would pre-seed `b=2` into base before the
    /// loop, causing T1=c*b to resolve in pass 1 and skip T1=1, yielding T1=2
    /// (wrong).
    #[test]
    fn transitive_dependency_does_not_corrupt_binding_order() {
        use yevice_core::cost::VariableBinding;

        let problem = OptimizationProblem {
            decision_variables: vec![dv("c", vec![1.0])],
            fixed_params: HashMap::new(),
            bindings: vec![
                VariableBinding {
                    target: var("T1"),
                    expr: Expr::product(vec![Expr::variable("c"), Expr::variable("b")]),
                    description: "T1 = c * b".into(),
                    source: "test".into(),
                },
                VariableBinding {
                    target: var("T1"),
                    expr: Expr::constant(1.0),
                    description: "T1 = 1".into(),
                    source: "test".into(),
                },
                VariableBinding {
                    target: var("b"),
                    expr: Expr::constant(2.0),
                    description: "b = 2".into(),
                    source: "test".into(),
                },
            ],
            constraints: vec![],
            objective: Expr::variable("T1"),
            direction: ObjectiveDirection::Minimize,
        };

        let result = EnumerationSolver.solve(&problem).expect("solve");
        assert!(result.feasible);
        assert!(
            (result.objective_value - 1.0).abs() < 1e-9,
            "T1 must be 1 (first-pass resolvable wins under fixed-point semantics), got {}",
            result.objective_value
        );
    }

    // -----------------------------------------------------------------------
    // validate_bindings tests
    // -----------------------------------------------------------------------

    /// An objective variable bound by nothing must be rejected with an error
    /// that names it.
    #[test]
    fn validate_bindings_rejects_unbound_objective_variable() {
        let problem = problem_with(
            Expr::variable("undefined_var"),
            ObjectiveDirection::Minimize,
            vec![dv("x", vec![1.0, 2.0, 3.0])],
            vec![],
            HashMap::new(),
        );

        let err = validate_bindings(&problem).unwrap_err();
        match &err {
            SolverError::UnboundVariables { variables } => {
                assert_eq!(variables, &vec!["undefined_var".to_string()]);
            }
            other => panic!("expected UnboundVariables, got {other:?}"),
        }
        assert!(
            err.to_string().contains("undefined_var"),
            "error message must name the unbound variable: {err}"
        );
    }

    /// A binding whose source variables are all bound (transitively, through
    /// chained bindings) satisfies the objective variable.
    #[test]
    fn validate_bindings_accepts_transitively_bound_variable() {
        use yevice_core::cost::VariableBinding;

        // x (decision) → T2 = x → T1 = T2 + 1; objective references T1.
        let problem = OptimizationProblem {
            objective: Expr::variable("T1"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![dv("x", vec![1.0, 2.0])],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![
                VariableBinding {
                    target: var("T1"),
                    expr: Expr::sum(vec![Expr::variable("T2"), Expr::constant(1.0)]),
                    description: String::new(),
                    source: "test".into(),
                },
                VariableBinding {
                    target: var("T2"),
                    expr: Expr::variable("x"),
                    description: String::new(),
                    source: "test".into(),
                },
            ],
        };

        assert!(validate_bindings(&problem).is_ok());
    }

    /// A binding whose own source variable is missing must NOT mask the
    /// unbound objective variable.
    #[test]
    fn validate_bindings_rejects_binding_with_unbound_source() {
        use yevice_core::cost::VariableBinding;

        // binding: derived = missing * 0.01; objective references derived.
        let problem = OptimizationProblem {
            objective: Expr::variable("derived"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![VariableBinding {
                target: var("derived"),
                expr: Expr::product(vec![Expr::variable("missing"), Expr::constant(0.01)]),
                description: String::new(),
                source: "test".into(),
            }],
        };

        let err = validate_bindings(&problem).unwrap_err();
        assert!(
            matches!(&err, SolverError::UnboundVariables { variables } if variables == &vec!["derived".to_string()]),
            "expected UnboundVariables naming 'derived', got {err:?}"
        );
    }

    // -----------------------------------------------------------------------
    // prune_domains tests
    // -----------------------------------------------------------------------

    /// Single-variable Ge constraint shrinks the domain to feasible values
    /// and the resulting solver run still returns the correct optimum.
    #[test]
    fn prune_domains_drops_infeasible_single_variable_values() {
        // constraint: x >= 3, domain {1, 2, 3, 4, 5} → pruned {3, 4, 5}.
        let constraint = OptimizationConstraint {
            lhs: Expr::variable("x"),
            relation: Relation::Ge,
            rhs: 3.0,
            label: None,
        };

        let problem = problem_with(
            Expr::variable("x"),
            ObjectiveDirection::Minimize,
            vec![dv("x", vec![1.0, 2.0, 3.0, 4.0, 5.0])],
            vec![constraint],
            HashMap::new(),
        );

        let pruned = prune_domains(&problem).expect("expected feasible pruning");
        assert_eq!(pruned.len(), 1);
        assert_eq!(pruned[0].domain, vec![3.0, 4.0, 5.0]);
    }

    /// When every domain value violates a single-variable constraint, the
    /// pruner reports `SolverError::Infeasible`, and `EnumerationSolver::solve`
    /// translates that into a non-feasible `Solution`.
    #[test]
    fn prune_domains_infeasible_when_all_values_eliminated() {
        // constraint: x >= 100, domain {1, 2, 3} → all values pruned.
        let constraint = OptimizationConstraint {
            lhs: Expr::variable("x"),
            relation: Relation::Ge,
            rhs: 100.0,
            label: None,
        };

        let problem = problem_with(
            Expr::variable("x"),
            ObjectiveDirection::Minimize,
            vec![dv("x", vec![1.0, 2.0, 3.0])],
            vec![constraint],
            HashMap::new(),
        );

        let err = prune_domains(&problem).unwrap_err();
        assert!(
            matches!(err, SolverError::Infeasible),
            "expected Infeasible, got {err:?}"
        );

        // And the solver as a whole should surface a non-feasible Solution.
        let sol = EnumerationSolver.solve(&problem).unwrap();
        assert!(!sol.feasible);
    }

    /// Multi-variable constraints must NOT be touched by the pruner — values
    /// that *individually* look invalid may become feasible once another
    /// decision variable's value is chosen.
    #[test]
    fn prune_domains_leaves_multi_variable_constraints_alone() {
        // constraint: x + y <= 5
        // domain x = {1, 2, 3, 4, 5}, y = {1, 2, 3, 4, 5}
        // No single-variable pruning is valid here: for any x in {1..=4} there
        // is some y making it feasible. The pruner must leave both domains
        // untouched.
        let constraint = OptimizationConstraint {
            lhs: Expr::sum(vec![Expr::variable("x"), Expr::variable("y")]),
            relation: Relation::Le,
            rhs: 5.0,
            label: None,
        };

        let problem = problem_with(
            Expr::sum(vec![Expr::variable("x"), Expr::variable("y")]),
            ObjectiveDirection::Minimize,
            vec![
                dv("x", vec![1.0, 2.0, 3.0, 4.0, 5.0]),
                dv("y", vec![1.0, 2.0, 3.0, 4.0, 5.0]),
            ],
            vec![constraint],
            HashMap::new(),
        );

        let pruned = prune_domains(&problem).expect("expected feasible pruning");
        assert_eq!(pruned.len(), 2);
        assert_eq!(pruned[0].domain, vec![1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!(pruned[1].domain, vec![1.0, 2.0, 3.0, 4.0, 5.0]);
    }

    /// Pruning is sound: a problem with a Ge constraint on a single variable
    /// must still return the same optimum as without pruning.
    #[test]
    fn prune_preserves_solver_optimum() {
        // objective = 10 * x;  x ∈ {1, 2, 3, 4, 5};  constraint: x >= 3
        // Optimum: x=3, objective=30.
        let objective = Expr::product(vec![Expr::constant(10.0), Expr::variable("x")]);
        let constraint = OptimizationConstraint {
            lhs: Expr::variable("x"),
            relation: Relation::Ge,
            rhs: 3.0,
            label: None,
        };

        let problem = problem_with(
            objective,
            ObjectiveDirection::Minimize,
            vec![dv("x", vec![1.0, 2.0, 3.0, 4.0, 5.0])],
            vec![constraint],
            HashMap::new(),
        );

        let sol = EnumerationSolver.solve(&problem).unwrap();
        assert!(sol.feasible);
        assert_eq!(sol.assignments[&var("x")], 3.0);
        assert_eq!(sol.objective_value, 30.0);
    }

    /// Pruning must not panic for domain products that overflow `u64`. The
    /// before/after diagnostic computes a saturating product so that an
    /// 11-variable × 1024-value problem (above 2^64) still goes through
    /// pruning cleanly.  This regression test exists to make sure the
    /// debug-mode arithmetic overflow check stays satisfied.
    #[test]
    fn prune_domains_saturates_huge_combination_count() {
        // 11 vars × domain size 1024 = 1024^11 ≈ 1.4e33 > 2^64.
        let dvs: Vec<DecisionVariable> = (0..11_u32)
            .map(|i| dv(&format!("x{i}"), (0..1024).map(f64::from).collect()))
            .collect();
        // Single-variable constraint on x0 so the pruning loop actually runs.
        let constraint = OptimizationConstraint {
            lhs: Expr::variable("x0"),
            relation: Relation::Le,
            rhs: 2.0_f64.powi(20),
            label: None,
        };

        let problem = problem_with(
            Expr::constant(0.0),
            ObjectiveDirection::Minimize,
            dvs,
            vec![constraint],
            HashMap::new(),
        );

        // Must not panic even though product(domains) overflows u64.
        let pruned = prune_domains(&problem).expect("expected pruning to succeed");
        assert_eq!(pruned.len(), 11);
    }

    /// A constraint depending on a single decision variable through a binding
    /// (e.g. `usage = factor * x`) must still be picked up by the pruner.
    #[test]
    fn prune_follows_binding_to_single_decision_variable() {
        use yevice_core::cost::VariableBinding;

        // binding: usage = factor * x  (factor is a fixed param)
        // constraint: usage <= 50
        // x ∈ {1..=10}, factor = 10  → x*10 <= 50  → x ∈ {1..=5}
        let binding = VariableBinding {
            target: var("usage"),
            expr: Expr::product(vec![Expr::variable("factor"), Expr::variable("x")]),
            description: "usage = factor * x".into(),
            source: "test".into(),
        };
        let constraint = OptimizationConstraint {
            lhs: Expr::variable("usage"),
            relation: Relation::Le,
            rhs: 50.0,
            label: None,
        };

        let mut fixed = HashMap::new();
        fixed.insert(var("factor"), 10.0);

        let problem = OptimizationProblem {
            objective: Expr::variable("usage"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![dv(
                "x",
                vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0],
            )],
            constraints: vec![constraint],
            fixed_params: fixed,
            bindings: vec![binding],
        };

        let pruned = prune_domains(&problem).expect("expected feasible pruning");
        assert_eq!(pruned.len(), 1);
        assert_eq!(pruned[0].domain, vec![1.0, 2.0, 3.0, 4.0, 5.0]);
    }

    /// Regression (codex comment 3407403956): when `decision_variables` contains
    /// two entries with the **same name** (`x={0}` and `x={10}`), the enumerator
    /// uses last-write-wins (the last slot's value wins each iteration), so
    /// `x=10` is the effective value.  A constraint `x >= 5` is therefore
    /// satisfiable.  The pruner must NOT apply the constraint to the first slot
    /// (`{0}`) in isolation, which would empty it and return `Infeasible` before
    /// the enumerator ever runs.
    ///
    /// Expected: `EnumerationSolver` finds a feasible solution (x=10).
    #[test]
    fn prune_domains_skips_duplicate_named_variables() {
        // Two decision-variable entries with the same name: x={0} and x={10}.
        // The enumerator last-write-wins → effective value is always from the
        // second slot (x=10).
        let constraint = OptimizationConstraint {
            lhs: Expr::variable("x"),
            relation: Relation::Ge,
            rhs: 5.0,
            label: None,
        };

        let problem = problem_with(
            Expr::variable("x"),
            ObjectiveDirection::Maximize,
            vec![dv("x", vec![0.0]), dv("x", vec![10.0])],
            vec![constraint],
            HashMap::new(),
        );

        // prune_domains must not empty the first slot and return Infeasible.
        let pruned =
            prune_domains(&problem).expect("expected feasible: duplicate name must be skipped");
        assert_eq!(pruned.len(), 2, "both slots must be preserved");

        // The full solver must also find a feasible solution via x=10.
        let sol = EnumerationSolver.solve(&problem).unwrap();
        assert!(
            sol.feasible,
            "expected feasible solution with duplicate x names and x>=5; \
             enumerator last-write-wins so x=10 satisfies the constraint"
        );
        assert_eq!(
            sol.assignments[&var("x")],
            10.0,
            "expected x=10 (last-write-wins from second slot)"
        );
    }

    // -----------------------------------------------------------------------
    // solver_from_name tests
    // -----------------------------------------------------------------------

    /// The `"enumeration"` name maps to a working solver.
    #[test]
    fn solver_from_name_enumeration_returns_working_solver() {
        let solver = solver_from_name("enumeration").expect("must return Ok");
        // Solve a trivial problem to exercise the boxed solver.
        let problem = problem_with(
            Expr::variable("x"),
            ObjectiveDirection::Minimize,
            vec![dv("x", vec![1.0, 2.0, 3.0])],
            vec![],
            HashMap::new(),
        );
        let sol = solver.solve(&problem).unwrap();
        assert!(sol.feasible);
        assert_eq!(sol.assignments[&var("x")], 1.0);
    }

    /// Unknown names return `UnknownSolver` listing the allowed values.
    #[test]
    fn solver_from_name_unknown_returns_error() {
        let result = solver_from_name("simplex");
        let err = match result {
            Ok(_) => panic!("expected UnknownSolver error"),
            Err(e) => e,
        };
        match &err {
            SolverError::UnknownSolver { requested, allowed } => {
                assert_eq!(requested, "simplex");
                assert!(allowed.contains(&"enumeration".to_string()));
            }
            other => panic!("expected UnknownSolver, got {other:?}"),
        }
        assert!(err.to_string().contains("enumeration"));
    }

    /// `EnumerationSolver::solve` must reject an unbound objective up-front
    /// instead of reporting an infeasible solution.
    #[test]
    fn solve_rejects_unbound_objective_variable_up_front() {
        let problem = problem_with(
            Expr::variable("undefined_var"),
            ObjectiveDirection::Minimize,
            vec![dv("x", vec![1.0, 2.0, 3.0])],
            vec![],
            HashMap::new(),
        );

        let result = EnumerationSolver.solve(&problem);
        assert!(
            matches!(result, Err(SolverError::UnboundVariables { .. })),
            "expected UnboundVariables error, got {result:?}"
        );
    }
}
