//! User-defined variable bindings for relationships not detectable from CFn,
//! plus auto-derived bindings from the architecture connection graph.
//!
//! # User-defined binding modes
//!
//! 1. Simple: `source` + `factor` + `batch_size`
//!    ```yaml
//!    - target: ProcessorFunction_requests
//!      source: Workflow_transitions
//!      factor: 3
//!    ```
//!
//! 2. Expression: `expr` with full arithmetic
//!    ```yaml
//!    - target: OutputBucket_storage_gb
//!      expr: "ProcessingJob_executions * OutputBucket_avg_object_size_gb * OutputBucket_retention_days / 30"
//!    ```

use std::collections::HashMap;

use serde::Deserialize;

use crate::cost::{Expr, VariableBinding};
use crate::expr_parser;
use crate::resource::{Architecture, Connection};
use crate::types::VariableName;

/// User-defined bindings file structure.
#[derive(Debug, Deserialize)]
pub struct BindingsFile {
    #[serde(default)]
    pub bindings: Vec<UserBinding>,
}

/// A single user-defined binding.
#[derive(Debug, Deserialize)]
pub struct UserBinding {
    /// Target variable name.
    pub target: String,
    /// Source variable name (for simple mode). Mutually exclusive with `expr`.
    #[serde(default)]
    pub source: Option<String>,
    /// Expression string (for expression mode). Mutually exclusive with `source`.
    #[serde(default)]
    pub expr: Option<String>,
    /// Multiplier factor (simple mode only, default: 1.0).
    #[serde(default = "default_factor")]
    pub factor: f64,
    /// Batch size (simple mode only, default: 1.0).
    #[serde(default = "default_batch")]
    pub batch_size: f64,
    /// Human-readable description.
    #[serde(default)]
    pub description: String,
}

fn default_factor() -> f64 {
    1.0
}
fn default_batch() -> f64 {
    1.0
}

/// Convert user-defined bindings to `VariableBinding`s.
pub fn to_variable_bindings(user_bindings: &[UserBinding]) -> Vec<VariableBinding> {
    user_bindings
        .iter()
        .filter_map(|ub| {
            let (expr, auto_desc) = if let Some(expr_str) = &ub.expr {
                // Expression mode: parse the expression string
                match expr_parser::parse_expr(expr_str) {
                    Ok(parsed) => (parsed, expr_str.clone()),
                    Err(e) => {
                        tracing::warn!(
                            target = ub.target,
                            expr = expr_str.as_str(),
                            error = %e,
                            "failed to parse binding expression, skipping"
                        );
                        return None;
                    }
                }
            } else if let Some(source) = &ub.source {
                // Simple mode: source + factor + batch_size
                let source_var = Expr::variable(VariableName::new(source));
                let built = build_simple_expr(source_var, ub.batch_size, ub.factor);
                let desc = build_simple_description(source, ub.batch_size, ub.factor);
                (built, desc)
            } else {
                tracing::warn!(
                    target = ub.target,
                    "binding must have either 'source' or 'expr', skipping"
                );
                return None;
            };

            let description = if ub.description.is_empty() {
                auto_desc
            } else {
                ub.description.clone()
            };

            let source_label = if let Some(src) = &ub.source {
                format!("user-defined ({src} -> {})", ub.target)
            } else {
                format!("user-defined (expr -> {})", ub.target)
            };

            Some(VariableBinding {
                target: VariableName::new(&ub.target),
                expr,
                description,
                source: source_label,
            })
        })
        .collect()
}

fn build_simple_expr(source_var: Expr, batch_size: f64, factor: f64) -> Expr {
    if batch_size > 1.0 {
        let ceiled = Expr::ceil(Expr::div(source_var, Expr::constant(batch_size)));
        if (factor - 1.0).abs() < f64::EPSILON {
            ceiled
        } else {
            Expr::product(vec![ceiled, Expr::constant(factor)])
        }
    } else if (factor - 1.0).abs() < f64::EPSILON {
        source_var
    } else {
        Expr::product(vec![source_var, Expr::constant(factor)])
    }
}

fn build_simple_description(source: &str, batch_size: f64, factor: f64) -> String {
    let mut desc = source.to_string();
    if batch_size > 1.0 {
        desc = format!("ceil({source} / {batch_size})");
    }
    if (factor - 1.0).abs() > f64::EPSILON {
        desc = format!("{desc} * {factor}");
    }
    desc
}

// ===========================================================================
// Auto-derived bindings from the Architecture connection graph
// ===========================================================================

/// A rule that derives variable bindings from a single connection.
///
/// Provider-specific binding logic lives in the service crates that
/// implement this trait; core only walks the graph and aggregates.
pub trait ConnectionRule: Send + Sync {
    fn derive(&self, conn: &Connection, arch: &Architecture) -> Vec<VariableBinding>;
}

/// Scale `expr` by `factor` (identity if factor is 1.0).
pub fn scale_expr(expr: Expr, factor: f64) -> Expr {
    if (factor - 1.0).abs() < f64::EPSILON {
        expr
    } else {
        Expr::product(vec![expr, Expr::constant(factor)])
    }
}

pub fn scale_description(base: impl Into<String>, factor: f64) -> String {
    let base = base.into();
    if (factor - 1.0).abs() < f64::EPSILON {
        base
    } else {
        format!("{base} * {factor}")
    }
}

pub fn scaled_binding(
    target: VariableName,
    source_var: &VariableName,
    factor: f64,
    source: String,
) -> VariableBinding {
    VariableBinding {
        target,
        expr: scale_expr(Expr::variable(source_var.clone()), factor),
        description: scale_description(source_var.to_string(), factor),
        source,
    }
}

fn upsert_binding(
    map: &mut HashMap<VariableName, VariableBinding>,
    order: &mut Vec<VariableName>,
    binding: VariableBinding,
) {
    use std::collections::hash_map::Entry;
    let target = binding.target.clone();
    match map.entry(target.clone()) {
        Entry::Vacant(e) => {
            e.insert(binding);
            order.push(target);
        }
        Entry::Occupied(mut e) => {
            let existing = e.get().clone();
            e.insert(VariableBinding {
                target: binding.target,
                expr: Expr::sum(vec![existing.expr, binding.expr]),
                description: format!("{} + {}", existing.description, binding.description),
                source: format!("{} + {}", existing.source, binding.source),
            });
            // First-insertion order is preserved.
        }
    }
}

/// Derive variable bindings from the architecture's connection graph.
///
/// This function walks every connection and asks each `ConnectionRule` to
/// produce bindings. Provider-specific rules live in their respective service
/// crates; core is only responsible for the graph-walk and upsert aggregation.
pub fn derive_bindings(
    arch: &Architecture,
    rules: &[Box<dyn ConnectionRule>],
) -> Vec<VariableBinding> {
    let mut map: HashMap<VariableName, VariableBinding> = HashMap::new();
    let mut order: Vec<VariableName> = Vec::new();
    for conn in &arch.connections {
        for rule in rules {
            for binding in rule.derive(conn, arch) {
                upsert_binding(&mut map, &mut order, binding);
            }
        }
    }
    let mut bindings = Vec::with_capacity(order.len());
    for name in order {
        if let Some(binding) = map.remove(&name) {
            bindings.push(binding);
        }
    }
    bindings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evaluate::Params;

    fn params_from(pairs: &[(&str, f64)]) -> Params {
        pairs
            .iter()
            .map(|(k, v)| (VariableName::new(*k), *v))
            .collect()
    }

    #[test]
    fn test_simple_1to1_binding() {
        let bindings = to_variable_bindings(&[UserBinding {
            target: "Workflow_transitions".into(),
            source: Some("TriggerFunction_requests".into()),
            expr: None,
            factor: 1.0,
            batch_size: 1.0,
            description: String::new(),
        }]);

        assert_eq!(bindings.len(), 1);
        let params = params_from(&[("TriggerFunction_requests", 1000.0)]);
        let result = crate::evaluate::evaluate(&bindings[0].expr, &params).unwrap();
        assert_eq!(result, 1000.0);
    }

    #[test]
    fn test_factor_binding() {
        let bindings = to_variable_bindings(&[UserBinding {
            target: "ProcessorFunction_requests".into(),
            source: Some("Workflow_transitions".into()),
            expr: None,
            factor: 3.0,
            batch_size: 1.0,
            description: String::new(),
        }]);

        let params = params_from(&[("Workflow_transitions", 500.0)]);
        let result = crate::evaluate::evaluate(&bindings[0].expr, &params).unwrap();
        assert_eq!(result, 1500.0);
    }

    #[test]
    fn test_batch_binding() {
        let bindings = to_variable_bindings(&[UserBinding {
            target: "BatchFunction_requests".into(),
            source: Some("DataStream_put_records".into()),
            expr: None,
            factor: 1.0,
            batch_size: 100.0,
            description: String::new(),
        }]);

        let params = params_from(&[("DataStream_put_records", 1050.0)]);
        let result = crate::evaluate::evaluate(&bindings[0].expr, &params).unwrap();
        assert_eq!(result, 11.0);
    }

    #[test]
    fn test_expr_binding() {
        let bindings = to_variable_bindings(&[UserBinding {
            target: "OutputBucket_storage_gb".into(),
            source: None,
            expr: Some("executions * avg_size_gb * retention_days / 30".into()),
            factor: 1.0,
            batch_size: 1.0,
            description: "S3 average storage".into(),
        }]);

        assert_eq!(bindings.len(), 1);
        let params = params_from(&[
            ("executions", 1000.0),
            ("avg_size_gb", 0.7),
            ("retention_days", 7.0),
        ]);
        let result = crate::evaluate::evaluate(&bindings[0].expr, &params).unwrap();
        assert!((result - 163.333).abs() < 0.01);
    }

    #[test]
    fn test_expr_with_ceil() {
        let bindings = to_variable_bindings(&[UserBinding {
            target: "executions".into(),
            source: None,
            expr: Some("ceil(transitions / 3)".into()),
            factor: 1.0,
            batch_size: 1.0,
            description: String::new(),
        }]);

        let params = params_from(&[("transitions", 3000.0)]);
        let result = crate::evaluate::evaluate(&bindings[0].expr, &params).unwrap();
        assert_eq!(result, 1000.0);
    }

    /// Minimal no-op rule for testing the skeleton itself.
    struct NoopRule;
    impl ConnectionRule for NoopRule {
        fn derive(&self, _conn: &Connection, _arch: &Architecture) -> Vec<VariableBinding> {
            Vec::new()
        }
    }

    #[test]
    fn test_derive_bindings_empty_rules_returns_empty() {
        use crate::resource::Architecture;
        use crate::types::Region;

        let arch = Architecture {
            name: "empty".into(),
            region: Region::new("ap-northeast-1"),
            resources: vec![],
            connections: vec![],
        };

        let rules: Vec<Box<dyn ConnectionRule>> = vec![Box::new(NoopRule)];
        let bindings = derive_bindings(&arch, &rules);
        assert!(bindings.is_empty());
    }
}
