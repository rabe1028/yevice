use std::io::{self, ErrorKind};
use std::path::Path;

use thiserror::Error;

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
}
