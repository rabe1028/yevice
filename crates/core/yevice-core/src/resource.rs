//! Cloud resource shell — type-erased provider-agnostic container.
//!
//! `ResourceShell` replaces the old `ResourceSpec` enum. It stores the
//! strongly-typed spec as a `serde_json::Value` so that the core crate
//! stays independent of any specific service implementation.

use std::collections::{HashMap, HashSet};

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
    /// # Panics
    ///
    /// Panics if `spec` cannot be serialized to a JSON value.  This should
    /// never happen for types that derive [`serde::Serialize`] without custom
    /// error paths.  For code that needs to handle serialization failures
    /// gracefully, prefer [`ResourceShell::try_new`].
    pub fn new<T: Serialize>(service_id: impl Into<String>, provider: Provider, spec: &T) -> Self {
        Self::try_new(service_id, provider, spec).expect("spec serialization failed")
    }

    /// Fallible variant of [`ResourceShell::new`].
    ///
    /// Returns `Err` if `spec` cannot be serialized to a JSON value instead of
    /// panicking.  Prefer this constructor in new code where the caller can
    /// propagate errors.
    ///
    /// `new` is the panic version intended for contexts where `spec` is
    /// guaranteed to be serde-serialisable (e.g. a statically-known type).
    /// `try_new` is the fallible version recommended for new code.
    pub fn try_new<T: Serialize>(
        service_id: impl Into<String>,
        provider: Provider,
        spec: &T,
    ) -> Result<Self, serde_json::Error> {
        Ok(Self {
            service_id: service_id.into(),
            provider,
            spec_data: serde_json::to_value(spec)?,
            metadata: HashMap::new(),
        })
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
    /// Containment parent (VPC / subnet / cluster), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<LogicalId>,
}

impl Resource {
    pub fn provider(&self) -> Provider {
        self.shell.provider
    }
}

// ---- Connection (graph edges) ----

/// A connection between two resources in the architecture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

// ---- Connection deduplication helper ----

/// Stringified key used to dedupe connections: `(source, target, type)`.
///
/// `ConnectionType` is rendered via the same `&'static str` mapping used by
/// both CFn and Terraform converters, so identical edges from different paths
/// collide on the same key.
pub type EdgeKey = (String, String, String);

/// Render a [`ConnectionType`] as a stable string for use in [`EdgeKey`].
pub fn connection_type_str(conn_type: &ConnectionType) -> &'static str {
    match conn_type {
        ConnectionType::EventSource => "EventSource",
        ConnectionType::Invocation => "Invocation",
        ConnectionType::DataFlow => "DataFlow",
        ConnectionType::Notification => "Notification",
    }
}

/// Accumulator that deduplicates [`Connection`] edges by
/// `(source, target, connection_type)` while applying optional endpoint
/// existence guards.
///
/// Used by IaC converters (CFn, Terraform) to avoid double-counting edges
/// that can be produced from multiple template paths (e.g. both an
/// `AWS::Lambda::EventSourceMapping` and a SAM `Events` block).
///
/// # Example
///
/// ```ignore
/// let mut dedupe = ConnectionDeduper::new();
/// dedupe.try_push(conn, |id| nodes.contains(id), |_| true);
/// let (connections, _seen) = dedupe.into_parts();
/// ```
#[derive(Debug, Default)]
pub struct ConnectionDeduper {
    connections: Vec<Connection>,
    seen: HashSet<EdgeKey>,
}

impl ConnectionDeduper {
    /// Create an empty deduper.
    pub fn new() -> Self {
        Self::default()
    }

    /// Attempt to push `conn`.
    ///
    /// `source_exists` and `target_exists` are endpoint guards: when either
    /// returns `false`, the connection is dropped silently. This lets each
    /// caller decide whether the source endpoint must be a known node (CFn
    /// ESM allows external ARNs; structured-property edges and Terraform
    /// require both endpoints to exist).
    ///
    /// Returns `true` if the connection was newly inserted, `false` if it was
    /// a duplicate or failed the endpoint guard.
    pub fn try_push<S, T>(&mut self, conn: Connection, source_exists: S, target_exists: T) -> bool
    where
        S: FnOnce(&str) -> bool,
        T: FnOnce(&str) -> bool,
    {
        if !source_exists(conn.source.as_str()) {
            return false;
        }
        if !target_exists(conn.target.as_str()) {
            return false;
        }
        let key = (
            conn.source.as_str().to_string(),
            conn.target.as_str().to_string(),
            connection_type_str(&conn.connection_type).to_string(),
        );
        if self.seen.insert(key) {
            self.connections.push(conn);
            true
        } else {
            false
        }
    }

    /// Number of unique connections accumulated so far.
    pub fn len(&self) -> usize {
        self.connections.len()
    }

    /// Whether any connection has been accumulated.
    pub fn is_empty(&self) -> bool {
        self.connections.is_empty()
    }

    /// Borrow the accumulated connections without consuming the deduper.
    pub fn connections(&self) -> &[Connection] {
        &self.connections
    }

    /// Consume the deduper and return the accumulated connections.
    pub fn into_connections(self) -> Vec<Connection> {
        self.connections
    }
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

    /// Build the output-agnostic topology view: every resource as a node
    /// plus all connections. Includes non-costed / `other` resources.
    pub fn topology(&self) -> crate::topology::Topology {
        crate::topology::Topology {
            nodes: self
                .resources
                .iter()
                .map(|r| crate::topology::TopologyNode {
                    logical_id: r.logical_id.clone(),
                    resource_type: r.resource_type.clone(),
                    provider: r.shell.provider,
                    service_id: r.shell.service_id.clone(),
                    label: None,
                    group: r.group.clone(),
                })
                .collect(),
            connections: self.connections.clone(),
        }
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
            group: None,
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

    #[test]
    fn try_new_returns_ok_and_matches_new() {
        #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        struct MySpec {
            vcpu: f64,
            memory_mb: f64,
        }

        let spec = MySpec {
            vcpu: 2.0,
            memory_mb: 512.0,
        };

        let via_try_new =
            ResourceShell::try_new("aws.ec2", Provider::Aws, &spec).expect("try_new failed");
        let via_new = ResourceShell::new("aws.ec2", Provider::Aws, &spec);

        // Both shells should expose the same service_id, provider, and decoded spec.
        assert_eq!(via_try_new.service_id, via_new.service_id);
        assert_eq!(via_try_new.provider, via_new.provider);

        let decoded_try: MySpec = via_try_new.decode().expect("decode try_new");
        let decoded_new: MySpec = via_new.decode().expect("decode new");
        assert_eq!(decoded_try, decoded_new);
    }

    fn conn(source: &str, target: &str, ty: ConnectionType) -> Connection {
        Connection {
            source: LogicalId::new(source),
            target: LogicalId::new(target),
            connection_type: ty,
            batch_size: None,
            parallelization_factor: None,
            factor: None,
            source_hint: None,
        }
    }

    #[test]
    fn dedupe_collapses_identical_triples() {
        let mut d = ConnectionDeduper::new();
        assert!(d.try_push(conn("A", "B", ConnectionType::DataFlow), |_| true, |_| true));
        // Same (source, target, type) — must collapse.
        assert!(!d.try_push(conn("A", "B", ConnectionType::DataFlow), |_| true, |_| true));
        // Different type — kept.
        assert!(d.try_push(
            conn("A", "B", ConnectionType::Invocation),
            |_| true,
            |_| true
        ));
        let edges = d.into_connections();
        assert_eq!(edges.len(), 2);
        assert_eq!(edges[0].connection_type, ConnectionType::DataFlow);
        assert_eq!(edges[1].connection_type, ConnectionType::Invocation);
    }

    #[test]
    fn dedupe_drops_unknown_source_when_guarded() {
        let known: HashSet<String> = ["B"].iter().copied().map(String::from).collect();
        let mut d = ConnectionDeduper::new();
        // Source guard rejects — connection dropped.
        assert!(!d.try_push(
            conn("Missing", "B", ConnectionType::EventSource),
            |id| known.contains(id),
            |id| known.contains(id),
        ));
        // Source guard relaxed (ESM-style) — connection kept even though "Ext"
        // is not in the node set.
        assert!(d.try_push(
            conn("Ext", "B", ConnectionType::EventSource),
            |_| true,
            |id| known.contains(id),
        ));
        assert_eq!(d.len(), 1);
    }

    #[test]
    fn dedupe_drops_unknown_target() {
        let known: HashSet<String> = ["A"].iter().copied().map(String::from).collect();
        let mut d = ConnectionDeduper::new();
        assert!(!d.try_push(
            conn("A", "Missing", ConnectionType::DataFlow),
            |id| known.contains(id),
            |id| known.contains(id),
        ));
        assert!(d.is_empty());
    }

    #[test]
    fn connection_type_str_matches_debug_repr() {
        // Insurance against the historical CFn `format!("{:?}", ty)` key:
        // the dedupe string must match the Debug representation so converters
        // that previously used `{:?}` produce identical keys after migration.
        for ty in [
            ConnectionType::EventSource,
            ConnectionType::Invocation,
            ConnectionType::DataFlow,
            ConnectionType::Notification,
        ] {
            assert_eq!(connection_type_str(&ty), format!("{ty:?}"));
        }
    }
}
