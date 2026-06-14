use std::collections::HashMap;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::bindings::{BindingsFile, to_variable_bindings};
use crate::capacity::Quotas;
use crate::cost::{ArchitectureCost, CostBuildError, VariableBinding};
use crate::evaluate::Params;
use crate::types::VariableName;

/// IaC input file maximum size in bytes (OOM prevention limit).
pub const MAX_IAC_FILE_BYTES: u64 = 16 * 1024 * 1024; // 16 MiB

/// Read a file as a UTF-8 string, returning an error if the file exceeds
/// [`MAX_IAC_FILE_BYTES`].
///
/// The size is checked via [`std::fs::metadata`] (which follows symlinks)
/// before reading. This is a best-effort OOM guard for local, trusted IaC
/// files: it does not defend against a file that grows between the metadata
/// check and the read (TOCTOU), nor against pseudo-files whose reported
/// length differs from the bytes produced. That is acceptable under the
/// threat model (developer-supplied config files), not adversarial FS races.
///
/// # Errors
///
/// Returns [`io::Error`] with [`ErrorKind::InvalidData`] when the file size
/// exceeds the limit, or any other [`io::Error`] that the underlying
/// [`std::fs`] operations may produce.
pub fn read_to_string_capped(path: &Path) -> io::Result<String> {
    let len = std::fs::metadata(path)?.len();
    if len > MAX_IAC_FILE_BYTES {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("file too large: {len} bytes exceeds limit {MAX_IAC_FILE_BYTES}"),
        ));
    }
    std::fs::read_to_string(path)
}

/// Unified error type returned by the shared IaC file-read helper.
///
/// Carries the offending path along with the underlying [`io::Error`] so that
/// downstream parser errors can render messages of the form
/// `"<path>: <io error>"` without each call site re-implementing the wrap.
///
/// Each IaC parser's error enum should add a `#[from] IoReadError` variant
/// (and keep its legacy `Io(std::io::Error)` variant intact) so [`read_iac_file`]
/// flows through the existing `?` chains.
#[derive(Debug, Error)]
#[error("failed to read {}: {source}", path.display())]
pub struct IoReadError {
    /// File that could not be read.
    pub path: PathBuf,
    /// Underlying I/O error (including the size-limit error from
    /// [`read_to_string_capped`]).
    #[source]
    pub source: io::Error,
}

impl IoReadError {
    /// Construct an [`IoReadError`] for `path` from a raw [`io::Error`].
    pub fn new(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self {
            path: path.into(),
            source,
        }
    }
}

/// Read an IaC input file as a UTF-8 string with the workspace-wide size cap
/// applied (see [`MAX_IAC_FILE_BYTES`]).
///
/// This is a thin wrapper over [`read_to_string_capped`] that attaches the
/// offending path to the returned error. Every IaC parser (CFn, Terraform,
/// Wrangler) should funnel its file reads through this helper so that:
///
/// 1. The 16 MiB cap is enforced consistently.
/// 2. Read failures carry the path in their `Display` output.
/// 3. Parser-specific error enums can convert via `#[from] IoReadError`.
pub fn read_iac_file(path: &Path) -> Result<String, IoReadError> {
    read_to_string_capped(path).map_err(|source| IoReadError::new(path, source))
}

/// A YAML scalar that could not be interpreted as an `f64`.
#[derive(Debug, Error)]
#[error("{0}")]
pub struct YamlNumberError(String);

/// A parameter-style YAML entry whose value is not a valid number.
#[derive(Debug, Error)]
#[error("{context} '{name}': invalid value")]
pub struct ParamValueError {
    /// What kind of entry failed (e.g. `param`, `profile base_param`).
    context: &'static str,
    /// Full (flattened) variable name of the offending entry.
    name: String,
    #[source]
    source: YamlNumberError,
}

/// Interpret a YAML scalar as an `f64`. Accepts numbers and numeric strings.
pub(crate) fn extract_f64(v: &serde_yaml_ng::Value) -> Result<f64, YamlNumberError> {
    match v {
        serde_yaml_ng::Value::Number(n) => n
            .as_f64()
            .ok_or_else(|| YamlNumberError(format!("cannot interpret number {v:?} as f64"))),
        serde_yaml_ng::Value::String(s) => s
            .parse::<f64>()
            .map_err(|_| YamlNumberError(format!("cannot interpret string {v:?} as f64"))),
        _ => Err(YamlNumberError(format!(
            "cannot interpret value {v:?} as a number"
        ))),
    }
}

/// Flatten a parsed YAML map of usage parameters into [`Params`].
///
/// Supports both flat (`Foo_requests: 100`) and hierarchical
/// (`Foo: { requests: 100 }`) entries; hierarchical entries are flattened to
/// `{logical_id}_{short_name}`. `context` labels error messages (e.g. `param`
/// or `profile base_param`).
pub(crate) fn params_from_yaml_map(
    map: std::collections::HashMap<String, serde_yaml_ng::Value>,
    context: &'static str,
) -> Result<Params, ParamValueError> {
    let mut params = Params::default();
    for (k, v) in map {
        match v {
            // Hierarchical: key is logical ID, value is a mapping of short var names
            serde_yaml_ng::Value::Mapping(sub_map) => {
                for (sub_k, sub_v) in sub_map {
                    let Some(sub_key) = sub_k.as_str() else {
                        tracing::warn!(key = ?sub_k, "non-string key in {context} mapping; skipping");
                        continue;
                    };
                    let full_name = format!("{k}_{sub_key}");
                    let val = extract_f64(&sub_v).map_err(|source| ParamValueError {
                        context,
                        name: full_name.clone(),
                        source,
                    })?;
                    params.insert(VariableName::new(full_name), val);
                }
            }
            // Flat: key is the full variable name
            _ => {
                let val = extract_f64(&v).map_err(|source| ParamValueError {
                    context,
                    name: k.clone(),
                    source,
                })?;
                params.insert(VariableName::new(k), val);
            }
        }
    }
    Ok(params)
}

/// Errors raised while parsing a usage-params YAML document.
#[derive(Debug, Error)]
pub enum ParamsParseError {
    /// The document is not a valid YAML mapping of variable names to values.
    #[error("failed to parse params file")]
    Yaml(#[source] serde_yaml_ng::Error),

    /// An entry holds a value that cannot be read as a number.
    #[error(transparent)]
    Value(#[from] ParamValueError),
}

/// Parse usage parameters from YAML text.
///
/// Supports both flat and hierarchical formats:
///
/// Flat (legacy):
/// ```yaml
/// IngestFunction_requests: 5000000
/// ```
///
/// Hierarchical:
/// ```yaml
/// IngestFunction:
///   requests: 5000000
/// ```
pub fn parse_params(content: &str) -> Result<Params, ParamsParseError> {
    let map: std::collections::HashMap<String, serde_yaml_ng::Value> =
        serde_yaml_ng::from_str(content).map_err(ParamsParseError::Yaml)?;
    Ok(params_from_yaml_map(map, "param")?)
}

/// Error raised while parsing a user-defined bindings YAML document.
#[derive(Debug, Error)]
#[error("failed to parse bindings file")]
pub struct BindingsParseError(#[source] serde_yaml_ng::Error);

/// Parse user-defined variable bindings from YAML text.
///
/// The document must follow the [`BindingsFile`] structure; entries with
/// invalid expressions are skipped with a warning (see
/// [`to_variable_bindings`]).
pub fn parse_bindings(content: &str) -> Result<Vec<VariableBinding>, BindingsParseError> {
    let file: BindingsFile = serde_yaml_ng::from_str(content).map_err(BindingsParseError)?;
    Ok(to_variable_bindings(&file.bindings))
}

/// Error raised while parsing a string-map YAML document
/// (e.g. CloudFormation parameter or import values).
#[derive(Debug, Error)]
#[error("failed to parse string map")]
pub struct StringMapParseError(#[source] serde_yaml_ng::Error);

/// Parse a flat YAML mapping of `name: scalar` entries into string values.
///
/// Numbers and booleans are stringified; entries with non-scalar values
/// (sequences, mappings, null) are silently skipped. Used for CloudFormation
/// parameter and cross-stack import value files.
pub fn parse_string_map(content: &str) -> Result<HashMap<String, String>, StringMapParseError> {
    let map: HashMap<String, serde_yaml_ng::Value> =
        serde_yaml_ng::from_str(content).map_err(StringMapParseError)?;

    let mut result = HashMap::new();
    for (k, v) in map {
        let val = match v {
            serde_yaml_ng::Value::String(s) => s,
            serde_yaml_ng::Value::Number(n) => n.to_string(),
            serde_yaml_ng::Value::Bool(b) => b.to_string(),
            _ => continue,
        };
        result.insert(k, val);
    }

    Ok(result)
}

/// Errors raised while parsing a quotas YAML document.
#[derive(Debug, Error)]
pub enum QuotasParseError {
    /// The document is not a valid YAML mapping of quota keys to numbers.
    #[error("failed to parse quotas file")]
    Yaml(#[source] serde_yaml_ng::Error),

    /// One or more keys are not provider-namespaced (contain no `.`).
    #[error(
        "quotas use non-namespaced keys ({keys}); quota files now use \
         provider-namespaced keys such as 'aws.lambda.concurrent_executions'. \
         Please migrate these keys."
    )]
    LegacyKeys {
        /// Comma-separated list of the offending keys.
        keys: String,
    },
}

/// Parse service quotas from YAML text.
///
/// Keys must be provider-namespaced (e.g. `aws.lambda.concurrent_executions`);
/// legacy non-namespaced keys are rejected with [`QuotasParseError::LegacyKeys`].
pub fn parse_quotas(content: &str) -> Result<Quotas, QuotasParseError> {
    let quotas: Quotas = serde_yaml_ng::from_str(content).map_err(QuotasParseError::Yaml)?;
    let legacy: Vec<&str> = quotas.keys().filter(|k| !k.contains('.')).collect();
    if !legacy.is_empty() {
        return Err(QuotasParseError::LegacyKeys {
            keys: legacy.join(", "),
        });
    }
    Ok(quotas)
}

/// Error raised while parsing a cost-model JSON document.
#[derive(Debug, Error)]
pub enum CostModelParseError {
    /// The document is not valid JSON or does not match the expected schema.
    #[error("failed to parse cost model JSON")]
    Json(#[source] serde_json::Error),
    /// One or more resources have inconsistent component currencies.
    #[error(transparent)]
    CurrencyMismatch(#[from] CostBuildError),
}

/// Parse a cost model (as produced by `generate`) from JSON text.
///
/// After JSON deserialization, each [`crate::cost::ResourceCost`] is validated for currency
/// consistency. Returns [`CostModelParseError::CurrencyMismatch`] when any
/// resource mixes currencies across its components — catching hand-edited JSON
/// that bypasses the [`crate::cost::ResourceCost::new`] constructor.
pub fn parse_cost_model(content: &str) -> Result<ArchitectureCost, CostModelParseError> {
    let arch: ArchitectureCost =
        serde_json::from_str(content).map_err(CostModelParseError::Json)?;
    arch.validate()?;
    Ok(arch)
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    #[test]
    fn read_to_string_capped_succeeds_for_small_file() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "yevice_core_io_test_small_{}.txt",
            std::process::id()
        ));
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"hello world").unwrap();
        drop(f);

        let result = read_to_string_capped(&path).unwrap();
        assert_eq!(result, "hello world");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_to_string_capped_fails_for_oversized_file() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "yevice_core_io_test_large_{}.bin",
            std::process::id()
        ));

        // Write MAX_IAC_FILE_BYTES + 1 bytes so it exceeds the limit.
        let size = (MAX_IAC_FILE_BYTES + 1) as usize;
        let data = vec![b'x'; size];
        std::fs::write(&path, &data).unwrap();

        let err = read_to_string_capped(&path).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            err.to_string().contains("file too large"),
            "error message should mention 'file too large', got: {err}"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_to_string_capped_succeeds_at_exact_limit() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "yevice_core_io_test_exact_{}.bin",
            std::process::id()
        ));

        // Write exactly MAX_IAC_FILE_BYTES bytes (all ASCII spaces).
        let size = MAX_IAC_FILE_BYTES as usize;
        let data = vec![b' '; size];
        std::fs::write(&path, &data).unwrap();

        let result = read_to_string_capped(&path);
        assert!(result.is_ok(), "exact limit should succeed");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_iac_file_attaches_path_to_error() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "yevice_core_io_test_missing_{}.txt",
            std::process::id()
        ));
        // Ensure the file is absent.
        let _ = std::fs::remove_file(&path);

        let err = read_iac_file(&path).expect_err("missing file must error");
        let msg = err.to_string();
        assert!(
            msg.contains(path.to_string_lossy().as_ref()),
            "error must mention the path; got: {msg}"
        );
        assert!(
            msg.contains("failed to read"),
            "error must mention 'failed to read'; got: {msg}"
        );
    }

    #[test]
    fn read_iac_file_passes_through_content() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("yevice_core_io_test_ok_{}.txt", std::process::id()));
        std::fs::write(&path, b"hello").unwrap();
        let content = read_iac_file(&path).unwrap();
        assert_eq!(content, "hello");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn parse_params_supports_flat_and_hierarchical_keys() {
        let yaml = "\
IngestFunction_requests: 5000000
Table:
  read_units: 25
  write_units: '12.5'
";
        let params = parse_params(yaml).unwrap();
        assert_eq!(
            params.get(&VariableName::new("IngestFunction_requests")),
            Some(&5_000_000.0)
        );
        assert_eq!(
            params.get(&VariableName::new("Table_read_units")),
            Some(&25.0)
        );
        assert_eq!(
            params.get(&VariableName::new("Table_write_units")),
            Some(&12.5),
            "numeric strings must be accepted"
        );
    }

    #[test]
    fn parse_params_rejects_non_numeric_value() {
        let err = parse_params("Foo_requests: [1, 2]\n").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("Foo_requests") && msg.contains("invalid value"),
            "error must name the offending param; got: {msg}"
        );
    }

    #[test]
    fn parse_params_rejects_invalid_yaml() {
        let err = parse_params(": not yaml :\n").unwrap_err();
        assert!(
            err.to_string().contains("failed to parse params file"),
            "got: {err}"
        );
    }

    #[test]
    fn parse_bindings_reads_simple_and_expr_modes() {
        let yaml = "\
bindings:
  - target: Worker_requests
    source: Queue_requests
    factor: 2
  - target: Bucket_storage_gb
    expr: \"Job_executions * 0.5\"
";
        let bindings = parse_bindings(yaml).unwrap();
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0].target, VariableName::new("Worker_requests"));
        assert_eq!(bindings[1].target, VariableName::new("Bucket_storage_gb"));
    }

    #[test]
    fn parse_bindings_rejects_invalid_yaml() {
        let err = parse_bindings("bindings: 42\n").unwrap_err();
        assert!(
            err.to_string().contains("failed to parse bindings file"),
            "got: {err}"
        );
    }

    #[test]
    fn parse_string_map_stringifies_scalars_and_skips_non_scalars() {
        let yaml = "\
Name: api
Count: 3
Enabled: true
Tags: [a, b]
Nested:
  key: value
";
        let map = parse_string_map(yaml).unwrap();
        assert_eq!(map.get("Name"), Some(&"api".to_string()));
        assert_eq!(map.get("Count"), Some(&"3".to_string()));
        assert_eq!(map.get("Enabled"), Some(&"true".to_string()));
        assert!(!map.contains_key("Tags"), "sequences must be skipped");
        assert!(!map.contains_key("Nested"), "mappings must be skipped");
    }

    #[test]
    fn parse_string_map_rejects_invalid_yaml() {
        let err = parse_string_map(": not yaml :\n").unwrap_err();
        assert!(
            err.to_string().contains("failed to parse string map"),
            "got: {err}"
        );
    }

    /// Non-namespaced key (no `.`) must cause `parse_quotas` to fail with a
    /// message that names the offending key.
    #[test]
    fn parse_quotas_rejects_non_namespaced_keys() {
        let err = parse_quotas("lambda_concurrent_executions: 50\n").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("lambda_concurrent_executions"),
            "error must name the non-namespaced key; got: {msg}"
        );
        assert!(
            msg.contains("non-namespaced"),
            "error must mention 'non-namespaced'; got: {msg}"
        );
    }

    /// Namespaced keys (containing `.`) must be accepted without error.
    #[test]
    fn parse_quotas_accepts_namespaced_keys() {
        let quotas = parse_quotas("aws.lambda.concurrent_executions: 50\n").unwrap();
        assert_eq!(
            quotas.get("aws.lambda.concurrent_executions"),
            Some(50.0),
            "namespaced quota value must be loaded correctly"
        );
    }

    #[test]
    fn parse_cost_model_reads_minimal_model() {
        let json = r#"{
            "name": "test",
            "resources": [],
            "region": "ap-northeast-1",
            "topology": { "nodes": [], "connections": [] }
        }"#;
        let model = parse_cost_model(json).unwrap();
        assert_eq!(model.name.as_str(), "test");
        assert!(model.resources.is_empty());
    }

    #[test]
    fn parse_cost_model_rejects_invalid_json() {
        let err = parse_cost_model("{ not json").unwrap_err();
        assert!(
            err.to_string().contains("failed to parse cost model JSON"),
            "got: {err}"
        );
    }

    /// A cost_model.json with mixed currencies (USD + JPY on the same resource)
    /// must be rejected at parse time, preventing silent incorrect summation.
    #[test]
    fn parse_cost_model_rejects_mixed_currency_resource() {
        let json = r#"{
            "name": "arch",
            "resources": [
                {
                    "logical_id": "MixedResource",
                    "resource_type": "AWS::Foo::Bar",
                    "label": "mixed",
                    "expr": { "type": "Constant", "value": 101.0 },
                    "components": [
                        {
                            "name": "usd_part",
                            "expr": { "type": "Constant", "value": 1.0 },
                            "currency": "USD"
                        },
                        {
                            "name": "jpy_part",
                            "expr": { "type": "Constant", "value": 100.0 },
                            "currency": "JPY"
                        }
                    ],
                    "required_variables": [],
                    "currency": "USD"
                }
            ],
            "region": "ap-northeast-1"
        }"#;
        let err = parse_cost_model(json).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("inconsistent component currencies"),
            "error must describe currency mismatch; got: {msg}"
        );
    }

    /// A cost_model.json with a single consistent currency must be accepted.
    #[test]
    fn parse_cost_model_accepts_single_currency_resource() {
        let json = r#"{
            "name": "arch",
            "resources": [
                {
                    "logical_id": "UsdResource",
                    "resource_type": "AWS::Foo::Bar",
                    "label": "usd",
                    "expr": { "type": "Constant", "value": 10.0 },
                    "components": [
                        {
                            "name": "part_a",
                            "expr": { "type": "Constant", "value": 6.0 },
                            "currency": "USD"
                        },
                        {
                            "name": "part_b",
                            "expr": { "type": "Constant", "value": 4.0 },
                            "currency": "USD"
                        }
                    ],
                    "required_variables": [],
                    "currency": "USD"
                }
            ],
            "region": "ap-northeast-1"
        }"#;
        let model = parse_cost_model(json).unwrap();
        assert_eq!(model.resources.len(), 1);
    }
}
