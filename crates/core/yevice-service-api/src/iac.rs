//! IaC adapter traits for CloudFormation and Terraform.
//!
//! Each IaC format has its own `*Adapter` trait that converts raw resource
//! data into a `ResourceShell`. Adapters are collected in `*AdapterRegistry`
//! and looked up at parse time by resource type string.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
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
// Shared property value type (all IaC formats)
// ---------------------------------------------------------------------------

/// A property value shared across all IaC formats (CFN, TF, Wrangler).
///
/// `Concrete` holds a statically-resolved JSON value.  The typed reference
/// variants (`ResourceRef`, `ResourceAttr`, `Interpolated`) carry unresolved
/// cross-resource references so that adapters can inspect or ignore them as
/// appropriate.
///
/// # Non-exhaustiveness
///
/// This enum is marked `#[non_exhaustive]` so that adding new variants in a
/// future release is not a breaking change for downstream match arms that
/// include a wildcard (`_`) arm.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum IacPropertyValue {
    /// A statically-resolved concrete JSON value.
    Concrete(serde_json::Value),
    /// An unresolved `!Ref LogicalId` / `resource_type.name` referencing
    /// another resource in the same template/config.
    ResourceRef { logical_id: String },
    /// An unresolved `!GetAtt LogicalId.Attr` /
    /// `resource_type.name.attr` resource attribute reference.
    ResourceAttr { logical_id: String, attr: String },
    /// An interpolated string composed of literal text and typed references
    /// (e.g., from `Fn::Sub` or TF template strings).
    ///
    /// `rendered` is the CFN-native `${...}` form of the interpolated string,
    /// pre-computed from `parts` at construction time so that `get_str()` can
    /// return a `&str` without allocating.
    Interpolated {
        parts: Vec<IacStringPart>,
        /// Pre-rendered CFN-native form (e.g. `"hello-${MyBucket}-suffix"`).
        /// Skipped during serialization/deserialization; rebuild with
        /// `render_iac_parts(&parts)` if needed after deserialization.
        #[serde(skip)]
        rendered: String,
    },
}

impl IacPropertyValue {
    /// Construct an `Interpolated` variant from a list of parts.
    ///
    /// The `rendered` field is computed automatically via `render_iac_parts`
    /// so callers do not need to manage it manually.
    pub fn new_interpolated(parts: Vec<IacStringPart>) -> Self {
        let rendered = render_iac_parts(&parts);
        Self::Interpolated { parts, rendered }
    }
}

/// One segment of an interpolated string property value.
///
/// Matches the variant structure of `IacPropertyValue` but at the
/// per-segment level inside an `Interpolated` value.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum IacStringPart {
    /// A plain literal string segment.
    Literal(String),
    /// A `!Ref`-style / resource-name reference segment.
    Ref(String),
    /// A `!GetAtt`-style / resource attribute reference segment.
    Attr { logical_id: String, attr: String },
}

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

/// Render a slice of `IacStringPart` into a CFN-native `${...}` interpolated
/// string.  Literal segments are emitted as-is; `Ref` segments become
/// `${LogicalId}`; `Attr` segments become `${LogicalId.Attr}`.
fn render_iac_parts(parts: &[IacStringPart]) -> String {
    let mut result = String::new();
    for part in parts {
        match part {
            IacStringPart::Literal(s) => result.push_str(s),
            IacStringPart::Ref(id) => {
                result.push_str("${");
                result.push_str(id);
                result.push('}');
            }
            IacStringPart::Attr { logical_id, attr } => {
                result.push_str("${");
                result.push_str(logical_id);
                result.push('.');
                result.push_str(attr);
                result.push('}');
            }
        }
    }
    result
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
    /// The `Properties` block of the CFn resource, keyed by property name.
    pub properties: BTreeMap<String, IacPropertyValue>,
}

impl RawCfnResource {
    /// Construct a `RawCfnResource` from a concrete `serde_json::Value` properties block.
    ///
    /// All top-level property values are wrapped as `IacPropertyValue::Concrete`.
    /// This constructor is intended for use in tests and adapters that work with
    /// already-resolved concrete JSON values (no sentinel parsing is performed).
    pub fn new(
        logical_id: impl Into<String>,
        resource_type: impl Into<String>,
        properties: Value,
    ) -> Self {
        let props = if let Value::Object(map) = properties {
            map.into_iter()
                .map(|(k, v)| (k, IacPropertyValue::Concrete(v)))
                .collect()
        } else {
            BTreeMap::new()
        };
        Self {
            logical_id: LogicalId::new(logical_id),
            resource_type: ResourceType::new(resource_type),
            properties: props,
        }
    }

    /// Get a string property value.
    ///
    /// For `Interpolated` values the pre-rendered CFN-native form (e.g.
    /// `"hello-${MyBucket}-suffix"`) is returned so that adapters that
    /// previously received a concrete `Fn::Sub` / `Fn::Join` string continue
    /// to work correctly after the typed `Interpolated` migration.
    pub fn get_str(&self, key: &str) -> Option<&str> {
        match self.properties.get(key)? {
            IacPropertyValue::Concrete(Value::String(s)) => Some(s.as_str()),
            IacPropertyValue::Concrete(_) => None,
            IacPropertyValue::ResourceRef { logical_id } => {
                tracing::warn!(key, ref_id = %logical_id, "unresolved !Ref where str expected; skipping");
                None
            }
            IacPropertyValue::ResourceAttr { logical_id, attr } => {
                tracing::warn!(key, %logical_id, %attr, "unresolved !GetAtt where str expected; skipping");
                None
            }
            IacPropertyValue::Interpolated { rendered, .. } => Some(rendered.as_str()),
        }
    }

    /// Get a numeric property value as f64.
    pub fn get_f64(&self, key: &str) -> Option<f64> {
        match self.properties.get(key)? {
            IacPropertyValue::Concrete(Value::Number(n)) => n.as_f64(),
            IacPropertyValue::Concrete(Value::String(s)) => s.parse().ok(),
            IacPropertyValue::Concrete(_) => None,
            IacPropertyValue::ResourceRef { logical_id } => {
                tracing::warn!(key, ref_id = %logical_id, "unresolved !Ref where f64 expected; skipping");
                None
            }
            IacPropertyValue::ResourceAttr { logical_id, attr } => {
                tracing::warn!(key, %logical_id, %attr, "unresolved !GetAtt where f64 expected; skipping");
                None
            }
            IacPropertyValue::Interpolated { .. } => None,
        }
    }

    /// Get a boolean property value.
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        match self.properties.get(key)? {
            IacPropertyValue::Concrete(Value::Bool(b)) => Some(*b),
            IacPropertyValue::Concrete(Value::String(s)) => match s.to_lowercase().as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            },
            IacPropertyValue::Concrete(_) => None,
            IacPropertyValue::ResourceRef { logical_id } => {
                tracing::warn!(key, ref_id = %logical_id, "unresolved !Ref where bool expected; skipping");
                None
            }
            IacPropertyValue::ResourceAttr { logical_id, attr } => {
                tracing::warn!(key, %logical_id, %attr, "unresolved !GetAtt where bool expected; skipping");
                None
            }
            IacPropertyValue::Interpolated { .. } => None,
        }
    }

    /// Returns the underlying JSON value for any `Concrete` variant.
    /// Returns `None` for unresolved intrinsic references.
    pub fn get_object(&self, key: &str) -> Option<&serde_json::Value> {
        match self.properties.get(key)? {
            IacPropertyValue::Concrete(v) => Some(v),
            IacPropertyValue::ResourceRef { logical_id } => {
                tracing::warn!(key, ref_id = %logical_id, "unresolved !Ref where object expected; skipping");
                None
            }
            IacPropertyValue::ResourceAttr { logical_id, attr } => {
                tracing::warn!(key, %logical_id, %attr, "unresolved !GetAtt where object expected; skipping");
                None
            }
            IacPropertyValue::Interpolated { .. } => None,
        }
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

    /// Returns a sorted list of all registered CloudFormation resource type strings.
    pub fn registered_types(&self) -> Vec<&str> {
        let mut types: Vec<&str> = self.adapters.keys().map(String::as_str).collect();
        types.sort_unstable();
        types
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
/// Attribute values are stored as `IacPropertyValue` so adapters can use
/// `get_str`, `get_f64`, and `get_bool` methods without depending on the
/// internal `TfValue` type.  Concrete scalars are wrapped in
/// `IacPropertyValue::Concrete`; cross-resource references become
/// `IacPropertyValue::ResourceRef` or `IacPropertyValue::ResourceAttr`.
#[derive(Debug, Clone)]
pub struct RawTfResource {
    pub logical_id: LogicalId,
    pub resource_type: ResourceType,
    /// Flat attribute map with resolved values.
    pub attrs: HashMap<String, IacPropertyValue>,
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
        match self.attrs.get(key)? {
            IacPropertyValue::Concrete(Value::String(s)) => Some(s.as_str()),
            IacPropertyValue::Concrete(_) => None,
            IacPropertyValue::ResourceRef { .. }
            | IacPropertyValue::ResourceAttr { .. }
            | IacPropertyValue::Interpolated { .. } => None,
        }
    }

    /// Get a numeric attribute value as f64.
    pub fn get_f64(&self, key: &str) -> Option<f64> {
        match self.attrs.get(key)? {
            IacPropertyValue::Concrete(Value::Number(n)) => n.as_f64(),
            IacPropertyValue::Concrete(Value::String(s)) => s.parse::<f64>().ok(),
            IacPropertyValue::Concrete(_) => None,
            IacPropertyValue::ResourceRef { .. }
            | IacPropertyValue::ResourceAttr { .. }
            | IacPropertyValue::Interpolated { .. } => None,
        }
    }

    /// Get a boolean attribute value.
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        match self.attrs.get(key)? {
            IacPropertyValue::Concrete(Value::Bool(b)) => Some(*b),
            IacPropertyValue::Concrete(Value::String(s)) => match s.to_lowercase().as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            },
            IacPropertyValue::Concrete(_) => None,
            IacPropertyValue::ResourceRef { .. }
            | IacPropertyValue::ResourceAttr { .. }
            | IacPropertyValue::Interpolated { .. } => None,
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

    /// Returns a sorted list of all registered Terraform resource type strings.
    pub fn registered_types(&self) -> Vec<&str> {
        let mut types: Vec<&str> = self.adapters.keys().map(String::as_str).collect();
        types.sort_unstable();
        types
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

    // -----------------------------------------------------------------------
    // IacPropertyValue::Interpolated regression test (PR #44 / issue #39)
    // -----------------------------------------------------------------------

    /// `get_str()` must return the CFN-native rendered form for `Interpolated`
    /// values so that adapters that previously received a concrete
    /// `Fn::Sub`/`Fn::Join` string continue to work after the typed migration.
    #[test]
    fn get_str_interpolated_renders_parts() {
        let parts = vec![
            IacStringPart::Literal("hello-".to_string()),
            IacStringPart::Ref("MyBucket".to_string()),
            IacStringPart::Literal("-suffix".to_string()),
        ];
        let mut raw =
            RawCfnResource::new("MyResource", "AWS::Test::Resource", serde_json::json!({}));
        raw.properties.insert(
            "Name".to_string(),
            IacPropertyValue::new_interpolated(parts),
        );

        assert_eq!(raw.get_str("Name"), Some("hello-${MyBucket}-suffix"));
    }

    /// `get_str()` also handles `Attr` parts correctly.
    #[test]
    fn get_str_interpolated_with_attr_part() {
        let parts = vec![
            IacStringPart::Literal("arn:prefix:".to_string()),
            IacStringPart::Attr {
                logical_id: "MyTable".to_string(),
                attr: "Arn".to_string(),
            },
        ];
        let mut raw =
            RawCfnResource::new("MyResource", "AWS::Test::Resource", serde_json::json!({}));
        raw.properties
            .insert("Arn".to_string(), IacPropertyValue::new_interpolated(parts));

        assert_eq!(raw.get_str("Arn"), Some("arn:prefix:${MyTable.Arn}"));
    }

    /// `render_iac_parts` helper produces the correct CFN `${...}` string.
    #[test]
    fn render_iac_parts_all_variants() {
        let parts = vec![
            IacStringPart::Literal("prefix-".to_string()),
            IacStringPart::Ref("ResA".to_string()),
            IacStringPart::Literal(":".to_string()),
            IacStringPart::Attr {
                logical_id: "ResB".to_string(),
                attr: "Name".to_string(),
            },
        ];
        assert_eq!(render_iac_parts(&parts), "prefix-${ResA}:${ResB.Name}");
    }
}
