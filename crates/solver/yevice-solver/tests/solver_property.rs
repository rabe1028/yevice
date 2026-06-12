//! Property-based tests for [`EnumerationSolver`].
//!
//! The key insight: the existing unit tests in `lib.rs` use hand-crafted
//! problems where decision-variable names never collide with binding targets.
//! This file generates problems that *do* produce such collisions (same name
//! in both a decision variable and a binding, same target in both a fixed and
//! a decision-dependent binding, etc.).  A naive oracle that single-pass
//! re-evaluates from scratch serves as the reference.
//!
//! # Multiple-optima note
//!
//! When several assignments yield the same minimum objective value, the
//! `EnumerationSolver` and the naive oracle may deterministically choose
//! *different* assignments (the solver uses mixed-radix enumeration order; the
//! oracle iterates in its own order).  We therefore assert only **objective
//! equality**, not assignment equality.  This is safe because both produce a
//! *valid* optimal — they are just different tie-break choices.

use std::collections::HashMap;

use proptest::prelude::*;
use yevice_core::cost::VariableBinding;
use yevice_core::evaluate::{self, Params};
use yevice_core::expr::Expr;
use yevice_core::optimize::{DecisionVariable, ObjectiveDirection, OptimizationProblem};
use yevice_core::types::VariableName;
use yevice_solver::{EnumerationSolver, Solution, Solver};

// ---------------------------------------------------------------------------
// Naive oracle
// ---------------------------------------------------------------------------

/// Single-pass brute-force reference solver.
///
/// Semantics: enumerate every combination of decision variable values in
/// domain-order (outer variable = first decision variable), build params from
/// `fixed_params`, insert each decision value, call `resolve_bindings`, skip
/// constraints (we keep constraints empty in the generator for simplicity),
/// evaluate the objective, track the minimum (or maximum).
///
/// This mirrors the *original* single-pass behaviour before the loop-invariant
/// binding hoist was added, so it is the ground truth for list-order priority.
fn solve_naive(problem: &OptimizationProblem) -> Solution {
    // Guard: empty domain → immediately infeasible.
    if problem
        .decision_variables
        .iter()
        .any(|dv| dv.domain.is_empty())
    {
        return Solution {
            assignments: HashMap::new(),
            objective_value: f64::NAN,
            feasible: false,
            evaluation_failures: 0,
            total_combinations: 0,
            first_evaluation_error: None,
        };
    }

    // Compute total combination count.
    let total_combinations: u64 = problem
        .decision_variables
        .iter()
        .map(|dv| dv.domain.len() as u64)
        .product();

    let vars = &problem.decision_variables;
    let n = vars.len();

    // Mixed-radix weights (identical logic to EnumerationSolver).
    let mut weights = vec![1u64; n.max(1)];
    for k in (0..n.saturating_sub(1)).rev() {
        weights[k] = weights[k + 1].saturating_mul(vars[k + 1].domain.len() as u64);
    }

    let mut best: Option<f64> = None;
    let mut best_assignments: HashMap<VariableName, f64> = HashMap::new();
    let mut evaluation_failures: u64 = 0;
    let mut first_evaluation_error: Option<String> = None;

    for i in 0..total_combinations {
        // Build params: fixed first, then decision values (decision wins on collision).
        let mut params: Params = problem
            .fixed_params
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();

        for k in 0..n {
            let idx = ((i / weights[k]) as usize) % vars[k].domain.len();
            params.insert(vars[k].name.clone(), vars[k].domain[idx]);
        }

        // Resolve bindings using the public single-pass API.  `resolve_bindings`
        // implements the "first resolvable wins" fixed-point semantics, which is
        // the canonical reference.
        let resolved = match evaluate::resolve_bindings(&problem.bindings, &params) {
            Ok(p) => p,
            Err(_) => continue,
        };

        // Evaluate objective.
        let obj = match evaluate::evaluate(&problem.objective, &resolved) {
            Ok(v) => v,
            Err(e) => {
                evaluation_failures += 1;
                if first_evaluation_error.is_none() {
                    first_evaluation_error = Some(format!("{e}"));
                }
                continue;
            }
        };

        let better = match best {
            None => true,
            Some(cur) => match problem.direction {
                ObjectiveDirection::Minimize => obj < cur,
                ObjectiveDirection::Maximize => obj > cur,
            },
        };

        if better {
            best = Some(obj);
            best_assignments = vars
                .iter()
                .enumerate()
                .map(|(k, dv)| {
                    let idx = ((i / weights[k]) as usize) % dv.domain.len();
                    (dv.name.clone(), dv.domain[idx])
                })
                .collect();
        }
    }

    match best {
        Some(obj) => Solution {
            assignments: best_assignments,
            objective_value: obj,
            feasible: true,
            evaluation_failures,
            total_combinations,
            first_evaluation_error,
        },
        None => Solution {
            assignments: HashMap::new(),
            objective_value: f64::NAN,
            feasible: false,
            evaluation_failures,
            total_combinations,
            first_evaluation_error,
        },
    }
}

// ---------------------------------------------------------------------------
// Generators
// ---------------------------------------------------------------------------

/// Small pool of variable names — kept intentionally short so that the
/// generator frequently produces *collisions* (same name used as both a
/// decision variable and a binding target, or the same target in multiple
/// bindings).  Collisions are the exact class of bug the property test is
/// designed to catch.
const VAR_POOL: &[&str] = &["a", "b", "c", "T1", "T2", "T3"];

fn arb_variable_name() -> impl Strategy<Value = VariableName> {
    prop::sample::select(VAR_POOL).prop_map(VariableName::new)
}

fn arb_domain() -> impl Strategy<Value = Vec<f64>> {
    // 1..=3 distinct integer values drawn from 1..=5.
    prop::collection::btree_set(1i32..=5, 1..=3)
        .prop_map(|s| s.into_iter().map(f64::from).collect())
}

fn arb_decision_var() -> impl Strategy<Value = DecisionVariable> {
    (arb_variable_name(), arb_domain()).prop_map(|(name, domain)| DecisionVariable { name, domain })
}

/// Deduplicate a `Vec<DecisionVariable>` by name: keep the last occurrence
/// (arbitrary but deterministic).
fn dedup_decision_vars(mut vs: Vec<DecisionVariable>) -> Vec<DecisionVariable> {
    // Walk from the end; keep track of seen names.
    let mut seen = std::collections::HashSet::new();
    vs.reverse();
    vs.retain(|dv| seen.insert(dv.name.clone()));
    vs.reverse();
    vs
}

fn arb_decision_vars() -> impl Strategy<Value = Vec<DecisionVariable>> {
    prop::collection::vec(arb_decision_var(), 1..=3).prop_map(dedup_decision_vars)
}

fn arb_fixed_params() -> impl Strategy<Value = HashMap<VariableName, f64>> {
    prop::collection::vec((arb_variable_name(), 1i32..=10), 0..=2)
        .prop_map(|kvs| kvs.into_iter().map(|(k, v)| (k, f64::from(v))).collect())
}

/// Leaf expression: either a constant (1..=10) or a variable from the pool.
fn arb_leaf() -> impl Strategy<Value = Expr> {
    prop_oneof![
        (1i32..=10).prop_map(|v| Expr::constant(f64::from(v))),
        arb_variable_name().prop_map(Expr::variable),
    ]
}

/// Simple expression: leaf, or a sum/product of exactly two leaves.
/// Depth is kept at 1–2 so that evaluation is fast and failures are rare.
fn arb_simple_expr() -> impl Strategy<Value = Expr> {
    prop_oneof![
        arb_leaf(),
        (arb_leaf(), arb_leaf()).prop_map(|(a, b)| Expr::sum(vec![a, b])),
        (arb_leaf(), arb_leaf()).prop_map(|(a, b)| Expr::product(vec![a, b])),
    ]
}

fn arb_binding() -> impl Strategy<Value = VariableBinding> {
    (arb_variable_name(), arb_simple_expr()).prop_map(|(target, expr)| VariableBinding {
        target,
        expr,
        description: String::new(),
        source: "proptest".into(),
    })
}

fn arb_direction() -> impl Strategy<Value = ObjectiveDirection> {
    prop_oneof![
        Just(ObjectiveDirection::Minimize),
        Just(ObjectiveDirection::Maximize),
    ]
}

fn arb_problem() -> impl Strategy<Value = OptimizationProblem> {
    (
        arb_decision_vars(),
        arb_fixed_params(),
        prop::collection::vec(arb_binding(), 0..=5),
        arb_simple_expr(),
        arb_direction(),
    )
        .prop_map(
            |(decision_variables, fixed_params, bindings, objective, direction)| {
                OptimizationProblem {
                    decision_variables,
                    fixed_params,
                    bindings,
                    constraints: vec![],
                    objective,
                    direction,
                }
            },
        )
}

fn arb_transitive_chain_problem() -> impl Strategy<Value = OptimizationProblem> {
    (
        arb_variable_name(), // chain_a (= chain_b)
        arb_variable_name(), // chain_b (= const_val)
        arb_variable_name(), // decision var name
        1i32..=5i32,         // const_val for chain_b
        arb_domain(),        // decision var domain
    )
        .prop_map(
            |(chain_a, chain_b, dv_name, const_val, domain)| OptimizationProblem {
                decision_variables: vec![DecisionVariable {
                    name: dv_name.clone(),
                    domain,
                }],
                fixed_params: HashMap::new(),
                bindings: vec![
                    VariableBinding {
                        target: chain_a.clone(),
                        expr: Expr::variable(chain_b.clone()),
                        description: String::new(),
                        source: "proptest-chain".into(),
                    },
                    VariableBinding {
                        target: chain_b.clone(),
                        expr: Expr::constant(f64::from(const_val)),
                        description: String::new(),
                        source: "proptest-chain".into(),
                    },
                ],
                constraints: vec![],
                objective: Expr::sum(vec![Expr::variable(chain_a), Expr::variable(dv_name)]),
                direction: ObjectiveDirection::Minimize,
            },
        )
}

// ---------------------------------------------------------------------------
// Property tests
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        max_shrink_iters: 1000,
        .. ProptestConfig::default()
    })]

    /// For any randomly generated problem, the `EnumerationSolver` must produce
    /// the same objective value as the naive oracle.
    ///
    /// Only objective equality is asserted (not assignment equality) because
    /// multiple optimal assignments may exist — see the module-level note above.
    ///
    /// Problems where the naive oracle itself finds no feasible solution are
    /// skipped via `prop_assume!`, to avoid asserting that two `NaN` values are
    /// equal (which would spuriously fail) and to focus the test on the cases
    /// where something meaningful can go wrong.
    #[test]
    fn solver_matches_naive_oracle(problem in arb_problem()) {
        // Skip pathologically large problems to keep the test suite fast.
        let combination_count: u64 = problem.decision_variables.iter()
            .map(|d| d.domain.len() as u64)
            .product();
        prop_assume!(combination_count > 0 && combination_count <= 256);

        let solver = EnumerationSolver;
        let actual = match solver.solve(&problem) {
            Ok(s) => s,
            Err(_) => {
                // TooManyCombinations (or another hard error) — both paths
                // reject this problem; skip it.
                return Ok(());
            }
        };

        let expected = solve_naive(&problem);

        // If the naive oracle found no feasible solution, skip: both are
        // infeasible by definition (no assignment could pass, so there is
        // nothing useful to compare).
        prop_assume!(expected.feasible);

        prop_assert!(
            actual.feasible,
            "solver reported infeasible but naive oracle found a feasible solution \
             with objective={} on problem: {:?}",
            expected.objective_value,
            problem
        );

        prop_assert!(
            (actual.objective_value - expected.objective_value).abs() < 1e-9,
            "objective mismatch: solver={}, naive={}, problem={:?}",
            actual.objective_value,
            expected.objective_value,
            problem
        );
    }

    /// 同一の問題を2回 solve した結果が一致することを確認。
    /// HashMap の非決定的な反復順序やその他の内部状態の漏れを検出する。
    #[test]
    fn solver_is_deterministic(problem in arb_problem()) {
        // 組合せ数が多すぎる場合はスキップ（速度維持）
        let combination_count: u64 = problem.decision_variables.iter()
            .map(|d| d.domain.len() as u64)
            .product();
        prop_assume!(combination_count > 0 && combination_count <= 256);

        let solver = EnumerationSolver;
        let r1 = solver.solve(&problem);
        let r2 = solver.solve(&problem);

        match (&r1, &r2) {
            (Ok(s1), Ok(s2)) => {
                prop_assert_eq!(s1.feasible, s2.feasible, "feasibility differs between runs");
                if s1.feasible {
                    prop_assert!(
                        (s1.objective_value - s2.objective_value).abs() < 1e-9,
                        "objective differs: {} vs {}", s1.objective_value, s2.objective_value
                    );
                    prop_assert_eq!(&s1.assignments, &s2.assignments,
                        "assignments differ between runs on problem: {:?}", problem);
                }
            }
            (Err(_), Err(_)) => {}
            _ => return Err(proptest::test_runner::TestCaseError::fail(
                "solver returned Ok first run and Err second run (or vice-versa)",
            )),
        }
    }

    /// `A = B`, `B = const` という2段bindingチェーンを持つ問題で
    /// solverがoracleと一致することを確認する。
    /// このクラスの問題は loop-invariant hoist バグで壊れていた。
    #[test]
    fn solver_matches_oracle_on_transitive_chain(problem in arb_transitive_chain_problem()) {
        let combination_count: u64 = problem.decision_variables.iter()
            .map(|d| d.domain.len() as u64)
            .product();
        prop_assume!(combination_count > 0 && combination_count <= 64);

        let solver = EnumerationSolver;
        let actual = match solver.solve(&problem) {
            Ok(s) => s,
            Err(_) => return Ok(()),
        };

        let expected = solve_naive(&problem);
        prop_assume!(expected.feasible);

        prop_assert!(
            actual.feasible,
            "solver reported infeasible on transitive-chain problem where oracle found feasible (obj={}): {:?}",
            expected.objective_value, problem
        );
        prop_assert!(
            (actual.objective_value - expected.objective_value).abs() < 1e-9,
            "transitive-chain objective mismatch: solver={}, naive={}, problem={:?}",
            actual.objective_value, expected.objective_value, problem
        );
    }
}
