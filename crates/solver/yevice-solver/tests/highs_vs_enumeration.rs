//! Cross-solver equivalence tests for the HiGHS MILP backend.
//!
//! These tests compile only when the `highs` Cargo feature is enabled.
//! They build a small `OptimizationProblem`, solve it with both
//! `EnumerationSolver` and `HighsSolver`, and assert that the two optima
//! agree within `max(abs_tol, rel_tol * |cost|)` per ADR-0002.
//!
//! The property test is intentionally small (domain ≤ 10, ≤ 3 decision
//! variables) so it runs fast in CI without spending the HiGHS budget on
//! pathological inputs.

#![cfg(feature = "highs")]

use std::collections::HashMap;

use proptest::prelude::*;
use yevice_core::expr::{Expr, Tier};
use yevice_core::optimize::{
    DecisionVariable, ObjectiveDirection, OptimizationConstraint, OptimizationProblem, Relation,
};
use yevice_core::types::VariableName;
use yevice_solver::milp::MilpOptions;
use yevice_solver::{EnumerationSolver, Solver, highs_backend::HighsSolver};

const ABS_TOL: f64 = 1e-4;
const REL_TOL: f64 = 1e-6;

fn assert_close(enum_cost: f64, milp_cost: f64, context: &str) {
    assert!(
        !(enum_cost.is_nan() || milp_cost.is_nan()),
        "one solver returned NaN ({context}): enum={enum_cost}, milp={milp_cost}"
    );
    let tol = ABS_TOL.max(REL_TOL * enum_cost.abs());
    let diff = (enum_cost - milp_cost).abs();
    assert!(
        diff <= tol,
        "objective mismatch ({context}): enum={enum_cost}, milp={milp_cost}, diff={diff}, tol={tol}"
    );
}

fn highs_solver() -> HighsSolver {
    HighsSolver {
        // Keep the per-test budget tight so CI doesn't drag.
        options: MilpOptions {
            time_limit_sec: Some(10.0),
            mip_gap: Some(1e-6),
            threads: Some(1),
        },
    }
}

// ---------------------------------------------------------------------------
// Hand-rolled smoke tests
// ---------------------------------------------------------------------------

#[test]
fn linear_objective_two_vars() {
    // minimize 3x + 2y, x ∈ {1,2,3}, y ∈ {0,4,8}
    let problem = OptimizationProblem {
        objective: Expr::sum(vec![
            Expr::linear(3.0, Expr::variable("x"), 0.0),
            Expr::linear(2.0, Expr::variable("y"), 0.0),
        ]),
        direction: ObjectiveDirection::Minimize,
        decision_variables: vec![
            DecisionVariable {
                name: VariableName::new("x"),
                domain: vec![1.0, 2.0, 3.0],
            },
            DecisionVariable {
                name: VariableName::new("y"),
                domain: vec![0.0, 4.0, 8.0],
            },
        ],
        constraints: vec![],
        fixed_params: HashMap::new(),
        bindings: vec![],
    };

    let e = EnumerationSolver.solve(&problem).unwrap();
    let h = highs_solver().solve(&problem).unwrap();
    assert!(e.feasible && h.feasible);
    assert_close(e.objective_value, h.objective_value, "linear two-vars");
    // Optimum is x=1, y=0 → 3.
    assert!((h.objective_value - 3.0).abs() < 1e-4);
}

#[test]
fn tiered_pricing_matches() {
    // Tiered: 100@0.10, then 0.05 unlimited.
    // var = x, x ∈ {50, 100, 150, 200}, minimize cost.
    let problem = OptimizationProblem {
        objective: Expr::tiered(
            vec![
                Tier {
                    upper_limit: Some(100.0),
                    unit_price: 0.10,
                },
                Tier {
                    upper_limit: None,
                    unit_price: 0.05,
                },
            ],
            Expr::variable("x"),
        ),
        direction: ObjectiveDirection::Minimize,
        decision_variables: vec![DecisionVariable {
            name: VariableName::new("x"),
            domain: vec![50.0, 100.0, 150.0, 200.0],
        }],
        constraints: vec![OptimizationConstraint {
            lhs: Expr::variable("x"),
            relation: Relation::Ge,
            rhs: 50.0,
            label: None,
        }],
        fixed_params: HashMap::new(),
        bindings: vec![],
    };

    let e = EnumerationSolver.solve(&problem).unwrap();
    let h = highs_solver().solve(&problem).unwrap();
    assert!(e.feasible && h.feasible);
    assert_close(e.objective_value, h.objective_value, "tiered");
}

#[test]
fn max_free_tier_pattern() {
    // Lambda-style free-tier: cost = price * max(usage - allowance, 0)
    // usage ∈ {0, 100, 200, 500}, allowance = 100, price = 0.20.
    let problem = OptimizationProblem {
        objective: Expr::product(vec![
            Expr::constant(0.20),
            Expr::Max {
                expr: Box::new(Expr::linear(1.0, Expr::variable("usage"), -100.0)),
                floor: 0.0,
            },
        ]),
        direction: ObjectiveDirection::Minimize,
        decision_variables: vec![DecisionVariable {
            name: VariableName::new("usage"),
            domain: vec![0.0, 100.0, 200.0, 500.0],
        }],
        constraints: vec![OptimizationConstraint {
            lhs: Expr::variable("usage"),
            relation: Relation::Ge,
            rhs: 100.0,
            label: None,
        }],
        fixed_params: HashMap::new(),
        bindings: vec![],
    };

    let e = EnumerationSolver.solve(&problem).unwrap();
    let h = highs_solver().solve(&problem).unwrap();
    assert!(e.feasible && h.feasible);
    assert_close(e.objective_value, h.objective_value, "max free-tier");
    // Optimum: usage=100 → max(0,0)*0.20 = 0.
    assert!(h.objective_value.abs() < 1e-4);
}

#[test]
fn ceil_minimization_le_constraint() {
    // Billable units: ceil(usage / 1024) at price = 1.
    // The Ceil is in a Le constraint LHS (positive coeff) and in the
    // objective with positive coeff under minimization → both OK.
    let problem = OptimizationProblem {
        objective: Expr::ceil(Expr::div(Expr::variable("usage"), Expr::constant(1024.0))),
        direction: ObjectiveDirection::Minimize,
        decision_variables: vec![DecisionVariable {
            name: VariableName::new("usage"),
            domain: vec![512.0, 1024.0, 2048.0, 3000.0],
        }],
        constraints: vec![],
        fixed_params: HashMap::new(),
        bindings: vec![],
    };

    let e = EnumerationSolver.solve(&problem).unwrap();
    let h = highs_solver().solve(&problem).unwrap();
    assert!(e.feasible && h.feasible);
    assert_close(e.objective_value, h.objective_value, "ceil minimize");
    // Optimum: usage=512 → ceil(0.5) = 1.
    assert!((h.objective_value - 1.0).abs() < 1e-4);
}

#[test]
fn max_with_floor_above_range() {
    // Codex P1 regression: floor sits above the inner's interval, so
    // big-M must cover (floor - lower) on the other leg.
    //   minimize max(x, 100) + x,  x ∈ {0, 5, 10}  → max=100, total=100.
    let problem = OptimizationProblem {
        objective: Expr::sum(vec![
            Expr::Max {
                expr: Box::new(Expr::variable("x")),
                floor: 100.0,
            },
            Expr::variable("x"),
        ]),
        direction: ObjectiveDirection::Minimize,
        decision_variables: vec![DecisionVariable {
            name: VariableName::new("x"),
            domain: vec![0.0, 5.0, 10.0],
        }],
        constraints: vec![],
        fixed_params: HashMap::new(),
        bindings: vec![],
    };
    let e = EnumerationSolver.solve(&problem).unwrap();
    let h = highs_solver().solve(&problem).unwrap();
    assert!(e.feasible && h.feasible);
    assert_close(e.objective_value, h.objective_value, "max floor-above");
    assert!((h.objective_value - 100.0).abs() < 1e-4);
}

#[test]
fn product_constant_times_ceil() {
    // Codex P2 regression: `2.0 * ceil(x)` should classify the ceil as
    // positive in a minimization objective (auto-tight), not Unknown.
    let problem = OptimizationProblem {
        objective: Expr::product(vec![Expr::constant(2.0), Expr::ceil(Expr::variable("x"))]),
        direction: ObjectiveDirection::Minimize,
        decision_variables: vec![DecisionVariable {
            name: VariableName::new("x"),
            domain: vec![0.5, 1.5],
        }],
        constraints: vec![],
        fixed_params: HashMap::new(),
        bindings: vec![],
    };
    let h = highs_solver().solve(&problem).unwrap();
    assert!(h.feasible, "constant * ceil(...) must be accepted");
    // x=0.5 → 2 * ceil(0.5) = 2 * 1 = 2.
    assert!((h.objective_value - 2.0).abs() < 1e-4);
}

#[test]
fn binding_target_referenced_before_definition() {
    // Codex P2 regression: a binding `b = a + 1` listed BEFORE the
    // binding `a = x` must still resolve via the encoder's two-pass
    // target-registration strategy.
    use yevice_core::cost::VariableBinding;
    let bindings = vec![
        VariableBinding {
            target: VariableName::new("b"),
            expr: Expr::linear(1.0, Expr::variable("a"), 1.0),
            description: "b = a + 1".into(),
            source: "test".into(),
        },
        VariableBinding {
            target: VariableName::new("a"),
            expr: Expr::variable("x"),
            description: "a = x".into(),
            source: "test".into(),
        },
    ];
    let problem = OptimizationProblem {
        objective: Expr::variable("b"),
        direction: ObjectiveDirection::Minimize,
        decision_variables: vec![DecisionVariable {
            name: VariableName::new("x"),
            domain: vec![1.0, 2.0],
        }],
        constraints: vec![],
        fixed_params: HashMap::new(),
        bindings,
    };
    let e = EnumerationSolver.solve(&problem).unwrap();
    let h = highs_solver().solve(&problem).unwrap();
    assert!(e.feasible && h.feasible);
    assert_close(
        e.objective_value,
        h.objective_value,
        "out-of-order bindings",
    );
}

#[test]
fn decision_var_wins_over_colliding_fixed_param_bounds() {
    // Codex round-2 P2: decision-var x ∈ {0,10} must override fixed
    // param x=100 in expr_bounds, so Max(x, 0) bounds and the optimum
    // are based on the decision domain.
    let mut fixed = HashMap::new();
    fixed.insert(VariableName::new("x"), 100.0);
    let problem = OptimizationProblem {
        objective: Expr::Max {
            expr: Box::new(Expr::variable("x")),
            floor: 0.0,
        },
        direction: ObjectiveDirection::Minimize,
        decision_variables: vec![DecisionVariable {
            name: VariableName::new("x"),
            domain: vec![0.0, 10.0],
        }],
        constraints: vec![],
        fixed_params: fixed,
        bindings: vec![],
    };
    let h = highs_solver().solve(&problem).unwrap();
    assert!(h.feasible);
    // Optimum: x=0, max(0, 0) = 0 — NOT 100.
    assert!(
        h.objective_value.abs() < 1e-4,
        "expected 0, got {}",
        h.objective_value
    );
}

#[test]
fn chained_bindings_max_objective_round_trip() {
    // Codex round-2 P2: bindings `[b = a + 1, a = x]` with Max(b, 0)
    // must encode without UnboundedExpression. The fixed-point range
    // propagation in pass B(.0) should give `a` finite bounds before
    // `b`'s range is queried.
    use yevice_core::cost::VariableBinding;
    let bindings = vec![
        VariableBinding {
            target: VariableName::new("b"),
            expr: Expr::linear(1.0, Expr::variable("a"), 1.0),
            description: "b = a + 1".into(),
            source: "test".into(),
        },
        VariableBinding {
            target: VariableName::new("a"),
            expr: Expr::variable("x"),
            description: "a = x".into(),
            source: "test".into(),
        },
    ];
    let problem = OptimizationProblem {
        objective: Expr::Max {
            expr: Box::new(Expr::variable("b")),
            floor: 0.0,
        },
        direction: ObjectiveDirection::Minimize,
        decision_variables: vec![DecisionVariable {
            name: VariableName::new("x"),
            domain: vec![0.0, 1.0, 2.0],
        }],
        constraints: vec![],
        fixed_params: HashMap::new(),
        bindings,
    };
    let e = EnumerationSolver.solve(&problem).unwrap();
    let h = highs_solver().solve(&problem).unwrap();
    assert!(e.feasible && h.feasible);
    assert_close(
        e.objective_value,
        h.objective_value,
        "chained bindings + Max",
    );
}

#[test]
fn tiered_clamps_negative_usage_to_zero() {
    // Codex round-2 P2: Tiered evaluator clamps negative usage to 0.
    // The MILP encoder must too — otherwise an inner expression whose
    // interval reaches negative values would be infeasible.
    // Setup: x ∈ {-10, 0, 50}, tiered cost of (x - 5) with a single tier
    // at 0.10/unit. The evaluator gives x=-10 → cost 0, x=0 → cost 0,
    // x=50 → 0.10*45 = 4.5. Optimum is cost 0, attained at x=-10 or 0.
    let problem = OptimizationProblem {
        objective: Expr::tiered(
            vec![Tier {
                upper_limit: None,
                unit_price: 0.10,
            }],
            Expr::linear(1.0, Expr::variable("x"), -5.0),
        ),
        direction: ObjectiveDirection::Minimize,
        decision_variables: vec![DecisionVariable {
            name: VariableName::new("x"),
            domain: vec![-10.0, 0.0, 50.0],
        }],
        constraints: vec![],
        fixed_params: HashMap::new(),
        bindings: vec![],
    };
    let e = EnumerationSolver.solve(&problem).unwrap();
    let h = highs_solver().solve(&problem).unwrap();
    assert!(e.feasible, "enumerator must accept negative usage");
    assert!(h.feasible, "MILP must accept negative usage (clamped to 0)");
    assert_close(
        e.objective_value,
        h.objective_value,
        "tiered negative usage",
    );
    assert!(h.objective_value.abs() < 1e-4);
}

#[test]
fn infeasible_problem_consistent() {
    // x ∈ {1,2,3}, constraint x >= 100 → infeasible.
    let problem = OptimizationProblem {
        objective: Expr::variable("x"),
        direction: ObjectiveDirection::Minimize,
        decision_variables: vec![DecisionVariable {
            name: VariableName::new("x"),
            domain: vec![1.0, 2.0, 3.0],
        }],
        constraints: vec![OptimizationConstraint {
            lhs: Expr::variable("x"),
            relation: Relation::Ge,
            rhs: 100.0,
            label: None,
        }],
        fixed_params: HashMap::new(),
        bindings: vec![],
    };

    let e = EnumerationSolver.solve(&problem).unwrap();
    let h = highs_solver().solve(&problem).unwrap();
    assert!(!e.feasible);
    assert!(!h.feasible);
}

// ---------------------------------------------------------------------------
// Property test (small problems)
// ---------------------------------------------------------------------------

fn arb_domain() -> impl Strategy<Value = Vec<f64>> {
    prop::collection::vec(0u32..50, 1..=8usize).prop_map(|v| {
        let mut floats: Vec<f64> = v.into_iter().map(f64::from).collect();
        floats.sort_by(|a, b| a.partial_cmp(b).unwrap());
        floats.dedup();
        floats
    })
}

fn arb_problem() -> impl Strategy<Value = OptimizationProblem> {
    // 1-3 decision variables, each with up to 8 domain values, and a linear
    // objective with random integer coefficients.
    (
        prop::collection::vec(arb_domain(), 1..=3),
        prop::collection::vec(-5i32..=5, 1..=3),
        any::<bool>(),
    )
        .prop_map(|(domains, coeffs, maximize)| {
            let mut decision_variables = Vec::new();
            let mut terms = Vec::new();
            for (i, dom) in domains.iter().enumerate() {
                let name = format!("x{i}");
                decision_variables.push(DecisionVariable {
                    name: VariableName::new(&name),
                    domain: dom.clone(),
                });
                let c = f64::from(*coeffs.get(i).unwrap_or(&1));
                terms.push(Expr::linear(c, Expr::variable(name.as_str()), 0.0));
            }
            OptimizationProblem {
                objective: Expr::sum(terms),
                direction: if maximize {
                    ObjectiveDirection::Maximize
                } else {
                    ObjectiveDirection::Minimize
                },
                decision_variables,
                constraints: vec![],
                fixed_params: HashMap::new(),
                bindings: vec![],
            }
        })
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 32,
        ..ProptestConfig::default()
    })]

    /// For every randomly generated small problem, HiGHS and Enumeration
    /// must agree on the optimal objective value to within the ADR tolerance.
    #[test]
    fn cross_solver_equivalence(problem in arb_problem()) {
        let e = EnumerationSolver.solve(&problem).expect("enum solve");
        let h = highs_solver().solve(&problem).expect("highs solve");
        prop_assert_eq!(e.feasible, h.feasible, "feasibility mismatch");
        if e.feasible {
            let tol = ABS_TOL.max(REL_TOL * e.objective_value.abs());
            let diff = (e.objective_value - h.objective_value).abs();
            prop_assert!(
                diff <= tol,
                "objective mismatch: enum={}, milp={}, diff={}, tol={}",
                e.objective_value, h.objective_value, diff, tol
            );
        }
    }
}
