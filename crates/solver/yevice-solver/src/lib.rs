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

        // -----------------------------------------------------------------------
        // Pre-loop: partition bindings into those independent of decision
        // variables and those that may depend on them.
        // -----------------------------------------------------------------------

        // Collect all decision variable names into a "dependent" set, then
        // transitively expand it: a binding whose expression references any
        // dependent variable makes its *target* dependent too.
        let decision_names: std::collections::HashSet<&VariableName> =
            vars.iter().map(|dv| &dv.name).collect();

        let mut dependent: std::collections::HashSet<&VariableName> =
            decision_names.iter().copied().collect();

        // Fixed-point expansion of the dependent set over bindings.
        loop {
            let mut changed = false;
            for binding in &problem.bindings {
                if dependent.contains(&binding.target) {
                    continue;
                }
                let expr_vars = binding.expr.variables();
                if expr_vars.iter().any(|v| dependent.contains(v)) {
                    dependent.insert(&binding.target);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        // Partition bindings: fixed_bindings are those whose expr variables are
        // entirely outside the dependent set (safe to resolve once).
        // decision_bindings must be re-resolved each iteration (original order kept).
        // Clone into owned Vecs so we can pass slices to resolve_bindings_into.
        let (fixed_bindings, decision_bindings): (Vec<_>, Vec<_>) = problem
            .bindings
            .iter()
            .cloned()
            .partition(|b| b.expr.variables().iter().all(|v| !dependent.contains(v)));

        // Compute the set of "contested" targets: targets that appear in BOTH
        // fixed_bindings AND decision_bindings.  For these targets the original
        // list-order priority is restored by removing the pre-resolved fixed
        // value from scratch before re-evaluating the decision-dependent binding
        // (see loop body below).
        let decision_binding_targets: std::collections::HashSet<&VariableName> =
            decision_bindings.iter().map(|b| &b.target).collect();
        let contested_targets: std::collections::HashSet<&VariableName> = fixed_bindings
            .iter()
            .filter(|b| decision_binding_targets.contains(&b.target))
            .map(|b| &b.target)
            .collect();

        // Build the base params map once: fixed_params + fixed bindings resolved.
        // Pre-insert slots for decision variables so that the inner loop can
        // update values via get_mut without cloning keys.
        //
        // NOTE: decision_binding targets are intentionally NOT pre-seeded here.
        // This preserves the correct priority contract:
        //   - fixed_param with same name as a binding target → fixed value wins
        //     (already in base; resolve_bindings_into skips keys already present).
        //   - decision variable with same name as a binding target → decision
        //     value wins (written by get_mut after clone_from; skipped by
        //     resolve_bindings_into).
        //   - pure binding target (neither fixed nor decision) → absent from
        //     base; clone_from removes any stale value; resolve_bindings_into
        //     recomputes it fresh each iteration.
        let mut base: Params = problem
            .fixed_params
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();

        // Resolve fixed bindings into base (these are decision-independent).
        // resolve_bindings_into does not fail — it absorbs errors internally
        // (unresolvable bindings are warned and skipped).
        resolve_bindings_into(&mut base, &fixed_bindings);

        // Pre-allocate slots for decision variables (value filled each iteration).
        // This pre-seed is required so that get_mut(&vars[k].name).expect(…)
        // in the inner loop can update values without a fresh allocation.
        for dv in vars {
            base.entry(dv.name.clone()).or_insert(0.0);
        }

        // Scratch buffer: reused every iteration via clone_from to avoid
        // allocating a new HashMap each time.  clone_from replaces all contents
        // of scratch with those of base (reusing the underlying allocation where
        // possible; each String key is still cloned, but no heap realloc occurs
        // as long as the load factor stays stable).
        let mut scratch = base.clone();

        let mut best: Option<Solution> = None;
        let mut evaluation_failures: u64 = 0;
        let mut first_evaluation_error: Option<String> = None;

        for i in 0..combination_count {
            // Restore scratch to the fixed base state (reuses existing allocation).
            // After this, scratch contains exactly the keys in base:
            //   - fixed_params values
            //   - fixed binding targets (already resolved)
            //   - decision variable slots (placeholder 0.0, overwritten below)
            // Pure decision_binding targets are NOT in scratch here (not pre-seeded).
            scratch.clone_from(&base);

            // Decode flat index i and write decision variable values directly
            // into scratch.  The slots were pre-seeded into base, so get_mut
            // always succeeds — the unwrap is an invariant, not a runtime check.
            for k in 0..n {
                let idx = ((i / weights[k]) as usize) % vars[k].domain.len();
                *scratch.get_mut(&vars[k].name).expect(
                    "decision variable slot pre-seeded into base before the enumeration loop",
                ) = vars[k].domain[idx];
            }

            // Restore list-order priority for contested targets: when the same
            // target appears in both fixed_bindings and decision_bindings, the
            // entry that comes first in problem.bindings must win — mirroring the
            // old single-pass `resolve_bindings(all_bindings, params)` semantics
            // where `contains_key` in resolve_bindings_into lets the first
            // resolvable binding win.
            //
            // After clone_from, scratch already holds the fixed-resolved value
            // for contested targets (written during pre-loop base construction).
            // Removing them here lets resolve_bindings_into recompute the
            // decision-dependent expression, so the decision-dependent binding
            // wins — which is correct when the decision-dependent binding appears
            // BEFORE the fixed binding in problem.bindings.
            //
            // Fixed params and decision variable slot values are NOT contested
            // (they are not binding targets), so they remain intact.
            for t in &contested_targets {
                scratch.remove(*t);
            }

            // Resolve decision-dependent bindings against the updated scratch.
            //
            // Priority contract enforced by resolve_bindings_into's
            // `contains_key` skip:
            //   - decision variable == binding target → scratch already holds the
            //     decision value (written above); resolve_bindings_into skips it
            //     → decision value wins (bug-2 fix).
            //   - fixed_param == binding target → base (and thus scratch after
            //     clone_from) already holds the fixed value; skipped → fixed
            //     value wins (bug-1 fix).
            //   - pure binding target → absent from scratch after clone_from
            //     (not pre-seeded); resolve_bindings_into inserts the fresh
            //     computed value → correct recomputation each iteration.
            //   - contested target → removed above; resolve_bindings_into
            //     inserts the decision-dependent value → list-order priority
            //     restored.
            //
            // resolve_bindings_into does not return a value — errors are absorbed
            // internally (division-by-zero or missing variables are warned and
            // the target is left absent, causing infeasibility below).
            resolve_bindings_into(&mut scratch, &decision_bindings);

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

    /// When the objective references an undefined variable for every combination,
    /// all evaluations fail.  The solver must return:
    ///   - feasible = false
    ///   - evaluation_failures == total_combinations
    ///   - first_evaluation_error = Some(..)
    ///
    /// This distinguishes "all evaluations errored" from genuine infeasibility.
    #[test]
    fn all_evaluations_fail_reports_diagnostic_fields() {
        // objective = undefined_var  (not in domain or fixed_params)
        // domain x ∈ {1.0, 2.0, 3.0} → 3 combinations, all fail to evaluate
        let problem = problem_with(
            Expr::variable("undefined_var"),
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
}
