//! Branch/boundary behaviour tests for core cost & capacity logic.
//!
//! These pin behaviour that the existing tests left unguarded, so mutations to
//! comparison operators (`>` -> `>=`), boolean predicates (`==` -> `!=`),
//! tier-width arithmetic (`-` -> `+`), and the component-vs-expr branch in
//! `evaluate_architecture` are caught.

use yevice_core::capacity::{
    CapacityModel, Constraint, QuotaType, Severity, ValidationResult, Violation, validate_capacity,
};
use yevice_core::cost::{
    ArchitectureCost, CostComponent, Expr, ResourceCost, Tier, VariableBinding, VariableInfo,
};
use yevice_core::evaluate::{Params, evaluate, evaluate_architecture};
use yevice_core::types::{ArchitectureName, LogicalId, Region, ResourceType, VariableName};

#[track_caller]
fn approx(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-9,
        "expected {expected}, got {actual}"
    );
}

fn params(pairs: &[(&str, f64)]) -> Params {
    pairs
        .iter()
        .map(|(k, v)| (VariableName::new(*k), *v))
        .collect()
}

// ---------------------------------------------------------------------------
// capacity.rs
// ---------------------------------------------------------------------------

fn violation(severity: Severity) -> Violation {
    Violation {
        severity,
        resource: LogicalId::new("r"),
        dimension: "d".into(),
        required: 1.0,
        limit: 0.0,
        quota_type: QuotaType::Soft,
        message: String::new(),
    }
}

#[test]
fn has_errors_is_true_only_when_an_error_violation_is_present() {
    let with_error = ValidationResult {
        violations: vec![violation(Severity::Warning), violation(Severity::Error)],
        skipped: Vec::new(),
    };
    assert!(with_error.has_errors());

    let without_error = ValidationResult {
        violations: vec![violation(Severity::Warning), violation(Severity::Info)],
        skipped: Vec::new(),
    };
    assert!(!without_error.has_errors());
}

#[test]
fn has_warnings_is_true_only_when_a_warning_violation_is_present() {
    let with_warning = ValidationResult {
        violations: vec![violation(Severity::Info), violation(Severity::Warning)],
        skipped: Vec::new(),
    };
    assert!(with_warning.has_warnings());

    let without_warning = ValidationResult {
        violations: vec![violation(Severity::Error), violation(Severity::Info)],
        skipped: Vec::new(),
    };
    assert!(!without_warning.has_warnings());
}

fn model(limit: f64, severity: Severity) -> CapacityModel {
    CapacityModel {
        logical_id: LogicalId::new("svc"),
        label: "svc".into(),
        constraints: vec![Constraint {
            dimension: "dim".into(),
            required: Expr::variable("x"),
            limit,
            quota_type: QuotaType::Hard,
            severity,
            message_template: "need {required} over {limit}".into(),
        }],
    }
}

#[test]
fn validate_capacity_flags_a_violation_only_when_required_strictly_exceeds_limit() {
    let models = vec![model(10.0, Severity::Error)];

    // required == limit -> NOT a violation (kills `>` -> `>=`).
    assert!(
        validate_capacity(&models, &params(&[("x", 10.0)]))
            .violations
            .is_empty()
    );
    // required < limit -> no violation.
    assert!(
        validate_capacity(&models, &params(&[("x", 9.0)]))
            .violations
            .is_empty()
    );
    // required > limit -> one violation.
    let result = validate_capacity(&models, &params(&[("x", 11.0)]));
    assert_eq!(result.violations.len(), 1);
    approx(result.violations[0].required, 11.0);
}

#[test]
fn validate_capacity_skips_constraints_whose_variable_is_unprovided() {
    let models = vec![model(10.0, Severity::Error)];
    // "x" is not provided -> evaluation fails -> constraint skipped, no panic.
    assert!(
        validate_capacity(&models, &Params::default())
            .violations
            .is_empty()
    );
}

#[test]
fn validate_capacity_records_skipped_when_variable_is_missing() {
    let models = vec![model(10.0, Severity::Error)];
    // "x" is not provided -> constraint is skipped and recorded.
    let result = validate_capacity(&models, &Params::default());
    assert!(result.violations.is_empty());
    assert_eq!(result.skipped.len(), 1);
    assert_eq!(result.skipped[0].resource, LogicalId::new("svc"));
    assert_eq!(result.skipped[0].dimension, "dim");
}

#[test]
fn validate_capacity_orders_violations_error_then_warning_then_info() {
    let models = vec![
        model(0.0, Severity::Info),
        model(0.0, Severity::Warning),
        model(0.0, Severity::Error),
    ];
    let result = validate_capacity(&models, &params(&[("x", 5.0)]));
    let severities: Vec<Severity> = result.violations.iter().map(|v| v.severity).collect();
    assert_eq!(
        severities,
        vec![Severity::Error, Severity::Warning, Severity::Info]
    );
}

// ---------------------------------------------------------------------------
// evaluate.rs
// ---------------------------------------------------------------------------

#[test]
fn tiered_eval_consumes_middle_tier_at_its_true_width() {
    // Free up to 100, then 100..300 @ 2.0, then >300 @ 5.0.
    let expr = Expr::tiered(
        vec![
            Tier {
                upper_limit: Some(100.0),
                unit_price: 0.0,
            },
            Tier {
                upper_limit: Some(300.0),
                unit_price: 2.0,
            },
            Tier {
                upper_limit: None,
                unit_price: 5.0,
            },
        ],
        Expr::variable("u"),
    );

    // usage 400: 100@0 + 200@2 (=400) + 100@5 (=500) = 900.
    // With tier_width `limit + prev_limit` the middle tier would over-consume
    // and the third tier would never be reached -> different total.
    approx(evaluate(&expr, &params(&[("u", 400.0)])).unwrap(), 900.0);
}

fn resource(label: &str, expr: Expr, components: Vec<CostComponent>) -> ResourceCost {
    ResourceCost {
        logical_id: LogicalId::new(label),
        resource_type: ResourceType::new("Test::Resource"),
        label: label.into(),
        expr,
        components,
        required_variables: vec![],
    }
}

fn architecture(resources: Vec<ResourceCost>, bindings: Vec<VariableBinding>) -> ArchitectureCost {
    ArchitectureCost {
        name: ArchitectureName::new("arch"),
        resources,
        bindings,
        region: Region::new("test"),
        topology: yevice_core::Topology::default(),
        diagnostics: Vec::new(),
    }
}

#[test]
fn evaluate_architecture_sums_components_when_all_components_evaluate() {
    // expr deliberately disagrees with the component sum so the chosen branch
    // is observable: the component-sum path (== branch) must win here.
    let rc = resource(
        "r",
        Expr::constant(999.0),
        vec![
            CostComponent {
                name: "a".into(),
                expr: Expr::constant(10.0),
            },
            CostComponent {
                name: "b".into(),
                expr: Expr::constant(20.0),
            },
        ],
    );
    let result =
        evaluate_architecture(&architecture(vec![rc], vec![]), &Params::default()).unwrap();
    approx(result.total_monthly_cost, 30.0);
    approx(result.resources[0].monthly_cost, 30.0);
}

#[test]
fn evaluate_architecture_falls_back_to_expr_when_a_component_cannot_evaluate() {
    // One component references an unprovided variable, so the component-sum
    // path is unavailable and the top-level `expr` is used instead.
    let rc = resource(
        "r",
        Expr::constant(42.0),
        vec![CostComponent {
            name: "c".into(),
            expr: Expr::variable("missing"),
        }],
    );
    let result =
        evaluate_architecture(&architecture(vec![rc], vec![]), &Params::default()).unwrap();
    approx(result.total_monthly_cost, 42.0);
}

// ---------------------------------------------------------------------------
// cost.rs aggregation helpers
// ---------------------------------------------------------------------------

#[test]
fn all_variables_excludes_bound_variables_and_keeps_unbound_ones() {
    let id = LogicalId::new("r");
    let rc = ResourceCost {
        logical_id: id.clone(),
        resource_type: ResourceType::new("Test::Resource"),
        label: "r".into(),
        expr: Expr::constant(0.0),
        components: vec![],
        required_variables: vec![
            VariableInfo::new(&id, "bound", "bound var", "u"),
            VariableInfo::new(&id, "free", "free var", "u"),
        ],
    };
    let binding = VariableBinding {
        target: id.var("bound"),
        expr: Expr::constant(1.0),
        description: String::new(),
        source: String::new(),
    };
    let arch = architecture(vec![rc], vec![binding]);

    let vars = arch.all_variables();
    assert_eq!(vars.len(), 1);
    assert_eq!(vars[0].name, id.var("free"));

    assert_eq!(arch.all_bindings().len(), 1);
    assert_eq!(arch.all_bindings()[0].target, id.var("bound"));
}
