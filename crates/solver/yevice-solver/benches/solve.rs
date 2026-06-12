use std::collections::HashMap;

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use yevice_core::cost::VariableBinding;
use yevice_core::expr::Expr;
use yevice_core::optimize::{
    DecisionVariable, ObjectiveDirection, OptimizationConstraint, OptimizationProblem, Relation,
};
use yevice_core::types::VariableName;
use yevice_solver::{EnumerationSolver, Solver};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn var(name: &str) -> VariableName {
    VariableName::new(name)
}

fn dv(name: &str, domain: Vec<f64>) -> DecisionVariable {
    DecisionVariable {
        name: var(name),
        domain,
    }
}

/// Build an evenly spaced domain of `n` values in [lo, hi].
fn linspace(lo: f64, hi: f64, n: usize) -> Vec<f64> {
    if n == 1 {
        return vec![lo];
    }
    (0..n)
        .map(|i| lo + (hi - lo) * (i as f64) / ((n - 1) as f64))
        .collect()
}

// ---------------------------------------------------------------------------
// Problem builders
// ---------------------------------------------------------------------------

/// Small problem: 2 decision variables × ~10 domain values each (~100 combos).
///
/// Variables: `instance_count` ∈ [1..10], `replica_count` ∈ [1..10]
/// Fixed params: `unit_cost` = 0.05, `base_fee` = 2.0, `min_replicas` = 2
/// Binding:  `total_units = instance_count * replica_count`
/// Objective: `unit_cost * total_units + base_fee`   (minimize)
/// Constraint: `replica_count >= min_replicas`
///
/// Feasible: yes (replica_count=2, instance_count=1 is optimal).
fn build_small() -> OptimizationProblem {
    let binding = VariableBinding {
        target: var("total_units"),
        expr: Expr::product(vec![
            Expr::variable("instance_count"),
            Expr::variable("replica_count"),
        ]),
        description: "total_units = instance_count * replica_count".into(),
        source: "bench".into(),
    };

    let objective = Expr::sum(vec![
        Expr::product(vec![
            Expr::variable("unit_cost"),
            Expr::variable("total_units"),
        ]),
        Expr::variable("base_fee"),
    ]);

    let constraint = OptimizationConstraint {
        lhs: Expr::variable("replica_count"),
        relation: Relation::Ge,
        rhs: 2.0,
        label: Some("min_replicas".into()),
    };

    let mut fixed = HashMap::new();
    fixed.insert(var("unit_cost"), 0.05);
    fixed.insert(var("base_fee"), 2.0);
    fixed.insert(var("min_replicas"), 2.0);

    OptimizationProblem {
        objective,
        direction: ObjectiveDirection::Minimize,
        decision_variables: vec![
            dv("instance_count", (1..=10).map(f64::from).collect()),
            dv("replica_count", (1..=10).map(f64::from).collect()),
        ],
        constraints: vec![constraint],
        fixed_params: fixed,
        bindings: vec![binding],
    }
}

/// Medium problem: 4 decision variables × domains totalling ~10 000 combos.
///
/// Variables:
///   `vcpu`        ∈ {1,2,4,8,16}              (5 values)
///   `memory_gb`   ∈ {1,2,4,8,16,32}           (6 values)
///   `storage_gb`  ∈ linspace(10, 500, 10)      (10 values)
///   `replicas`    ∈ {1,2,3,4,5,6}             (6 values)
///
/// Total: 5 × 6 × 10 × 6 = 1 800 combos  (fast but representative).
///
/// Fixed params: many cost-rate params (simulates a realistic FinOps model).
/// Bindings:
///   fixed-only:  `storage_rate = base_storage_rate * storage_discount`
///   decision-dep: `compute_cost  = vcpu * memory_gb * compute_rate * replicas`
///                 `storage_cost  = storage_gb * storage_rate`
///                 `total_cost    = compute_cost + storage_cost + fixed_overhead`
/// Objective: `total_cost`  (minimize)
/// Constraints:
///   `vcpu >= min_vcpu`, `memory_gb >= min_memory`, `replicas >= min_replicas`
fn build_medium() -> OptimizationProblem {
    // Fixed-only binding: storage_rate = base_storage_rate * storage_discount
    let b_storage_rate = VariableBinding {
        target: var("storage_rate"),
        expr: Expr::product(vec![
            Expr::variable("base_storage_rate"),
            Expr::variable("storage_discount"),
        ]),
        description: "storage_rate = base_storage_rate * storage_discount".into(),
        source: "bench".into(),
    };

    // Decision-dependent bindings
    let b_compute_cost = VariableBinding {
        target: var("compute_cost"),
        expr: Expr::product(vec![
            Expr::variable("vcpu"),
            Expr::variable("memory_gb"),
            Expr::variable("compute_rate"),
            Expr::variable("replicas"),
        ]),
        description: "compute_cost = vcpu * memory_gb * compute_rate * replicas".into(),
        source: "bench".into(),
    };
    let b_storage_cost = VariableBinding {
        target: var("storage_cost"),
        expr: Expr::product(vec![
            Expr::variable("storage_gb"),
            Expr::variable("storage_rate"),
        ]),
        description: "storage_cost = storage_gb * storage_rate".into(),
        source: "bench".into(),
    };
    let b_total_cost = VariableBinding {
        target: var("total_cost"),
        expr: Expr::sum(vec![
            Expr::variable("compute_cost"),
            Expr::variable("storage_cost"),
            Expr::variable("fixed_overhead"),
        ]),
        description: "total_cost = compute_cost + storage_cost + fixed_overhead".into(),
        source: "bench".into(),
    };

    let objective = Expr::variable("total_cost");

    let constraints = vec![
        OptimizationConstraint {
            lhs: Expr::variable("vcpu"),
            relation: Relation::Ge,
            rhs: 2.0,
            label: Some("min_vcpu".into()),
        },
        OptimizationConstraint {
            lhs: Expr::variable("memory_gb"),
            relation: Relation::Ge,
            rhs: 4.0,
            label: Some("min_memory".into()),
        },
        OptimizationConstraint {
            lhs: Expr::variable("replicas"),
            relation: Relation::Ge,
            rhs: 2.0,
            label: Some("min_replicas".into()),
        },
    ];

    let mut fixed = HashMap::new();
    fixed.insert(var("compute_rate"), 0.048);
    fixed.insert(var("base_storage_rate"), 0.10);
    fixed.insert(var("storage_discount"), 0.85);
    fixed.insert(var("fixed_overhead"), 50.0);
    // additional fixed params simulating usage/quota context
    fixed.insert(var("region_multiplier"), 1.0);
    fixed.insert(var("support_fee"), 10.0);
    fixed.insert(var("network_egress_gb"), 100.0);
    fixed.insert(var("network_rate"), 0.09);
    fixed.insert(var("data_transfer_cost"), 9.0);
    fixed.insert(var("license_fee"), 20.0);
    fixed.insert(var("monitoring_fee"), 5.0);
    fixed.insert(var("backup_fee"), 3.0);

    OptimizationProblem {
        objective,
        direction: ObjectiveDirection::Minimize,
        decision_variables: vec![
            dv("vcpu", vec![1.0, 2.0, 4.0, 8.0, 16.0]),
            dv("memory_gb", vec![1.0, 2.0, 4.0, 8.0, 16.0, 32.0]),
            dv("storage_gb", linspace(10.0, 500.0, 10)),
            dv("replicas", vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]),
        ],
        constraints,
        fixed_params: fixed,
        // fixed-only binding listed first (adversarial for partition correctness)
        bindings: vec![b_storage_rate, b_compute_cost, b_storage_cost, b_total_cost],
    }
}

/// Large problem: 5 decision variables totalling ~100 000 combos.
///
/// Variables:
///   `node_count`       ∈ {1..20}              (20 values)
///   `vcpu_per_node`    ∈ {2,4,8,16,32}        (5 values)
///   `memory_per_node`  ∈ {4,8,16,32,64}       (5 values)
///   `storage_per_node` ∈ linspace(50, 2000, 10) (10 values)
///   `replicas`         ∈ {1,2,3}              (3 values)
///
/// Total: 20 × 5 × 5 × 10 × 3 = 15 000 combos.
///
/// Fixed params: a fuller set (~20) simulating a realistic large-scale problem.
/// Bindings: fixed-only + decision-dependent chain.
/// Objective: total fleet cost (minimize).
/// Constraints: capacity and redundancy requirements.
fn build_large() -> OptimizationProblem {
    // Fixed-only: discount_factor = base_discount * loyalty_bonus
    let b_discount = VariableBinding {
        target: var("discount_factor"),
        expr: Expr::product(vec![
            Expr::variable("base_discount"),
            Expr::variable("loyalty_bonus"),
        ]),
        description: "discount_factor = base_discount * loyalty_bonus".into(),
        source: "bench".into(),
    };
    // Fixed-only: effective_compute_rate = compute_rate * discount_factor
    let b_eff_compute = VariableBinding {
        target: var("effective_compute_rate"),
        expr: Expr::product(vec![
            Expr::variable("compute_rate"),
            Expr::variable("discount_factor"),
        ]),
        description: "effective_compute_rate = compute_rate * discount_factor".into(),
        source: "bench".into(),
    };

    // Decision-dependent: node_compute_cost = node_count * vcpu_per_node * memory_per_node * effective_compute_rate
    let b_node_compute = VariableBinding {
        target: var("node_compute_cost"),
        expr: Expr::product(vec![
            Expr::variable("node_count"),
            Expr::variable("vcpu_per_node"),
            Expr::variable("memory_per_node"),
            Expr::variable("effective_compute_rate"),
        ]),
        description: "node_compute_cost = node_count * vcpu_per_node * memory_per_node * effective_compute_rate".into(),
        source: "bench".into(),
    };
    // Decision-dependent: node_storage_cost = node_count * storage_per_node * storage_rate
    let b_node_storage = VariableBinding {
        target: var("node_storage_cost"),
        expr: Expr::product(vec![
            Expr::variable("node_count"),
            Expr::variable("storage_per_node"),
            Expr::variable("storage_rate"),
        ]),
        description: "node_storage_cost = node_count * storage_per_node * storage_rate".into(),
        source: "bench".into(),
    };
    // Decision-dependent: replication_overhead = replicas * replication_rate * node_count
    let b_replication = VariableBinding {
        target: var("replication_overhead"),
        expr: Expr::product(vec![
            Expr::variable("replicas"),
            Expr::variable("replication_rate"),
            Expr::variable("node_count"),
        ]),
        description: "replication_overhead = replicas * replication_rate * node_count".into(),
        source: "bench".into(),
    };
    // Decision-dependent: fleet_cost = node_compute_cost + node_storage_cost + replication_overhead + infra_overhead
    let b_fleet = VariableBinding {
        target: var("fleet_cost"),
        expr: Expr::sum(vec![
            Expr::variable("node_compute_cost"),
            Expr::variable("node_storage_cost"),
            Expr::variable("replication_overhead"),
            Expr::variable("infra_overhead"),
        ]),
        description: "fleet_cost = node_compute_cost + node_storage_cost + replication_overhead + infra_overhead".into(),
        source: "bench".into(),
    };

    let objective = Expr::variable("fleet_cost");

    let constraints = vec![
        // Minimum fleet capacity
        OptimizationConstraint {
            lhs: Expr::variable("node_count"),
            relation: Relation::Ge,
            rhs: 3.0,
            label: Some("min_nodes".into()),
        },
        // Minimum per-node vcpu
        OptimizationConstraint {
            lhs: Expr::variable("vcpu_per_node"),
            relation: Relation::Ge,
            rhs: 4.0,
            label: Some("min_vcpu".into()),
        },
        // Minimum redundancy
        OptimizationConstraint {
            lhs: Expr::variable("replicas"),
            relation: Relation::Ge,
            rhs: 2.0,
            label: Some("min_replicas".into()),
        },
        // Fleet cost budget ceiling
        OptimizationConstraint {
            lhs: Expr::variable("fleet_cost"),
            relation: Relation::Le,
            rhs: 1_000_000.0,
            label: Some("budget_ceiling".into()),
        },
    ];

    let mut fixed = HashMap::new();
    fixed.insert(var("compute_rate"), 0.048);
    fixed.insert(var("storage_rate"), 0.10);
    fixed.insert(var("base_discount"), 0.95);
    fixed.insert(var("loyalty_bonus"), 0.98);
    fixed.insert(var("replication_rate"), 5.0);
    fixed.insert(var("infra_overhead"), 200.0);
    fixed.insert(var("support_tier_cost"), 50.0);
    fixed.insert(var("monitoring_cost"), 20.0);
    fixed.insert(var("backup_cost"), 15.0);
    fixed.insert(var("network_cost"), 30.0);
    fixed.insert(var("license_cost"), 100.0);
    fixed.insert(var("data_transfer_gb"), 500.0);
    fixed.insert(var("egress_rate"), 0.09);
    fixed.insert(var("ingress_rate"), 0.01);
    fixed.insert(var("snapshot_cost"), 10.0);
    fixed.insert(var("audit_log_cost"), 5.0);
    fixed.insert(var("dns_cost"), 2.0);
    fixed.insert(var("load_balancer_cost"), 18.0);
    fixed.insert(var("waf_cost"), 8.0);
    fixed.insert(var("cdn_cost"), 12.0);

    OptimizationProblem {
        objective,
        direction: ObjectiveDirection::Minimize,
        decision_variables: vec![
            dv("node_count", (1..=20).map(f64::from).collect()),
            dv("vcpu_per_node", vec![2.0, 4.0, 8.0, 16.0, 32.0]),
            dv("memory_per_node", vec![4.0, 8.0, 16.0, 32.0, 64.0]),
            dv("storage_per_node", linspace(50.0, 2000.0, 10)),
            dv("replicas", vec![1.0, 2.0, 3.0]),
        ],
        constraints,
        fixed_params: fixed,
        bindings: vec![
            b_discount,
            b_eff_compute,
            b_node_compute,
            b_node_storage,
            b_replication,
            b_fleet,
        ],
    }
}

// ---------------------------------------------------------------------------
// Benchmark functions
// ---------------------------------------------------------------------------

fn bench_small(c: &mut Criterion) {
    let problem = build_small();
    // Sanity-check: must be feasible before we measure.
    let sol = EnumerationSolver.solve(&problem).unwrap();
    assert!(sol.feasible, "bench_small problem must be feasible");

    c.bench_function("solve_small (~100 combos)", |b| {
        b.iter(|| EnumerationSolver.solve(black_box(&problem)).unwrap());
    });
}

fn bench_medium(c: &mut Criterion) {
    let problem = build_medium();
    let sol = EnumerationSolver.solve(&problem).unwrap();
    assert!(sol.feasible, "bench_medium problem must be feasible");

    c.bench_function("solve_medium (~1 800 combos)", |b| {
        b.iter(|| EnumerationSolver.solve(black_box(&problem)).unwrap());
    });
}

fn bench_large(c: &mut Criterion) {
    let problem = build_large();
    let sol = EnumerationSolver.solve(&problem).unwrap();
    assert!(sol.feasible, "bench_large problem must be feasible");

    c.bench_function("solve_large (~15 000 combos)", |b| {
        b.iter(|| EnumerationSolver.solve(black_box(&problem)).unwrap());
    });
}

// ---------------------------------------------------------------------------
// Criterion wiring
// ---------------------------------------------------------------------------

criterion_group!(benches, bench_small, bench_medium, bench_large);
criterion_main!(benches);
