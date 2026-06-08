//! Domain value objects for type-safe identifiers.
//!
//! These newtypes prevent mixing up different kinds of string identifiers
//! at compile time (e.g., passing a `ResourceType` where a `LogicalId` is expected).

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
