//! Domain value objects for type-safe identifiers.
//!
//! These newtypes prevent mixing up different kinds of string identifiers
//! at compile time (e.g., passing a `ResourceType` where a `LogicalId` is expected).

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_string())
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl PartialEq<str> for $name {
            fn eq(&self, other: &str) -> bool {
                self.0 == other
            }
        }

        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                self.0 == *other
            }
        }
    };
}

define_id!(
    /// CloudFormation logical resource ID (e.g., "MyFunction", "DataTable").
    LogicalId
);

impl LogicalId {
    pub fn var(&self, suffix: &str) -> VariableName {
        VariableName::new(format!("{}_{}", self.as_str(), suffix))
    }
}

define_id!(
    /// AWS resource type (e.g., "AWS::Lambda::Function").
    ResourceType
);

define_id!(
    /// AWS region code (e.g., "ap-northeast-1").
    Region
);

define_id!(
    /// Variable name in a cost expression (e.g., "IngestFunction_requests").
    VariableName
);

define_id!(
    /// CloudFormation parameter name (e.g., "Stage").
    ParameterName
);

define_id!(
    /// CloudFormation condition name (e.g., "IsProd").
    ConditionName
);

define_id!(
    /// CloudFormation stack export name for Fn::ImportValue.
    ExportName
);

define_id!(
    /// Architecture name for cost comparison.
    ArchitectureName
);

/// Canonical variable-name suffixes for `LogicalId::var(...)`.
///
/// Services derive cost-expression variable names from a logical id plus a
/// suffix (e.g. `IngestFunction.var("requests")` → `IngestFunction_requests`).
/// The suffix is a plain string, so a typo (`"requets"`) or a service-to-service
/// drift (`"requests"` vs `"monthly_requests"`) silently produces a distinct
/// variable that no test, no schema, and no quota would link back.
///
/// Defining the most common suffixes as `&'static str` constants makes typos
/// a compile error and surfaces the canonical name in IDE auto-completion.
///
/// **Naming policy** — when more than one phrasing exists in the codebase,
/// the canonical name is the shortest unambiguous form (`requests` over
/// `monthly_requests`; `storage_gb` over `storage_size_gb`). Service-specific
/// distinctions (`disk_size_gb` for an EBS root volume vs `storage_gb` for a
/// database) are preserved — only synonyms are unified.
///
/// Adding a constant here is purely additive; existing string literals keep
/// working until they are migrated row-by-row.
pub mod var {
    /// Request count over the billing period (most-common ~5 services).
    pub const REQUESTS: &str = "requests";

    /// Lambda invocation count (kept distinct from `requests`: invocations
    /// include async / event-source triggers that are not HTTP requests).
    pub const INVOCATIONS: &str = "invocations";

    /// Storage volume in GB-month (most-common ~10 services: RDS, EFS,
    /// CloudWatch Logs, OpenSearch, ElastiCache, etc.).
    pub const STORAGE_GB: &str = "storage_gb";

    /// Average per-request duration in milliseconds (used by Lambda; kept
    /// distinct from `avg_duration_sec` because the unit changes the
    /// downstream multiplier and mixing them silently doubles the cost).
    pub const AVG_DURATION_MS: &str = "avg_duration_ms";

    /// Average per-request duration in seconds.
    pub const AVG_DURATION_SEC: &str = "avg_duration_sec";

    /// Allocated memory in GB (Lambda function size, Fargate task memory).
    pub const MEMORY_GB: &str = "memory_gb";

    /// vCPU count (Fargate, Batch).
    pub const VCPU: &str = "vcpu";

    /// Instance / node count for capacity-priced services.
    pub const INSTANCE_COUNT: &str = "instance_count";

    /// Ingested data volume in GB (CloudWatch Logs, Firehose, GuardDuty).
    pub const INGESTION_GB: &str = "ingestion_gb";

    /// Backup storage volume in GB-month (RDS, FSx, AWS Backup).
    pub const BACKUP_GB: &str = "backup_gb";
}
