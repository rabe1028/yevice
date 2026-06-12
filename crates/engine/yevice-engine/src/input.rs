//! Input-format detection for IaC template paths.

use std::path::{Path, PathBuf};

use crate::error::EngineError;

/// Supported IaC input formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputFormat {
    /// AWS CloudFormation (YAML/JSON template).
    Cfn,
    /// Terraform (HCL file or directory).
    Tf,
    /// Cloudflare Wrangler (`wrangler.toml` / `wrangler.jsonc`).
    Wrangler,
}

/// Return `requested` when given, otherwise detect the format from the path.
pub fn resolve_input_format(
    template_path: &Path,
    requested: Option<InputFormat>,
) -> Result<InputFormat, EngineError> {
    match requested {
        Some(format) => Ok(format),
        None => detect_input_format(template_path),
    }
}

/// Detect the input format from a template path heuristically.
///
/// Directories are probed for `.tf` files first, then for a Wrangler config.
/// Files are classified by extension (`.yaml`/`.yml`/`.json` → CloudFormation,
/// `.tf`/`.tfvars` → Terraform, `.toml` or a `wrangler.*` file name → Wrangler).
pub fn detect_input_format(path: &Path) -> Result<InputFormat, EngineError> {
    if path.is_dir() {
        if directory_contains_tf_files(path)? {
            return Ok(InputFormat::Tf);
        }
        if find_wrangler_config(path).is_some() {
            return Ok(InputFormat::Wrangler);
        }
    }

    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_ascii_lowercase);

    match (extension.as_deref(), file_name.as_deref()) {
        (Some("yaml" | "yml" | "json"), _) => Ok(InputFormat::Cfn),
        (Some("tf" | "tfvars"), _) => Ok(InputFormat::Tf),
        (Some("toml"), _) | (_, Some("wrangler.toml" | "wrangler.jsonc")) => {
            Ok(InputFormat::Wrangler)
        }
        _ => Err(EngineError::UnknownInputFormat {
            path: path.to_path_buf(),
        }),
    }
}

fn directory_contains_tf_files(path: &Path) -> Result<bool, EngineError> {
    if !path.is_dir() {
        return Ok(false);
    }

    let entries = std::fs::read_dir(path).map_err(|source| EngineError::ReadDir {
        path: path.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| EngineError::ReadDir {
            path: path.to_path_buf(),
            source,
        })?;
        if entry.path().extension().is_some_and(|ext| ext == "tf") {
            return Ok(true);
        }
    }

    Ok(false)
}

/// Locate `wrangler.toml` or `wrangler.jsonc` directly inside `path`.
pub(crate) fn find_wrangler_config(path: &Path) -> Option<PathBuf> {
    ["wrangler.toml", "wrangler.jsonc"]
        .into_iter()
        .map(|name| path.join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("yevice-engine-{label}-{unique}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn detects_input_formats_from_paths() {
        assert_eq!(
            detect_input_format(Path::new("template.yaml")).unwrap(),
            InputFormat::Cfn
        );
        assert_eq!(
            detect_input_format(Path::new("main.tf")).unwrap(),
            InputFormat::Tf
        );
        assert_eq!(
            detect_input_format(Path::new("terraform.tfvars")).unwrap(),
            InputFormat::Tf
        );
        assert_eq!(
            detect_input_format(Path::new("wrangler.toml")).unwrap(),
            InputFormat::Wrangler
        );
        assert_eq!(
            detect_input_format(Path::new("wrangler.jsonc")).unwrap(),
            InputFormat::Wrangler
        );
    }

    #[test]
    fn detects_directory_inputs() {
        let tf_dir = temp_dir("detect-tf-dir");
        fs::write(
            tf_dir.join("main.tf"),
            "resource \"google_pubsub_topic\" \"events\" {}\n",
        )
        .unwrap();
        assert_eq!(detect_input_format(&tf_dir).unwrap(), InputFormat::Tf);
        fs::remove_dir_all(&tf_dir).unwrap();

        let wrangler_dir = temp_dir("detect-wrangler-dir");
        fs::write(
            wrangler_dir.join("wrangler.toml"),
            "name = \"edge-worker\"\n",
        )
        .unwrap();
        assert_eq!(
            detect_input_format(&wrangler_dir).unwrap(),
            InputFormat::Wrangler
        );
        fs::remove_dir_all(&wrangler_dir).unwrap();
    }

    #[test]
    fn unknown_extension_returns_unknown_format_error() {
        let err = detect_input_format(Path::new("input.xyz")).unwrap_err();
        assert!(matches!(err, EngineError::UnknownInputFormat { .. }));
        assert!(
            err.to_string().contains("input.xyz"),
            "error must name the path; got: {err}"
        );
    }

    #[test]
    fn resolve_input_format_prefers_explicit_request() {
        let format = resolve_input_format(Path::new("input.xyz"), Some(InputFormat::Tf)).unwrap();
        assert_eq!(format, InputFormat::Tf);
    }
}
