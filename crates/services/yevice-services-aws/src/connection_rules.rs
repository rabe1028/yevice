//! AWS-specific [`ConnectionRule`] implementations.
//!
//! These rules encode the AWS binding logic that was previously embedded
//! inside `yevice-core`. Core only walks the graph; these rules supply
//! the AWS service-id knowledge (e.g. `"aws.sqs"`, `"aws.lambda"`).

use yevice_core::bindings::{ConnectionRule, scale_description, scale_expr, scaled_binding};
use yevice_core::cost::{Expr, VariableBinding};
use yevice_core::resource::{Architecture, Connection, ConnectionType};
use yevice_core::types::{LogicalId, VariableName};

// ---------------------------------------------------------------------------
// Private helper — mirrors the old `source_rate_var` in core
// ---------------------------------------------------------------------------

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

/// Resolve the source rate variable for a connection, applying the `source_hint`
/// fallback. Shared by every rule's "skip if no known source" guard.
fn conn_source_rate(
    conn: &Connection,
    arch: &Architecture,
) -> Option<(VariableName, &'static str)> {
    source_rate_var(arch, &conn.source, conn.source_hint.as_deref())
}

// ---------------------------------------------------------------------------
// EventSource rule  (SQS / Kinesis / DDB Stream → Lambda)
// ---------------------------------------------------------------------------

pub struct AwsEventSourceRule;

impl ConnectionRule for AwsEventSourceRule {
    fn derive(&self, conn: &Connection, arch: &Architecture) -> Vec<VariableBinding> {
        if !matches!(conn.connection_type, ConnectionType::EventSource) {
            return Vec::new();
        }

        let batch_size = conn.batch_size.unwrap_or(1.0);
        let parallelization = conn.parallelization_factor.unwrap_or(1.0);
        let Some((source_var, source_type)) = conn_source_rate(conn, arch) else {
            return Vec::new();
        };

        let base_expr = Expr::ceil(Expr::div(
            Expr::variable(source_var.clone()),
            Expr::constant(batch_size),
        ));
        let expr = scale_expr(base_expr, parallelization);
        let description = scale_description(
            format!("ceil({source_var} / {batch_size})"),
            parallelization,
        );

        vec![VariableBinding {
            target: conn.target.var("requests"),
            expr,
            description,
            source: format!(
                "{source_type} -> Lambda ({} -> {}, batch={batch_size})",
                conn.source, conn.target
            ),
        }]
    }
}

// ---------------------------------------------------------------------------
// Invocation rule  (Lambda / SF Express → Lambda / StepFunctions)
// ---------------------------------------------------------------------------

pub struct AwsInvocationRule;

impl ConnectionRule for AwsInvocationRule {
    fn derive(&self, conn: &Connection, arch: &Architecture) -> Vec<VariableBinding> {
        if !matches!(conn.connection_type, ConnectionType::Invocation) {
            return Vec::new();
        }

        let factor = conn.factor.unwrap_or(1.0);
        let Some((source_var, source_type)) = conn_source_rate(conn, arch) else {
            return Vec::new();
        };

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
            _ => return Vec::new(),
        };

        vec![scaled_binding(
            target_var,
            &source_var,
            factor,
            format!(
                "{source_type} -> {target_type} ({} -> {})",
                conn.source, conn.target
            ),
        )]
    }
}

// ---------------------------------------------------------------------------
// DataFlow rule  (Lambda → DynamoDB / SNS / SQS)
// ---------------------------------------------------------------------------

pub struct AwsDataFlowRule;

impl ConnectionRule for AwsDataFlowRule {
    fn derive(&self, conn: &Connection, arch: &Architecture) -> Vec<VariableBinding> {
        if !matches!(conn.connection_type, ConnectionType::DataFlow) {
            return Vec::new();
        }

        let factor = conn.factor.unwrap_or(1.0);
        let Some((source_var, source_type)) = conn_source_rate(conn, arch) else {
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
}

// ---------------------------------------------------------------------------
// Notification rule  (S3 → Lambda / SQS)
// ---------------------------------------------------------------------------

pub struct AwsNotificationRule;

impl ConnectionRule for AwsNotificationRule {
    fn derive(&self, conn: &Connection, arch: &Architecture) -> Vec<VariableBinding> {
        if !matches!(conn.connection_type, ConnectionType::Notification) {
            return Vec::new();
        }

        let factor = conn.factor.unwrap_or(1.0);
        let Some((source_var, source_type)) = conn_source_rate(conn, arch) else {
            return Vec::new();
        };
        if source_type != "S3" {
            return Vec::new();
        }

        let target_service_id = arch
            .find_resource(&conn.target)
            .map(|r| r.shell.service_id.as_str());

        let (target_var, target_type) = match target_service_id {
            Some("aws.lambda") => (conn.target.var("requests"), "Lambda"),
            Some("aws.sqs") => (conn.target.var("requests"), "SQS"),
            _ => return Vec::new(),
        };

        vec![scaled_binding(
            target_var,
            &source_var,
            factor,
            format!(
                "{source_type} -> {target_type} ({} -> {})",
                conn.source, conn.target
            ),
        )]
    }
}

// ---------------------------------------------------------------------------
// Registry helper
// ---------------------------------------------------------------------------

/// Build the full set of AWS connection rules in the canonical dispatch order:
/// `[EventSource, Invocation, DataFlow, Notification]`.
///
/// Pass the returned `Vec` to [`yevice_core::bindings::derive_bindings`] or
/// store it in the `ServiceCatalog`.
pub fn aws_connection_rules() -> Vec<Box<dyn ConnectionRule>> {
    vec![
        Box::new(AwsEventSourceRule),
        Box::new(AwsInvocationRule),
        Box::new(AwsDataFlowRule),
        Box::new(AwsNotificationRule),
    ]
}

// ---------------------------------------------------------------------------
// Tests (migrated from yevice-core::bindings)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use yevice_core::bindings::derive_bindings;
    use yevice_core::evaluate::Params;
    use yevice_core::resource::{Architecture, Provider, Resource, ResourceShell};
    use yevice_core::types::{LogicalId, Region, ResourceType, VariableName};

    fn params_from(pairs: &[(&str, f64)]) -> Params {
        pairs
            .iter()
            .map(|(k, v)| (VariableName::new(*k), *v))
            .collect()
    }

    fn all_rules() -> Vec<Box<dyn ConnectionRule>> {
        aws_connection_rules()
    }

    fn make_resource(logical_id: &str, service_id: &str) -> Resource {
        Resource {
            logical_id: LogicalId::new(logical_id),
            resource_type: ResourceType::new("AWS::Unknown"),
            shell: ResourceShell::new(service_id, Provider::Aws, &serde_json::json!({})),
            group: None,
        }
    }

    fn make_resource_with_meta(
        logical_id: &str,
        service_id: &str,
        meta_key: &str,
        meta_val: &str,
    ) -> Resource {
        Resource {
            logical_id: LogicalId::new(logical_id),
            resource_type: ResourceType::new("AWS::Unknown"),
            shell: ResourceShell::new(service_id, Provider::Aws, &serde_json::json!({}))
                .with_metadata(meta_key, meta_val),
            group: None,
        }
    }

    // -----------------------------------------------------------------------
    // EventSource
    // -----------------------------------------------------------------------

    #[test]
    fn test_event_source_sqs_to_lambda() {
        let arch = Architecture {
            name: "test".into(),
            region: Region::new("ap-northeast-1"),
            resources: vec![
                make_resource("Queue", "aws.sqs"),
                make_resource("Worker", "aws.lambda"),
            ],
            connections: vec![Connection {
                source: LogicalId::new("Queue"),
                target: LogicalId::new("Worker"),
                connection_type: ConnectionType::EventSource,
                batch_size: Some(10.0),
                parallelization_factor: Some(1.0),
                factor: None,
                source_hint: None,
            }],
        };

        let bindings = derive_bindings(&arch, &all_rules());
        assert_eq!(bindings.len(), 1);
        let b = &bindings[0];
        assert_eq!(b.target, LogicalId::new("Worker").var("requests"));

        let params = params_from(&[("Queue_requests", 1000.0)]);
        let result = yevice_core::evaluate::evaluate(&b.expr, &params).unwrap();
        // ceil(1000 / 10) = 100
        assert_eq!(result, 100.0);
    }

    #[test]
    fn test_event_source_with_parallelization() {
        let arch = Architecture {
            name: "test".into(),
            region: Region::new("ap-northeast-1"),
            resources: vec![
                make_resource("Stream", "aws.kinesis"),
                make_resource("Processor", "aws.lambda"),
            ],
            connections: vec![Connection {
                source: LogicalId::new("Stream"),
                target: LogicalId::new("Processor"),
                connection_type: ConnectionType::EventSource,
                batch_size: Some(1.0),
                parallelization_factor: Some(3.0),
                factor: None,
                source_hint: None,
            }],
        };

        let bindings = derive_bindings(&arch, &all_rules());
        assert_eq!(bindings.len(), 1);
        let params = params_from(&[("Stream_put_records", 500.0)]);
        let result = yevice_core::evaluate::evaluate(&bindings[0].expr, &params).unwrap();
        // ceil(500/1) * 3 = 1500
        assert_eq!(result, 1500.0);
    }

    // -----------------------------------------------------------------------
    // Invocation
    // -----------------------------------------------------------------------

    #[test]
    fn test_invocation_lambda_to_lambda() {
        let arch = Architecture {
            name: "test".into(),
            region: Region::new("ap-northeast-1"),
            resources: vec![
                make_resource("Caller", "aws.lambda"),
                make_resource("Callee", "aws.lambda"),
            ],
            connections: vec![Connection {
                source: LogicalId::new("Caller"),
                target: LogicalId::new("Callee"),
                connection_type: ConnectionType::Invocation,
                batch_size: None,
                parallelization_factor: None,
                factor: Some(2.0),
                source_hint: None,
            }],
        };

        let bindings = derive_bindings(&arch, &all_rules());
        assert_eq!(bindings.len(), 1);
        let params = params_from(&[("Caller_requests", 100.0)]);
        let result = yevice_core::evaluate::evaluate(&bindings[0].expr, &params).unwrap();
        assert_eq!(result, 200.0);
    }

    #[test]
    fn test_invocation_lambda_to_step_functions_standard() {
        let arch = Architecture {
            name: "test".into(),
            region: Region::new("ap-northeast-1"),
            resources: vec![
                make_resource("Trigger", "aws.lambda"),
                make_resource("Workflow", "aws.step_functions"),
            ],
            connections: vec![Connection {
                source: LogicalId::new("Trigger"),
                target: LogicalId::new("Workflow"),
                connection_type: ConnectionType::Invocation,
                batch_size: None,
                parallelization_factor: None,
                factor: Some(1.0),
                source_hint: None,
            }],
        };

        let bindings = derive_bindings(&arch, &all_rules());
        assert_eq!(bindings.len(), 1);
        // Standard Step Functions → transitions (not requests)
        assert_eq!(
            bindings[0].target,
            LogicalId::new("Workflow").var("transitions")
        );
    }

    #[test]
    fn test_invocation_lambda_to_step_functions_express() {
        let arch = Architecture {
            name: "test".into(),
            region: Region::new("ap-northeast-1"),
            resources: vec![
                make_resource("Trigger", "aws.lambda"),
                make_resource_with_meta(
                    "Workflow",
                    "aws.step_functions",
                    "workflow_type",
                    "express",
                ),
            ],
            connections: vec![Connection {
                source: LogicalId::new("Trigger"),
                target: LogicalId::new("Workflow"),
                connection_type: ConnectionType::Invocation,
                batch_size: None,
                parallelization_factor: None,
                factor: Some(1.0),
                source_hint: None,
            }],
        };

        let bindings = derive_bindings(&arch, &all_rules());
        assert_eq!(bindings.len(), 1);
        // Express Step Functions → requests
        assert_eq!(
            bindings[0].target,
            LogicalId::new("Workflow").var("requests")
        );
    }

    // -----------------------------------------------------------------------
    // DataFlow
    // -----------------------------------------------------------------------

    #[test]
    fn test_dataflow_lambda_to_dynamodb_on_demand() {
        let arch = Architecture {
            name: "test".into(),
            region: Region::new("ap-northeast-1"),
            resources: vec![
                make_resource("Writer", "aws.lambda"),
                make_resource_with_meta("Table", "aws.dynamodb", "billing_mode", "on_demand"),
            ],
            connections: vec![Connection {
                source: LogicalId::new("Writer"),
                target: LogicalId::new("Table"),
                connection_type: ConnectionType::DataFlow,
                batch_size: None,
                parallelization_factor: None,
                factor: Some(1.0),
                source_hint: None,
            }],
        };

        let bindings = derive_bindings(&arch, &all_rules());
        assert_eq!(bindings.len(), 1);
        assert_eq!(
            bindings[0].target,
            LogicalId::new("Table").var("write_request_units")
        );
        let params = params_from(&[("Writer_requests", 300.0)]);
        let result = yevice_core::evaluate::evaluate(&bindings[0].expr, &params).unwrap();
        assert_eq!(result, 300.0);
    }

    #[test]
    fn test_dataflow_lambda_to_sns() {
        let arch = Architecture {
            name: "test".into(),
            region: Region::new("ap-northeast-1"),
            resources: vec![
                make_resource("Publisher", "aws.lambda"),
                make_resource("Topic", "aws.sns"),
            ],
            connections: vec![Connection {
                source: LogicalId::new("Publisher"),
                target: LogicalId::new("Topic"),
                connection_type: ConnectionType::DataFlow,
                batch_size: None,
                parallelization_factor: None,
                factor: Some(1.0),
                source_hint: None,
            }],
        };

        let bindings = derive_bindings(&arch, &all_rules());
        assert_eq!(bindings.len(), 1);
        assert_eq!(
            bindings[0].target,
            LogicalId::new("Topic").var("deliveries")
        );
    }

    #[test]
    fn test_dataflow_lambda_to_sqs() {
        let arch = Architecture {
            name: "test".into(),
            region: Region::new("ap-northeast-1"),
            resources: vec![
                make_resource("Sender", "aws.lambda"),
                make_resource("Queue", "aws.sqs"),
            ],
            connections: vec![Connection {
                source: LogicalId::new("Sender"),
                target: LogicalId::new("Queue"),
                connection_type: ConnectionType::DataFlow,
                batch_size: None,
                parallelization_factor: None,
                factor: Some(1.0),
                source_hint: None,
            }],
        };

        let bindings = derive_bindings(&arch, &all_rules());
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].target, LogicalId::new("Queue").var("requests"));
    }

    // -----------------------------------------------------------------------
    // Notification
    // -----------------------------------------------------------------------

    #[test]
    fn test_notification_s3_to_lambda() {
        let arch = Architecture {
            name: "test".into(),
            region: Region::new("ap-northeast-1"),
            resources: vec![
                make_resource("Bucket", "aws.s3"),
                make_resource("Handler", "aws.lambda"),
            ],
            connections: vec![Connection {
                source: LogicalId::new("Bucket"),
                target: LogicalId::new("Handler"),
                connection_type: ConnectionType::Notification,
                batch_size: None,
                parallelization_factor: None,
                factor: Some(1.0),
                source_hint: None,
            }],
        };

        let bindings = derive_bindings(&arch, &all_rules());
        assert_eq!(bindings.len(), 1);
        assert_eq!(
            bindings[0].target,
            LogicalId::new("Handler").var("requests")
        );
        let params = params_from(&[("Bucket_put_requests", 200.0)]);
        let result = yevice_core::evaluate::evaluate(&bindings[0].expr, &params).unwrap();
        assert_eq!(result, 200.0);
    }

    #[test]
    fn test_notification_s3_to_sqs() {
        let arch = Architecture {
            name: "test".into(),
            region: Region::new("ap-northeast-1"),
            resources: vec![
                make_resource("Bucket", "aws.s3"),
                make_resource("Queue", "aws.sqs"),
            ],
            connections: vec![Connection {
                source: LogicalId::new("Bucket"),
                target: LogicalId::new("Queue"),
                connection_type: ConnectionType::Notification,
                batch_size: None,
                parallelization_factor: None,
                factor: Some(1.0),
                source_hint: None,
            }],
        };

        let bindings = derive_bindings(&arch, &all_rules());
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].target, LogicalId::new("Queue").var("requests"));
    }

    // -----------------------------------------------------------------------
    // Fan-in (migrated from core)
    // -----------------------------------------------------------------------

    #[test]
    fn test_fan_in_bindings_are_summed() {
        let arch = Architecture {
            name: "fan-in".into(),
            region: Region::new("ap-northeast-1"),
            resources: vec![
                make_resource("Worker", "aws.lambda"),
                make_resource("QueueA", "aws.sqs"),
                make_resource("QueueB", "aws.sqs"),
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

        let bindings = derive_bindings(&arch, &all_rules());

        let worker_binding = bindings
            .iter()
            .find(|b| b.target == LogicalId::new("Worker").var("requests"))
            .expect("Worker_requests binding should exist");

        // With QueueA_requests=300 and QueueB_requests=200, summed result must be 500
        let params = params_from(&[("QueueA_requests", 300.0), ("QueueB_requests", 200.0)]);
        let result = yevice_core::evaluate::evaluate(&worker_binding.expr, &params).unwrap();
        assert_eq!(
            result, 500.0,
            "fan-in bindings must sum, not overwrite — got {result}"
        );
    }
}
