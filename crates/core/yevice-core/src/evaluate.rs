use std::collections::HashMap;

use crate::cost::{ArchitectureCost, Expr};
use crate::error::CoreError;
use crate::types::{ArchitectureName, LogicalId, ResourceType, VariableName};

/// Parameters for cost evaluation: variable name -> value.
pub type Params = HashMap<VariableName, f64>;

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
            if d == 0.0 { Ok(0.0) } else { Ok(n / d) }
        }
    }
}

/// Result of evaluating a single resource's cost.
#[derive(Debug)]
pub struct ResourceResult {
    pub logical_id: LogicalId,
    pub resource_type: ResourceType,
    pub label: String,
    pub monthly_cost: f64,
    /// Named cost component breakdown (name, cost).
    pub component_costs: Vec<(String, f64)>,
}

/// Result of evaluating an entire architecture's cost.
#[derive(Debug)]
pub struct ArchitectureResult {
    pub name: ArchitectureName,
    pub resources: Vec<ResourceResult>,
    pub total_monthly_cost: f64,
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
    let mut effective_params = resolve_bindings(&arch.bindings, params)?;

    // User-provided params override bindings
    for (k, v) in params {
        effective_params.insert(k.clone(), *v);
    }

    let mut resources = Vec::new();
    let mut total = 0.0;

    for rc in &arch.resources {
        let component_costs: Vec<(String, f64)> = rc
            .components
            .iter()
            .filter_map(|c| {
                evaluate(&c.expr, &effective_params)
                    .ok()
                    .map(|v| (c.name.clone(), v))
            })
            .collect();

        // Derive total from components when all evaluated successfully; otherwise re-evaluate expr.
        let cost = if component_costs.len() == rc.components.len() && !rc.components.is_empty() {
            component_costs.iter().map(|(_, v)| v).sum()
        } else {
            evaluate(&rc.expr, &effective_params)?
        };

        total += cost;
        resources.push(ResourceResult {
            logical_id: rc.logical_id.clone(),
            resource_type: rc.resource_type.clone(),
            label: rc.label.clone(),
            monthly_cost: cost,
            component_costs,
        });
    }

    Ok(ArchitectureResult {
        name: arch.name.clone(),
        resources,
        total_monthly_cost: total,
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

    let max_passes = bindings.len() + 1;
    for _ in 0..max_passes {
        let mut progressed = false;
        for binding in bindings {
            if params.contains_key(&binding.target) {
                continue;
            }
            if let Ok(value) = evaluate(&binding.expr, &params) {
                params.insert(binding.target.clone(), value);
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }

    Ok(params)
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
    fn test_constant() {
        let expr = Expr::constant(42.0);
        assert_eq!(evaluate(&expr, &Params::new()).unwrap(), 42.0);
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
        assert!(evaluate(&expr, &Params::new()).is_err());
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
        let base = Params::new();
        let resolved = resolve_bindings(&bindings, &base).unwrap();
        // Unresolvable binding stays unset, but the function returns Ok.
        assert!(!resolved.contains_key(&var("Derived")));
    }
}
