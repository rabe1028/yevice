//! Cloud resource shell — type-erased provider-agnostic container.
//!
//! `ResourceShell` replaces the old `ResourceSpec` enum. It stores the
//! strongly-typed spec as a `serde_json::Value` so that the core crate
//! stays independent of any specific service implementation.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::{LogicalId, Region, ResourceType};

/// Cloud provider for a resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Provider {
    Aws,
    Gcp,
    Cloudflare,
    Other,
}

impl Provider {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Aws => "aws",
            Self::Gcp => "gcp",
            Self::Cloudflare => "cloudflare",
            Self::Other => "other",
        }
    }
}

/// Type-erased resource container.
///
/// Stores the service identifier and the service-specific spec as a JSON value.
/// Service plugins decode the spec with [`ResourceShell::decode`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceShell {
    /// Unique service identifier, e.g. `"aws.lambda"`, `"gcp.cloud_run"`, `"other"`.
    pub service_id: String,
    /// Cloud provider.
    pub provider: Provider,
    /// Service-specific spec, stored as a JSON value.
    spec_data: Value,
    /// Arbitrary metadata (billing_mode, workflow_type, etc.) for use by bindings logic.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
}

impl ResourceShell {
    /// Create a new shell by serializing a typed spec.
    ///
    /// Panics if `spec` cannot be serialized to JSON (which should never happen
    /// for serde-annotated types).
    pub fn new<T: Serialize>(service_id: impl Into<String>, provider: Provider, spec: &T) -> Self {
        Self {
            service_id: service_id.into(),
            provider,
            spec_data: serde_json::to_value(spec).expect("spec serialization failed"),
            metadata: HashMap::new(),
        }
    }

    /// Create a shell for an unsupported / no-cost resource.
    pub fn other(original_type: impl Into<String>) -> Self {
        Self {
            service_id: "other".to_string(),
            provider: Provider::Other,
            spec_data: Value::Null,
            metadata: [("original_type".to_string(), original_type.into())]
                .into_iter()
                .collect(),
        }
    }

    /// Attach additional metadata to the shell.
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Decode the stored spec into a typed value.
    ///
    /// Returns an error if the stored JSON cannot be deserialized into `T`.
    pub fn decode<T: for<'de> Deserialize<'de>>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_value(self.spec_data.clone())
    }
}

/// A single cloud resource.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resource {
    pub logical_id: LogicalId,
    pub resource_type: ResourceType,
    pub shell: ResourceShell,
}

impl Resource {
    pub fn provider(&self) -> Provider {
        self.shell.provider
    }
}

// ---- Connection (graph edges) ----

/// A connection between two resources in the architecture.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Connection {
    pub source: LogicalId,
    pub target: LogicalId,
    pub connection_type: ConnectionType,
    pub batch_size: Option<f64>,
    pub parallelization_factor: Option<f64>,
    /// Multiplier: 1 invocation produces N downstream calls.
    pub factor: Option<f64>,
    /// Hint for the source resource type when the source is external (e.g., "sqs", "kinesis", "dynamodb").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConnectionType {
    /// Event source mapping (SQS/Kinesis/DDB Stream → Lambda).
    EventSource,
    /// Direct invocation (Lambda → StepFunctions, Lambda → Lambda).
    Invocation,
    /// Data flow (Lambda → DynamoDB, Lambda → S3).
    DataFlow,
    /// Notification (S3 → SQS, S3 → Lambda).
    Notification,
}

// ---- Architecture (top-level) ----

/// A complete architecture: typed resources + connection graph.
///
/// This is the provider-agnostic intermediate representation between
/// IaC parsers and output generators.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Architecture {
    pub name: String,
    pub region: Region,
    pub resources: Vec<Resource>,
    pub connections: Vec<Connection>,
}

impl Architecture {
    pub fn find_resource(&self, id: &LogicalId) -> Option<&Resource> {
        self.resources.iter().find(|r| &r.logical_id == id)
    }

    /// Returns true if any resource belongs to the given provider.
    pub fn has_provider(&self, provider: Provider) -> bool {
        self.resources
            .iter()
            .any(|resource| resource.provider() == provider)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_shell(service_id: &str, provider: Provider) -> ResourceShell {
        ResourceShell::new(service_id, provider, &serde_json::json!({}))
    }

    fn resource(logical_id: &str, resource_type: &str, shell: ResourceShell) -> Resource {
        Resource {
            logical_id: LogicalId::new(logical_id),
            resource_type: ResourceType::new(resource_type),
            shell,
        }
    }

    #[test]
    fn provider_classifies_each_provider_variant() {
        assert_eq!(Provider::Aws.as_str(), "aws");
        assert_eq!(Provider::Gcp.as_str(), "gcp");
        assert_eq!(Provider::Cloudflare.as_str(), "cloudflare");
        assert_eq!(Provider::Other.as_str(), "other");
    }

    #[test]
    fn resource_and_architecture_use_provider_classification() {
        let aws_resource = resource(
            "AppFunction",
            "AWS::Lambda::Function",
            make_shell("aws.lambda", Provider::Aws),
        );
        let gcp_resource = resource(
            "RunService",
            "google_cloud_run_v2_service",
            make_shell("gcp.cloud_run", Provider::Gcp),
        );
        let cloudflare_resource = resource(
            "EdgeWorker",
            "cloudflare_worker_script",
            make_shell("cloudflare.worker", Provider::Cloudflare),
        );

        assert_eq!(aws_resource.provider(), Provider::Aws);
        assert_eq!(gcp_resource.provider(), Provider::Gcp);
        assert_eq!(cloudflare_resource.provider(), Provider::Cloudflare);

        let architecture = Architecture {
            name: "mixed".to_string(),
            region: Region::new("ap-northeast-1"),
            resources: vec![aws_resource, gcp_resource, cloudflare_resource],
            connections: Vec::new(),
        };

        assert!(architecture.has_provider(Provider::Aws));
        assert!(architecture.has_provider(Provider::Gcp));
        assert!(architecture.has_provider(Provider::Cloudflare));
        assert!(!architecture.has_provider(Provider::Other));
    }

    #[test]
    fn shell_encode_decode_roundtrip() {
        #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        struct MySpec {
            memory_mb: f64,
            timeout_sec: f64,
        }

        let spec = MySpec {
            memory_mb: 256.0,
            timeout_sec: 30.0,
        };
        let shell = ResourceShell::new("aws.lambda", Provider::Aws, &spec);
        let decoded: MySpec = shell.decode().expect("decode failed");
        assert_eq!(decoded, spec);
    }

    #[test]
    fn shell_other_has_original_type_metadata() {
        let shell = ResourceShell::other("Custom::MyResource");
        assert_eq!(shell.service_id, "other");
        assert_eq!(shell.provider, Provider::Other);
        assert_eq!(
            shell.metadata.get("original_type").map(String::as_str),
            Some("Custom::MyResource")
        );
    }

    #[test]
    fn shell_with_metadata() {
        let shell = ResourceShell::new("aws.dynamodb", Provider::Aws, &serde_json::json!({}))
            .with_metadata("billing_mode", "on_demand");
        assert_eq!(
            shell.metadata.get("billing_mode").map(String::as_str),
            Some("on_demand")
        );
    }
}
