//! Capacity planning and quota validation types.

use serde::{Deserialize, Serialize};

use crate::expr::Expr;
use crate::types::LogicalId;

/// Severity of a capacity constraint violation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Error => write!(f, "ERROR"),
            Self::Warning => write!(f, "WARNING"),
            Self::Info => write!(f, "INFO"),
        }
    }
}

/// AWS quota type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuotaType {
    /// Cannot be increased.
    Hard,
    /// Can be increased via support request.
    Soft,
}

/// A capacity constraint on a resource.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Constraint {
    /// Dimension being constrained (e.g., "concurrent_executions").
    pub dimension: String,
    /// Expression that computes the required capacity from usage params.
    pub required: Expr,
    /// The limit value (from quota or provisioned setting).
    pub limit: f64,
    /// Whether this is a hard or soft limit.
    pub quota_type: QuotaType,
    /// Severity if violated.
    pub severity: Severity,
    /// Message template with {required} and {limit} placeholders.
    pub message_template: String,
}

/// Capacity model for a resource.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapacityModel {
    pub logical_id: LogicalId,
    pub label: String,
    pub constraints: Vec<Constraint>,
}

/// A detected violation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Violation {
    pub severity: Severity,
    pub resource: LogicalId,
    pub dimension: String,
    pub required: f64,
    pub limit: f64,
    pub quota_type: QuotaType,
    pub message: String,
}

/// Result of capacity validation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    pub violations: Vec<Violation>,
}

impl ValidationResult {
    pub fn has_errors(&self) -> bool {
        self.violations
            .iter()
            .any(|v| v.severity == Severity::Error)
    }

    pub fn has_warnings(&self) -> bool {
        self.violations
            .iter()
            .any(|v| v.severity == Severity::Warning)
    }
}

/// Evaluate capacity constraints against usage parameters.
pub fn validate_capacity(
    models: &[CapacityModel],
    params: &crate::evaluate::Params,
) -> ValidationResult {
    let mut violations = Vec::new();

    for model in models {
        for constraint in &model.constraints {
            let required = match crate::evaluate::evaluate(&constraint.required, params) {
                Ok(v) => v,
                Err(_) => continue, // Variable not provided, skip
            };

            if required > constraint.limit {
                let message = constraint
                    .message_template
                    .replace("{required}", &format!("{required:.0}"))
                    .replace("{limit}", &format!("{:.0}", constraint.limit));

                violations.push(Violation {
                    severity: constraint.severity,
                    resource: model.logical_id.clone(),
                    dimension: constraint.dimension.clone(),
                    required,
                    limit: constraint.limit,
                    quota_type: constraint.quota_type,
                    message,
                });
            }
        }
    }

    // Sort: Error first, then Warning, then Info
    violations.sort_by_key(|v| match v.severity {
        Severity::Error => 0,
        Severity::Warning => 1,
        Severity::Info => 2,
    });

    ValidationResult { violations }
}

// ---- Default AWS Quotas ----

/// Default quota values for ap-northeast-1.
/// Users can override via quotas.yaml.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegionQuotas {
    pub lambda_concurrent_executions: f64,
    pub dynamodb_max_wcu_per_table: f64,
    pub dynamodb_max_rcu_per_table: f64,
    pub dynamodb_max_tables: f64,
    pub kinesis_max_shards_per_stream: f64,
    pub kinesis_max_records_per_sec_per_shard: f64,
    pub kinesis_max_mb_per_sec_per_shard: f64,
}

impl Default for RegionQuotas {
    fn default() -> Self {
        Self {
            lambda_concurrent_executions: 1000.0,
            dynamodb_max_wcu_per_table: 40_000.0,
            dynamodb_max_rcu_per_table: 40_000.0,
            dynamodb_max_tables: 2500.0,
            kinesis_max_shards_per_stream: 200.0,
            kinesis_max_records_per_sec_per_shard: 1000.0,
            kinesis_max_mb_per_sec_per_shard: 1.0,
        }
    }
}
