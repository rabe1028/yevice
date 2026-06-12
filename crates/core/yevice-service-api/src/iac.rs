//! IaC adapter traits for CloudFormation and Terraform.
//!
//! Each IaC format has its own `*Adapter` trait that converts raw resource
//! data into a `ResourceShell`. Adapters are collected in `*AdapterRegistry`
//! and looked up at parse time by resource type string.

use std::{collections::HashMap, sync::Arc};

use serde_json::Value;
use yevice_core::{
    resource::ResourceShell,
    types::{LogicalId, ResourceType},
};

// ---------------------------------------------------------------------------
// Shared error type
// ---------------------------------------------------------------------------

/// Errors that can occur while converting raw IaC resources to `ResourceShell`.
#[derive(Debug, thiserror::Error)]
pub enum IacError {
    #[error("missing required property '{0}'")]
    MissingProperty(String),

    #[error("invalid value for '{field}': {cause}")]
    InvalidValue { field: String, cause: String },
}

// ---------------------------------------------------------------------------
// CloudFormation
// ---------------------------------------------------------------------------

/// A raw CloudFormation resource as parsed from YAML.
///
/// The `properties` field holds the deserialized `Properties` block.
#[derive(Debug, Clone)]
pub struct RawCfnResource {
    pub logical_id: LogicalId,
    pub resource_type: ResourceType,
    /// The `Properties` block of the CFn resource (may be `Value::Null`).
    pub properties: Value,
}

impl RawCfnResource {
    pub fn new(
        logical_id: impl Into<String>,
        resource_type: impl Into<String>,
        properties: Value,
    ) -> Self {
        Self {
            logical_id: LogicalId::new(logical_id),
            resource_type: ResourceType::new(resource_type),
            properties,
        }
    }

    /// Get a string property value.
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.properties.get(key)?.as_str()
    }

    /// Get a numeric property value as f64.
    pub fn get_f64(&self, key: &str) -> Option<f64> {
        match self.properties.get(key)? {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => {
                if s.starts_with("{{ref:") || s.starts_with("{{getatt:") {
                    tracing::warn!(
                        value = %s,
                        key = %key,
                        "unresolved CloudFormation reference {} where a number was expected; treating as absent",
                        s
                    );
                    return None;
                }
                s.parse::<f64>().ok()
            }
            _ => None,
        }
    }

    /// Get a boolean property value.
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        match self.properties.get(key)? {
            Value::Bool(b) => Some(*b),
            Value::String(s) => {
                if s.starts_with("{{ref:") || s.starts_with("{{getatt:") {
                    tracing::warn!(
                        value = %s,
                        key = %key,
                        "unresolved CloudFormation reference {} where a boolean was expected; treating as absent",
                        s
                    );
                    return None;
                }
                match s.to_lowercase().as_str() {
                    "true" => Some(true),
                    "false" => Some(false),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Access a nested object property.
    pub fn get_object(&self, key: &str) -> Option<&Value> {
        self.properties.get(key)
    }
}

/// Adapter for converting a single CloudFormation resource type to a `ResourceShell`.
pub trait CfnAdapter: Send + Sync {
    /// Returns the CloudFormation resource type strings this adapter handles
    /// (e.g., `&["AWS::Lambda::Function"]`).
    fn handles(&self) -> &[&'static str];

    /// Convert the raw resource into a `ResourceShell`.
    fn convert(&self, raw: &RawCfnResource) -> Result<ResourceShell, IacError>;
}

/// Registry of all `CfnAdapter` implementations, keyed by resource type.
#[derive(Default)]
pub struct CfnAdapterRegistry {
    adapters: HashMap<String, Arc<dyn CfnAdapter>>,
}

impl CfnAdapterRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an adapter. If the adapter handles multiple resource types,
    /// it is registered for each one (sharing the same `Arc`).
    ///
    /// # Panics
    ///
    /// Panics if any resource type handled by this adapter has already been registered.
    pub fn register(&mut self, adapter: impl CfnAdapter + 'static) {
        let arc: Arc<dyn CfnAdapter> = Arc::new(adapter);
        for rt in arc.handles() {
            let key = (*rt).to_string();
            assert!(
                !self.adapters.contains_key(&key),
                "duplicate CFN adapter registration for resource_type '{key}'"
            );
            self.adapters.insert(key, Arc::clone(&arc));
        }
    }

    /// Look up the adapter for a given resource type string.
    pub fn lookup(&self, resource_type: &str) -> Option<&dyn CfnAdapter> {
        self.adapters.get(resource_type).map(AsRef::as_ref)
    }

    /// Convert a raw CFn resource to a `ResourceShell`.
    ///
    /// Returns `None` if no adapter is registered for the resource type.
    pub fn convert(&self, raw: &RawCfnResource) -> Option<Result<ResourceShell, IacError>> {
        let adapter = self.lookup(raw.resource_type.as_str())?;
        Some(adapter.convert(raw))
    }
}

// ---------------------------------------------------------------------------
// Terraform
// ---------------------------------------------------------------------------

/// A resolved Terraform resource with concrete attribute values.
///
/// Attribute values are stored as `serde_json::Value` so adapters can use
/// `get_str`, `get_f64`, and `get_bool` methods without depending on the
/// internal `TfValue` type.
#[derive(Debug, Clone)]
pub struct RawTfResource {
    pub logical_id: LogicalId,
    pub resource_type: ResourceType,
    /// Flat attribute map with resolved (concrete) values.
    pub attrs: HashMap<String, Value>,
    /// Block attributes (e.g., `container_properties` blocks).
    pub blocks: HashMap<String, Vec<HashMap<String, Value>>>,
}

impl RawTfResource {
    pub fn new(logical_id: impl Into<String>, resource_type: impl Into<String>) -> Self {
        Self {
            logical_id: LogicalId::new(logical_id),
            resource_type: ResourceType::new(resource_type),
            attrs: HashMap::new(),
            blocks: HashMap::new(),
        }
    }

    /// Get a string attribute value.
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.attrs.get(key)?.as_str()
    }

    /// Get a numeric attribute value as f64.
    pub fn get_f64(&self, key: &str) -> Option<f64> {
        match self.attrs.get(key)? {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.parse::<f64>().ok(),
            _ => None,
        }
    }

    /// Get a boolean attribute value.
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        match self.attrs.get(key)? {
            Value::Bool(b) => Some(*b),
            Value::String(s) => match s.to_lowercase().as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            },
            _ => None,
        }
    }

    /// Access a block by name (returns the first block if present).
    pub fn get_block(&self, name: &str) -> Option<&HashMap<String, Value>> {
        self.blocks.get(name)?.first()
    }

    /// Access all blocks with the given name.
    pub fn get_blocks(&self, name: &str) -> &[HashMap<String, Value>] {
        self.blocks.get(name).map_or(&[], Vec::as_slice)
    }
}

/// Adapter for converting a single Terraform resource type to a `ResourceShell`.
pub trait TfAdapter: Send + Sync {
    /// Returns the Terraform resource type strings this adapter handles
    /// (e.g., `&["aws_lambda_function"]`).
    fn handles(&self) -> &[&'static str];

    /// Convert the raw resource into a `ResourceShell`.
    fn convert(&self, raw: &RawTfResource) -> Result<ResourceShell, IacError>;
}

/// Registry of all `TfAdapter` implementations, keyed by resource type.
#[derive(Default)]
pub struct TfAdapterRegistry {
    adapters: HashMap<String, Arc<dyn TfAdapter>>,
}

impl TfAdapterRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an adapter. If the adapter handles multiple resource types,
    /// it is registered for each one (sharing the same `Arc`).
    ///
    /// # Panics
    ///
    /// Panics if any resource type handled by this adapter has already been registered.
    pub fn register(&mut self, adapter: impl TfAdapter + 'static) {
        let arc: Arc<dyn TfAdapter> = Arc::new(adapter);
        for rt in arc.handles() {
            let key = (*rt).to_string();
            assert!(
                !self.adapters.contains_key(&key),
                "duplicate TF adapter registration for resource_type '{key}'"
            );
            self.adapters.insert(key, Arc::clone(&arc));
        }
    }

    /// Look up the adapter for a given resource type string.
    pub fn lookup(&self, resource_type: &str) -> Option<&dyn TfAdapter> {
        self.adapters.get(resource_type).map(AsRef::as_ref)
    }

    /// Convert a raw TF resource to a `ResourceShell`.
    ///
    /// Returns `None` if no adapter is registered for the resource type.
    pub fn convert(&self, raw: &RawTfResource) -> Option<Result<ResourceShell, IacError>> {
        let adapter = self.lookup(raw.resource_type.as_str())?;
        Some(adapter.convert(raw))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyCfnAdapter;

    impl CfnAdapter for DummyCfnAdapter {
        fn handles(&self) -> &[&'static str] {
            &["AWS::Test::Resource"]
        }

        fn convert(&self, _raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
            unimplemented!()
        }
    }

    struct DummyTfAdapter;

    impl TfAdapter for DummyTfAdapter {
        fn handles(&self) -> &[&'static str] {
            &["test_resource"]
        }

        fn convert(&self, _raw: &RawTfResource) -> Result<ResourceShell, IacError> {
            unimplemented!()
        }
    }

    #[test]
    #[should_panic(expected = "duplicate")]
    fn duplicate_cfn_adapter_registration_panics() {
        let mut registry = CfnAdapterRegistry::new();
        registry.register(DummyCfnAdapter);
        registry.register(DummyCfnAdapter);
    }

    #[test]
    #[should_panic(expected = "duplicate")]
    fn duplicate_tf_adapter_registration_panics() {
        let mut registry = TfAdapterRegistry::new();
        registry.register(DummyTfAdapter);
        registry.register(DummyTfAdapter);
    }
}
