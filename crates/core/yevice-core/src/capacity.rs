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

/// A constraint that was skipped because its required expression could not be
/// evaluated (e.g. a variable was not provided in the current params).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkippedConstraint {
    pub resource: LogicalId,
    pub dimension: String,
    pub reason: String,
}

/// Result of capacity validation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    pub violations: Vec<Violation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<SkippedConstraint>,
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
    let mut skipped = Vec::new();

    for model in models {
        for constraint in &model.constraints {
            // Skip when a required variable was not provided.
            let required = match crate::evaluate::evaluate(&constraint.required, params) {
                Ok(v) => v,
                Err(e) => {
                    skipped.push(SkippedConstraint {
                        resource: model.logical_id.clone(),
                        dimension: constraint.dimension.clone(),
                        reason: e.to_string(),
                    });
                    continue;
                }
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

    ValidationResult {
        violations,
        skipped,
    }
}

// ---- Provider-agnostic Quotas ----

/// Provider-agnostic service quotas, keyed by a namespaced string such as
/// `"aws.lambda.concurrent_executions"`. Quota keys are owned by the
/// provider crates that produce and consume them; core only stores them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Quotas(std::collections::HashMap<String, f64>);

impl Quotas {
    pub fn get(&self, key: &str) -> Option<f64> {
        self.0.get(key).copied()
    }

    pub fn insert(&mut self, key: impl Into<String>, value: f64) {
        self.0.insert(key.into(), value);
    }

    #[must_use]
    pub fn with(mut self, key: impl Into<String>, value: f64) -> Self {
        self.insert(key, value);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Iterate over the quota keys.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }

    /// Merge another set of quotas into this one; values in `other` win on key collision.
    pub fn merge_from(&mut self, other: Quotas) {
        self.0.extend(other.0);
    }
}

/// Supplies provider-specific default quotas for a region. Implemented by
/// service crates; core never hardcodes provider quota values.
pub trait QuotaProvider: Send + Sync {
    fn default_quotas(&self, region: &str) -> Quotas;
}
