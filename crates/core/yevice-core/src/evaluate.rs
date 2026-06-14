use std::collections::BTreeMap;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::cost::{ArchitectureCost, CostBuildError, Expr};
use crate::currency::Money;
use crate::error::CoreError;
use crate::types::{ArchitectureName, LogicalId, ResourceType, VariableName};

impl From<CostBuildError> for CoreError {
    fn from(e: CostBuildError) -> Self {
        match e {
            CostBuildError::ComponentCurrencyMismatch {
                resource_id,
                currencies,
            } => CoreError::ComponentCurrencyMismatch {
                resource_id,
                currencies,
            },
        }
    }
}

/// Default fallback currency applied when a `ResourceCost` does not declare
/// `currency` and components do not override it. See ADR-0001
/// "後方互換 (migration period)".
pub const FALLBACK_CURRENCY: &str = "USD";

/// Parameters for cost evaluation: variable name -> value.
pub type Params = FxHashMap<VariableName, f64>;

/// Evaluate a cost expression with the given parameters.
pub fn evaluate(expr: &Expr, params: &Params) -> Result<f64, CoreError> {
    match expr {
        Expr::Constant { value } => Ok(*value),

        Expr::Variable { name } => params
            .get(name)
            .copied()
            .ok_or_else(|| CoreError::UndefinedVariable(name.to_string())),

        Expr::Linear { coeff, var, offset } => {
            let v = evaluate(var, params)?;
            Ok(coeff * v + offset)
        }

        Expr::Tiered { tiers, var } => {
            let usage = evaluate(var, params)?;
            let mut total_cost = 0.0;
            let mut remaining = usage;
            let mut prev_limit = 0.0;

            for tier in tiers {
                if remaining <= 0.0 {
                    break;
                }
                let tier_width = match tier.upper_limit {
                    Some(limit) => limit - prev_limit,
                    None => remaining,
                };
                let consumed = remaining.min(tier_width);
                total_cost += consumed * tier.unit_price;
                remaining -= consumed;
                if let Some(limit) = tier.upper_limit {
                    prev_limit = limit;
                }
            }

            Ok(total_cost)
        }

        Expr::Sum { exprs } => {
            let mut total = 0.0;
            for e in exprs {
                total += evaluate(e, params)?;
            }
            Ok(total)
        }

        Expr::Product { exprs } => {
            let mut product = 1.0;
            for e in exprs {
                product *= evaluate(e, params)?;
            }
            Ok(product)
        }

        Expr::Max { expr, floor } => {
            let v = evaluate(expr, params)?;
            Ok(v.max(*floor))
        }

        Expr::Min { expr, ceiling } => {
            let v = evaluate(expr, params)?;
            Ok(v.min(*ceiling))
        }

        Expr::Ceil { expr } => {
            let v = evaluate(expr, params)?;
            Ok(v.ceil())
        }

        Expr::Div {
            numerator,
            denominator,
        } => {
            let n = evaluate(numerator, params)?;
            let d = evaluate(denominator, params)?;
            if d == 0.0 {
                return Err(CoreError::DivisionByZero);
            }
            Ok(n / d)
        }
    }
}

/// Result of evaluating a single resource's cost.
#[derive(Debug, Clone)]
pub struct ResourceResult {
    pub logical_id: LogicalId,
    pub resource_type: ResourceType,
    pub label: String,
    /// Monthly cost as a runtime-tagged [`Money`] (currency derived from
    /// `ResourceCost.currency` with USD fallback per ADR-0001).
    pub monthly_cost: Money,
    /// Named cost component breakdown (name, money).
    pub component_costs: Vec<(String, Money)>,
}

/// Result of evaluating an entire architecture's cost.
///
/// `totals_by_currency` holds the per-currency aggregate. `display_total` is
/// an FX-converted single-currency summary (populated by the CLI when
/// `--display-currency` is supplied); the evaluator itself leaves it `None`.
#[derive(Debug, Clone)]
pub struct ArchitectureResult {
    pub name: ArchitectureName,
    pub resources: Vec<ResourceResult>,
    /// Per-currency monthly totals. Keys are currency codes (e.g. `"USD"`,
    /// `"JPY"`) and values are the summed monthly amount in that currency.
    pub totals_by_currency: BTreeMap<String, f64>,
    /// Optional FX-converted single-currency display total, populated by the
    /// CLI layer when `--display-currency` is supplied and rates are
    /// available.
    pub display_total: Option<Money>,
}

impl ArchitectureResult {
    /// `true` when every resource shares the same currency.
    pub fn is_single_currency(&self) -> bool {
        self.totals_by_currency.len() <= 1
    }

    /// Sum of all per-currency totals as a raw `f64`. **Only meaningful in
    /// single-currency mode** — for multi-currency results, callers must
    /// either pick `display_total` (after FX conversion) or iterate
    /// `totals_by_currency`.
    pub fn naive_total(&self) -> f64 {
        self.totals_by_currency.values().sum()
    }
}

/// Resolve the effective currency for a `ResourceCost.expr` evaluation.
///
/// Priority: explicit resource currency → first non-None component currency →
/// fallback (`FALLBACK_CURRENCY`). When the resource carries no currency at
/// all, a `tracing::warn!` is emitted once per evaluation; the value is then
/// treated as USD so legacy `cost_model.json` files still load.
fn resource_currency(rc: &crate::cost::ResourceCost) -> String {
    if let Some(c) = &rc.currency {
        return c.clone();
    }
    if let Some(first) = rc.components.iter().find_map(|c| c.currency.clone()) {
        return first;
    }
    tracing::warn!(
        resource = %rc.logical_id,
        "ResourceCost.currency is None and no component overrides; falling back to {FALLBACK_CURRENCY}"
    );
    FALLBACK_CURRENCY.to_string()
}

fn component_currency(c: &crate::cost::CostComponent, parent: &str) -> String {
    c.currency.clone().unwrap_or_else(|| parent.to_string())
}

/// Evaluate all resource costs in an architecture.
///
/// Bindings are resolved first: each binding's expression is evaluated
/// using current params, and the result is inserted as the target variable.
/// User-provided params take precedence over bindings (explicit override).
pub fn evaluate_architecture(
    arch: &ArchitectureCost,
    params: &Params,
) -> Result<ArchitectureResult, CoreError> {
    // Guard: validate currency consistency for all resources before evaluation.
    // This catches ResourceCost values constructed via struct literals or
    // deserialized from hand-edited JSON that bypass ResourceCost::new().
    arch.validate().map_err(CoreError::from)?;

    let mut effective_params = resolve_bindings(&arch.bindings, params)?;

    // User-provided params override bindings
    for (k, v) in params {
        effective_params.insert(k.clone(), *v);
    }

    let mut resources = Vec::new();
    let mut totals_by_currency: BTreeMap<String, f64> = BTreeMap::new();

    for rc in &arch.resources {
        let parent_currency = resource_currency(rc);
        let component_costs: Vec<(String, Money)> = rc
            .components
            .iter()
            .filter_map(|c| match evaluate(&c.expr, &effective_params) {
                Ok(v) => Some((
                    c.name.clone(),
                    Money::monthly(v, component_currency(c, &parent_currency)),
                )),
                Err(e) => {
                    tracing::warn!(
                        component = %c.name,
                        error = %e,
                        "component cost could not be evaluated; omitted from breakdown (total derived from top-level expression)"
                    );
                    None
                }
            })
            .collect();

        // Derive total from components when all evaluated successfully; otherwise re-evaluate expr.
        let cost = if component_costs.len() == rc.components.len() && !rc.components.is_empty() {
            component_costs.iter().map(|(_, m)| m.value).sum()
        } else {
            evaluate(&rc.expr, &effective_params)?
        };

        *totals_by_currency
            .entry(parent_currency.clone())
            .or_insert(0.0) += cost;

        resources.push(ResourceResult {
            logical_id: rc.logical_id.clone(),
            resource_type: rc.resource_type.clone(),
            label: rc.label.clone(),
            monthly_cost: Money::monthly(cost, parent_currency),
            component_costs,
        });
    }

    Ok(ArchitectureResult {
        name: arch.name.clone(),
        resources,
        totals_by_currency,
        display_total: None,
    })
}

/// Resolve variable bindings by evaluating each binding's expression.
///
/// Iterates to a fixed point so chained bindings (`A → B → C`) resolve even
/// when the input slice lists them in any order. Each pass evaluates every
/// still-unresolved binding; if no new variable is produced in a pass,
/// resolution stops. Bounded by `bindings.len() + 1` passes to guarantee
/// termination if some bindings are unresolvable (missing user params or
/// a dependency cycle).
pub fn resolve_bindings(
    bindings: &[crate::cost::VariableBinding],
    base_params: &Params,
) -> Result<Params, CoreError> {
    let mut params = base_params.clone();
    resolve_bindings_into(&mut params, bindings);
    Ok(params)
}

/// In-place variant of [`resolve_bindings`]: resolves bindings directly into
/// the supplied `params` map without cloning it first.
///
/// The fixed-point semantics are identical to [`resolve_bindings`].  Variables
/// already present in `params` are never overwritten (decision-variable values
/// and pre-resolved fixed bindings take precedence).
pub fn resolve_bindings_into(params: &mut Params, bindings: &[crate::cost::VariableBinding]) {
    // Targets whose expression can never evaluate (e.g. division by zero), as
    // distinct from those merely waiting on a not-yet-resolved variable.
    let mut unresolvable: FxHashSet<VariableName> = FxHashSet::default();

    let max_passes = bindings.len() + 1;
    for _ in 0..max_passes {
        let mut progressed = false;
        for binding in bindings {
            if params.contains_key(&binding.target) || unresolvable.contains(&binding.target) {
                continue;
            }
            match evaluate(&binding.expr, params) {
                Ok(value) => {
                    params.insert(binding.target.clone(), value);
                    progressed = true;
                }
                // A missing variable may be produced by a later binding; retry next pass.
                Err(CoreError::UndefinedVariable(_)) => {}
                // A structural error (e.g. division by zero) will never resolve; warn once.
                Err(e) => {
                    tracing::warn!(
                        target = %binding.target,
                        error = %e,
                        "binding expression cannot be evaluated; skipping"
                    );
                    unresolvable.insert(binding.target.clone());
                }
            }
        }
        if !progressed {
            break;
        }
    }

    // Warn about bindings still unresolved because a required variable was never provided.
    for binding in bindings {
        if !params.contains_key(&binding.target) && !unresolvable.contains(&binding.target) {
            tracing::warn!(
                target = %binding.target,
                "binding could not be resolved with current params (missing variable)"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cost::Tier;

    fn var(name: &str) -> VariableName {
        VariableName::new(name)
    }

    fn params_from(pairs: &[(&str, f64)]) -> Params {
        pairs.iter().map(|(k, v)| (var(k), *v)).collect()
    }

    #[test]
    fn test_div_by_zero_returns_err() {
        let expr = Expr::div(Expr::constant(10.0), Expr::constant(0.0));
        let result = evaluate(&expr, &Params::default());
        assert!(
            matches!(result, Err(CoreError::DivisionByZero)),
            "expected DivisionByZero error, got {result:?}"
        );
    }

    #[test]
    fn test_div_nonzero() {
        let expr = Expr::div(Expr::constant(10.0), Expr::constant(2.0));
        assert_eq!(evaluate(&expr, &Params::default()).unwrap(), 5.0);
    }

    #[test]
    fn test_constant() {
        let expr = Expr::constant(42.0);
        assert_eq!(evaluate(&expr, &Params::default()).unwrap(), 42.0);
    }

    #[test]
    fn test_variable() {
        let expr = Expr::variable("x");
        let params = params_from(&[("x", 10.0)]);
        assert_eq!(evaluate(&expr, &params).unwrap(), 10.0);
    }

    #[test]
    fn test_undefined_variable() {
        let expr = Expr::variable("x");
        assert!(evaluate(&expr, &Params::default()).is_err());
    }

    #[test]
    fn test_linear() {
        let expr = Expr::linear(0.5, Expr::variable("hours"), 10.0);
        let params = params_from(&[("hours", 100.0)]);
        assert_eq!(evaluate(&expr, &params).unwrap(), 60.0);
    }

    #[test]
    fn test_tiered_with_free_tier() {
        let expr = Expr::tiered(
            vec![
                Tier {
                    upper_limit: Some(1_000_000.0),
                    unit_price: 0.0,
                },
                Tier {
                    upper_limit: None,
                    unit_price: 0.0000002,
                },
            ],
            Expr::variable("requests"),
        );

        // Within free tier
        let params = params_from(&[("requests", 500_000.0)]);
        assert_eq!(evaluate(&expr, &params).unwrap(), 0.0);

        // Beyond free tier
        let params = params_from(&[("requests", 2_000_000.0)]);
        let result = evaluate(&expr, &params).unwrap();
        assert!((result - 0.20).abs() < 1e-10);
    }

    #[test]
    fn test_s3_tiered_storage() {
        let expr = Expr::tiered(
            vec![
                Tier {
                    upper_limit: Some(50_000.0),
                    unit_price: 0.025,
                },
                Tier {
                    upper_limit: Some(500_000.0),
                    unit_price: 0.024,
                },
                Tier {
                    upper_limit: None,
                    unit_price: 0.023,
                },
            ],
            Expr::variable("storage_gb"),
        );

        let params = params_from(&[("storage_gb", 100.0)]);
        let result = evaluate(&expr, &params).unwrap();
        assert!((result - 2.5).abs() < 1e-10);

        let params = params_from(&[("storage_gb", 60_000.0)]);
        let result = evaluate(&expr, &params).unwrap();
        let expected = 50_000.0 * 0.025 + 10_000.0 * 0.024;
        assert!((result - expected).abs() < 1e-10);
    }

    #[test]
    fn test_sum_and_product() {
        let expr = Expr::sum(vec![
            Expr::constant(10.0),
            Expr::product(vec![Expr::constant(2.0), Expr::variable("x")]),
        ]);
        let params = params_from(&[("x", 5.0)]);
        assert_eq!(evaluate(&expr, &params).unwrap(), 20.0);
    }

    #[test]
    fn test_max_min() {
        let expr = Expr::Max {
            expr: Box::new(Expr::variable("x")),
            floor: 10.0,
        };

        let params = params_from(&[("x", 5.0)]);
        assert_eq!(evaluate(&expr, &params).unwrap(), 10.0);

        let params = params_from(&[("x", 15.0)]);
        assert_eq!(evaluate(&expr, &params).unwrap(), 15.0);
    }

    /// Regression: chained bindings (A → B → C) must resolve regardless of
    /// the order they appear in the input slice. The previous single-pass
    /// implementation silently dropped downstream bindings whose source
    /// was defined by a binding that came later in the list.
    #[test]
    fn test_resolve_bindings_handles_reverse_order_chains() {
        use crate::cost::{Expr, VariableBinding};

        // Worker_requests = Queue_requests
        let upstream = VariableBinding {
            target: var("Worker_requests"),
            expr: Expr::variable(var("Queue_requests")),
            description: "Queue_requests".into(),
            source: "test".into(),
        };
        // Logs_ingestion = Worker_requests * 0.001
        let downstream = VariableBinding {
            target: var("Logs_ingestion"),
            expr: Expr::product(vec![
                Expr::variable(var("Worker_requests")),
                Expr::constant(0.001),
            ]),
            description: "Worker_requests * 0.001".into(),
            source: "test".into(),
        };

        // Adversarial order: downstream comes BEFORE its prerequisite.
        let bindings = vec![downstream, upstream];
        let base = params_from(&[("Queue_requests", 1_000_000.0)]);

        let resolved = resolve_bindings(&bindings, &base).unwrap();

        assert_eq!(
            resolved.get(&var("Worker_requests")).copied(),
            Some(1_000_000.0)
        );
        assert_eq!(resolved.get(&var("Logs_ingestion")).copied(), Some(1_000.0));
    }

    /// Resolution must terminate even when some bindings can never resolve
    /// (e.g. user forgot to supply a required base param).
    #[test]
    fn test_resolve_bindings_terminates_on_missing_source() {
        use crate::cost::{Expr, VariableBinding};

        let bindings = vec![VariableBinding {
            target: var("Derived"),
            expr: Expr::variable(var("NeverProvided")),
            description: String::new(),
            source: String::new(),
        }];
        let base = Params::default();
        let resolved = resolve_bindings(&bindings, &base).unwrap();
        // Unresolvable binding stays unset, but the function returns Ok.
        assert!(!resolved.contains_key(&var("Derived")));
    }

    // ---------------------------------------------------------------------------
    // Currency-mismatch guard at evaluate_architecture boundary
    // ---------------------------------------------------------------------------

    fn make_arch(resources: Vec<crate::cost::ResourceCost>) -> ArchitectureCost {
        use crate::cost::ArchitectureCost;
        use crate::topology::Topology;
        use crate::types::{ArchitectureName, Region};
        ArchitectureCost {
            name: ArchitectureName::new("test"),
            resources,
            bindings: vec![],
            region: Region::new("ap-northeast-1"),
            topology: Topology::default(),
            diagnostics: vec![],
        }
    }

    /// A struct-literal ResourceCost with mixed currencies (USD + JPY) must
    /// cause `evaluate_architecture` to fail with `ComponentCurrencyMismatch`
    /// rather than silently summing incompatible amounts.
    #[test]
    fn evaluate_architecture_rejects_mixed_currency_resource() {
        use crate::cost::{CostComponent, ResourceCost};
        use crate::types::{LogicalId, ResourceType};

        let rc = ResourceCost {
            logical_id: LogicalId::new("MixedResource"),
            resource_type: ResourceType::new("AWS::Foo::Bar"),
            label: "mixed".into(),
            expr: Expr::constant(101.0),
            components: vec![
                CostComponent::with_currency("usd_part", Expr::constant(1.0), "USD"),
                CostComponent::with_currency("jpy_part", Expr::constant(100.0), "JPY"),
            ],
            required_variables: vec![],
            currency: Some("USD".into()),
        };

        let arch = make_arch(vec![rc]);
        let result = evaluate_architecture(&arch, &Params::default());
        assert!(
            matches!(result, Err(CoreError::ComponentCurrencyMismatch { .. })),
            "expected ComponentCurrencyMismatch, got {result:?}"
        );
    }

    /// A ResourceCost with a single, consistent currency must pass through
    /// `evaluate_architecture` without error.
    #[test]
    fn evaluate_architecture_accepts_single_currency_resource() {
        use crate::cost::{CostComponent, ResourceCost};
        use crate::types::{LogicalId, ResourceType};

        let rc = ResourceCost {
            logical_id: LogicalId::new("UsdResource"),
            resource_type: ResourceType::new("AWS::Foo::Bar"),
            label: "usd".into(),
            expr: Expr::constant(10.0),
            components: vec![
                CostComponent::with_currency("part_a", Expr::constant(6.0), "USD"),
                CostComponent::with_currency("part_b", Expr::constant(4.0), "USD"),
            ],
            required_variables: vec![],
            currency: Some("USD".into()),
        };

        let arch = make_arch(vec![rc]);
        let result = evaluate_architecture(&arch, &Params::default());
        assert!(result.is_ok(), "single-currency resource must evaluate OK");
        let arch_result = result.unwrap();
        assert_eq!(
            arch_result.totals_by_currency.get("USD").copied(),
            Some(10.0)
        );
    }
}
