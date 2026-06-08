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
use crate::resource::{Architecture, Connection, ConnectionType};
use crate::types::{LogicalId, VariableName};

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

/// Returns the source rate variable and a display label for a resource, given
/// its service_id (or a hint string for external sources).
fn source_rate_var(
    arch: &Architecture,
    id: &LogicalId,
    hint: Option<&str>,
) -> Option<(VariableName, &'static str)> {
    let service_id = arch.find_resource(id).map(|r| r.shell.service_id.as_str());
    match service_id {
        Some("aws.sqs") => Some((id.var("requests"), "SQS")),
        Some("aws.kinesis") => Some((id.var("put_records"), "Kinesis")),
        Some("aws.dynamodb") => Some((id.var("write_request_units"), "DynamoDB")),
        Some("aws.lambda") => Some((id.var("requests"), "Lambda")),
        Some("aws.s3") => Some((id.var("put_requests"), "S3")),
        _ => match hint {
            Some("sqs") => Some((id.var("requests"), "SQS")),
            Some("kinesis") => Some((id.var("put_records"), "Kinesis")),
            Some("dynamodb") => Some((id.var("write_request_units"), "DynamoDB")),
            Some("lambda") => Some((id.var("requests"), "Lambda")),
            Some("s3") => Some((id.var("put_requests"), "S3")),
            _ => None,
        },
    }
}

fn scale_expr(expr: Expr, factor: f64) -> Expr {
    if (factor - 1.0).abs() < f64::EPSILON {
        expr
    } else {
        Expr::product(vec![expr, Expr::constant(factor)])
    }
}

fn scale_description(base: impl Into<String>, factor: f64) -> String {
    let base = base.into();
    if (factor - 1.0).abs() < f64::EPSILON {
        base
    } else {
        format!("{base} * {factor}")
    }
}

fn scaled_binding(
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

fn derive_event_source_binding(conn: &Connection, arch: &Architecture) -> Option<VariableBinding> {
    let batch_size = conn.batch_size.unwrap_or(1.0);
    let parallelization = conn.parallelization_factor.unwrap_or(1.0);
    let (source_var, source_type) =
        source_rate_var(arch, &conn.source, conn.source_hint.as_deref())?;

    let base_expr = Expr::ceil(Expr::div(
        Expr::variable(source_var.clone()),
        Expr::constant(batch_size),
    ));
    let expr = scale_expr(base_expr, parallelization);
    let description = scale_description(
        format!("ceil({source_var} / {batch_size})"),
        parallelization,
    );

    Some(VariableBinding {
        target: conn.target.var("requests"),
        expr,
        description,
        source: format!(
            "{source_type} -> Lambda ({} -> {}, batch={batch_size})",
            conn.source, conn.target
        ),
    })
}

fn derive_invocation_binding(conn: &Connection, arch: &Architecture) -> Option<VariableBinding> {
    let factor = conn.factor.unwrap_or(1.0);
    let (source_var, source_type) =
        source_rate_var(arch, &conn.source, conn.source_hint.as_deref())?;

    let target_resource = arch.find_resource(&conn.target);
    let target_service_id = target_resource.map(|r| r.shell.service_id.as_str());
    let workflow_type =
        target_resource.and_then(|r| r.shell.metadata.get("workflow_type").map(String::as_str));

    let (target_var, target_type) = match target_service_id {
        Some("aws.lambda") => (conn.target.var("requests"), "Lambda"),
        Some("aws.step_functions") => match workflow_type {
            Some("express") => (conn.target.var("requests"), "Step Functions"),
            _ => (conn.target.var("transitions"), "Step Functions"),
        },
        _ => return None,
    };

    Some(scaled_binding(
        target_var,
        &source_var,
        factor,
        format!(
            "{source_type} -> {target_type} ({} -> {})",
            conn.source, conn.target
        ),
    ))
}

fn derive_dataflow_bindings(conn: &Connection, arch: &Architecture) -> Vec<VariableBinding> {
    let factor = conn.factor.unwrap_or(1.0);
    let Some((source_var, source_type)) =
        source_rate_var(arch, &conn.source, conn.source_hint.as_deref())
    else {
        return Vec::new();
    };

    let target_resource = arch.find_resource(&conn.target);
    let target_service_id = target_resource.map(|r| r.shell.service_id.as_str());
    let billing_mode =
        target_resource.and_then(|r| r.shell.metadata.get("billing_mode").map(String::as_str));

    match target_service_id {
        Some("aws.dynamodb") if billing_mode == Some("on_demand") => {
            vec![scaled_binding(
                conn.target.var("write_request_units"),
                &source_var,
                factor,
                format!(
                    "{source_type} -> DynamoDB ({} -> {})",
                    conn.source, conn.target
                ),
            )]
        }
        Some("aws.sns") => vec![scaled_binding(
            conn.target.var("deliveries"),
            &source_var,
            factor,
            format!("{source_type} -> SNS ({} -> {})", conn.source, conn.target),
        )],
        Some("aws.sqs") => vec![scaled_binding(
            conn.target.var("requests"),
            &source_var,
            factor,
            format!("{source_type} -> SQS ({} -> {})", conn.source, conn.target),
        )],
        _ => Vec::new(),
    }
}

fn derive_notification_binding(conn: &Connection, arch: &Architecture) -> Option<VariableBinding> {
    let factor = conn.factor.unwrap_or(1.0);
    let (source_var, source_type) =
        source_rate_var(arch, &conn.source, conn.source_hint.as_deref())?;
    if source_type != "S3" {
        return None;
    }

    let target_service_id = arch
        .find_resource(&conn.target)
        .map(|r| r.shell.service_id.as_str());

    let (target_var, target_type) = match target_service_id {
        Some("aws.lambda") => (conn.target.var("requests"), "Lambda"),
        Some("aws.sqs") => (conn.target.var("requests"), "SQS"),
        _ => return None,
    };

    Some(scaled_binding(
        target_var,
        &source_var,
        factor,
        format!(
            "{source_type} -> {target_type} ({} -> {})",
            conn.source, conn.target
        ),
    ))
}

/// Derive variable bindings from the architecture's connection graph.
///
/// This function inspects the connections and automatically creates
/// `VariableBinding`s that relate upstream resource usage to downstream
/// resource invocations — for example, deriving Lambda invocation counts
/// from SQS message counts.
pub fn derive_bindings(arch: &Architecture) -> Vec<VariableBinding> {
    let mut map: HashMap<VariableName, VariableBinding> = HashMap::new();
    let mut order: Vec<VariableName> = Vec::new();

    for conn in &arch.connections {
        match conn.connection_type {
            ConnectionType::EventSource => {
                if let Some(binding) = derive_event_source_binding(conn, arch) {
                    upsert_binding(&mut map, &mut order, binding);
                }
            }
            ConnectionType::Invocation => {
                if let Some(binding) = derive_invocation_binding(conn, arch) {
                    upsert_binding(&mut map, &mut order, binding);
                }
            }
            ConnectionType::DataFlow => {
                for binding in derive_dataflow_bindings(conn, arch) {
                    upsert_binding(&mut map, &mut order, binding);
                }
            }
            ConnectionType::Notification => {
                if let Some(binding) = derive_notification_binding(conn, arch) {
                    upsert_binding(&mut map, &mut order, binding);
                }
            }
        }
    }

    // Walk `order` so that bindings appear in insertion (~topological) order:
    // an upstream binding (`Worker_requests = QueueA_requests`) is emitted
    // before any downstream binding that consumes `Worker_requests`. This
    // matches the order connections are visited in `arch.connections`, which
    // typically reflects the graph's data-flow direction.
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

    #[test]
    fn test_fan_in_bindings_are_summed() {
        use crate::resource::{
            Architecture, Connection, ConnectionType, Provider, Resource, ResourceShell,
        };
        use crate::types::{LogicalId, Region, ResourceType};

        let lambda_shell = ResourceShell::new("aws.lambda", Provider::Aws, &serde_json::json!({}));
        let sqs_a_shell = ResourceShell::new("aws.sqs", Provider::Aws, &serde_json::json!({}));
        let sqs_b_shell = ResourceShell::new("aws.sqs", Provider::Aws, &serde_json::json!({}));

        let arch = Architecture {
            name: "fan-in".into(),
            region: Region::new("ap-northeast-1"),
            resources: vec![
                Resource {
                    logical_id: LogicalId::new("Worker"),
                    resource_type: ResourceType::new("AWS::Lambda::Function"),
                    shell: lambda_shell,
                },
                Resource {
                    logical_id: LogicalId::new("QueueA"),
                    resource_type: ResourceType::new("AWS::SQS::Queue"),
                    shell: sqs_a_shell,
                },
                Resource {
                    logical_id: LogicalId::new("QueueB"),
                    resource_type: ResourceType::new("AWS::SQS::Queue"),
                    shell: sqs_b_shell,
                },
            ],
            connections: vec![
                Connection {
                    source: LogicalId::new("QueueA"),
                    target: LogicalId::new("Worker"),
                    connection_type: ConnectionType::EventSource,
                    batch_size: Some(1.0),
                    parallelization_factor: Some(1.0),
                    factor: None,
                    source_hint: None,
                },
                Connection {
                    source: LogicalId::new("QueueB"),
                    target: LogicalId::new("Worker"),
                    connection_type: ConnectionType::EventSource,
                    batch_size: Some(1.0),
                    parallelization_factor: Some(1.0),
                    factor: None,
                    source_hint: None,
                },
            ],
        };

        let bindings = derive_bindings(&arch);

        let worker_binding = bindings
            .iter()
            .find(|b| b.target == LogicalId::new("Worker").var("requests"))
            .expect("Worker_requests binding should exist");

        // With QueueA_requests=300 and QueueB_requests=200, summed result must be 500
        // (not just whichever source happened to be processed last).
        let params = params_from(&[("QueueA_requests", 300.0), ("QueueB_requests", 200.0)]);
        let result = crate::evaluate::evaluate(&worker_binding.expr, &params).unwrap();
        assert_eq!(
            result, 500.0,
            "fan-in bindings must sum, not overwrite — got {result}"
        );
    }
}
