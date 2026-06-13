//! IaC input → [`Architecture`] construction.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use yevice_core::parse_policy::{ParseOutcome, ParsePolicy};
use yevice_core::resource::Architecture;

use crate::DEFAULT_ARCHITECTURE_NAME;
use crate::error::EngineError;
use crate::input::{InputFormat, find_wrangler_config};
use crate::registry::Registries;

/// Pre-parsed CloudFormation parameter and cross-stack import values.
///
/// File reading and YAML parsing are the caller's responsibility (see
/// [`yevice_core::io::parse_string_map`]); the engine only consumes the
/// resulting maps. Both maps are empty for Terraform and Wrangler inputs.
#[derive(Debug, Clone, Default)]
pub struct CfnInputs {
    /// CloudFormation parameter values (overriding template defaults).
    pub parameters: HashMap<String, String>,
    /// Cross-stack `Fn::ImportValue` values.
    pub imports: HashMap<String, String>,
}

/// Parse and resolve a CloudFormation template from a file path.
///
/// Path-based convenience over [`resolve_cfn_template_str`]. Defaults to
/// [`ParsePolicy::Strict`] for backward compatibility.
pub fn resolve_cfn_template(
    template_path: &Path,
    inputs: &CfnInputs,
) -> Result<yevice_cfn::parser::ResolvedTemplate, EngineError> {
    let outcome = resolve_cfn_template_with_policy(template_path, inputs, ParsePolicy::Strict)?;
    Ok(outcome.value)
}

/// Parse and resolve a CloudFormation template from a file path under the
/// given [`ParsePolicy`], returning structured diagnostics.
pub fn resolve_cfn_template_with_policy(
    template_path: &Path,
    inputs: &CfnInputs,
    policy: ParsePolicy,
) -> Result<ParseOutcome<yevice_cfn::parser::ResolvedTemplate>, EngineError> {
    let template =
        yevice_cfn::parser::parse_template(template_path).map_err(EngineError::CfnParse)?;
    resolve_parsed_template(template, inputs, policy, Some(template_path))
}

/// Parse and resolve a CloudFormation template from YAML text.
///
/// String-based core of [`resolve_cfn_template`], suitable for hosts that
/// receive templates over the wire instead of from the filesystem. Defaults
/// to [`ParsePolicy::Strict`].
pub fn resolve_cfn_template_str(
    template_src: &str,
    inputs: &CfnInputs,
) -> Result<yevice_cfn::parser::ResolvedTemplate, EngineError> {
    let outcome = resolve_cfn_template_str_with_policy(template_src, inputs, ParsePolicy::Strict)?;
    Ok(outcome.value)
}

/// String-based variant of [`resolve_cfn_template_with_policy`].
pub fn resolve_cfn_template_str_with_policy(
    template_src: &str,
    inputs: &CfnInputs,
    policy: ParsePolicy,
) -> Result<ParseOutcome<yevice_cfn::parser::ResolvedTemplate>, EngineError> {
    let template =
        yevice_cfn::parser::parse_template_str(template_src).map_err(EngineError::CfnParse)?;
    resolve_parsed_template(template, inputs, policy, None)
}

fn resolve_parsed_template(
    template: yevice_cfn::parser::CfnTemplate,
    inputs: &CfnInputs,
    policy: ParsePolicy,
    template_path: Option<&Path>,
) -> Result<ParseOutcome<yevice_cfn::parser::ResolvedTemplate>, EngineError> {
    let outcome = yevice_cfn::parser::resolve_template_with_policy(
        &template,
        &inputs.parameters,
        &inputs.imports,
        policy,
        template_path,
    )
    .map_err(EngineError::CfnResolve)?;

    let resolved_template = yevice_cfn::parser::CfnTemplate {
        parameters: template.parameters,
        mappings: template.mappings,
        conditions: template.conditions,
        resources: outcome.value,
    };
    Ok(ParseOutcome {
        value: resolved_template,
        diagnostics: outcome.diagnostics,
        had_errors: outcome.had_errors,
    })
}

/// Build an [`Architecture`] from an IaC input of the given format.
///
/// `architecture_name` falls back to [`DEFAULT_ARCHITECTURE_NAME`] when
/// `None`. For Wrangler inputs the name embedded in the config wins unless an
/// explicit non-default name is supplied.
///
/// Defaults to [`ParsePolicy::Strict`] for backward compatibility. New
/// callers should prefer [`build_architecture_from_input_with_policy`].
pub fn build_architecture_from_input(
    format: InputFormat,
    template_path: &Path,
    cfn_inputs: &CfnInputs,
    architecture_name: Option<&str>,
    region: &str,
    registries: &Registries,
) -> Result<Architecture, EngineError> {
    let outcome = build_architecture_from_input_with_policy(
        format,
        template_path,
        cfn_inputs,
        architecture_name,
        region,
        registries,
        ParsePolicy::Strict,
    )?;
    Ok(outcome.value)
}

/// Policy-aware variant of [`build_architecture_from_input`].
///
/// Returns a [`ParseOutcome`] so callers can surface IaC parse diagnostics
/// alongside the constructed architecture.
pub fn build_architecture_from_input_with_policy(
    format: InputFormat,
    template_path: &Path,
    cfn_inputs: &CfnInputs,
    architecture_name: Option<&str>,
    region: &str,
    registries: &Registries,
    policy: ParsePolicy,
) -> Result<ParseOutcome<Architecture>, EngineError> {
    match format {
        InputFormat::Cfn => {
            let template_outcome =
                resolve_cfn_template_with_policy(template_path, cfn_inputs, policy)?;
            let architecture = yevice_cfn::convert::build_architecture(
                architecture_name.unwrap_or(DEFAULT_ARCHITECTURE_NAME),
                region,
                &template_outcome.value,
                &registries.cfn_adapters,
            );
            Ok(ParseOutcome {
                value: architecture,
                diagnostics: template_outcome.diagnostics,
                had_errors: template_outcome.had_errors,
            })
        }
        InputFormat::Tf => {
            let resolved_outcome = resolve_tf_input_with_policy(template_path, policy)?;
            if detect_tf_provider(&resolved_outcome.value) == TfProvider::Unknown {
                return Err(EngineError::UnknownTfProvider {
                    path: template_path.to_path_buf(),
                });
            }

            let architecture = yevice_tf::build_architecture(
                architecture_name.unwrap_or(DEFAULT_ARCHITECTURE_NAME),
                region,
                &resolved_outcome.value,
                &registries.tf_adapters,
            );
            Ok(ParseOutcome {
                value: architecture,
                diagnostics: resolved_outcome.diagnostics,
                had_errors: resolved_outcome.had_errors,
            })
        }
        InputFormat::Wrangler => {
            let wrangler_path = resolve_wrangler_input_path(template_path)?;
            let outcome = yevice_wrangler::parse_wrangler_with_policy(&wrangler_path, policy)
                .map_err(|source| EngineError::WranglerParse {
                    path: wrangler_path.clone(),
                    source,
                })?;

            let mut architecture = outcome.value;
            if let Some(name_override) =
                architecture_name.filter(|name| *name != DEFAULT_ARCHITECTURE_NAME)
            {
                architecture.name = name_override.to_string();
            }

            Ok(ParseOutcome {
                value: architecture,
                diagnostics: outcome.diagnostics,
                had_errors: outcome.had_errors,
            })
        }
    }
}

/// Parse a Terraform configuration (file or directory) into a resolved config.
///
/// Variable files are merged in Terraform precedence order (lowest first;
/// later wins):
///
/// 1. `terraform.tfvars`
/// 2. `*.auto.tfvars` (alphabetical)
/// 3. an explicit `*.tfvars` passed as the input path
///
/// (HCL `.tfvars` only; the JSON variants are not parsed here.)
pub fn resolve_tf_input(path: &Path) -> Result<yevice_tf::ResolvedConfig, EngineError> {
    // Strict mirrors the historical CFN-shaped semantics; callers that want
    // diagnostics should use `resolve_tf_input_with_policy`.
    let outcome = resolve_tf_input_with_policy(path, ParsePolicy::Strict)?;
    Ok(outcome.value)
}

/// Policy-aware variant of [`resolve_tf_input`] that returns a
/// [`ParseOutcome`] carrying any structured Terraform diagnostics
/// (unresolved `var.*` / `local.*` under Lenient).
pub fn resolve_tf_input_with_policy(
    path: &Path,
    policy: ParsePolicy,
) -> Result<ParseOutcome<yevice_tf::ResolvedConfig>, EngineError> {
    let config_dir = terraform_config_dir(path)?;
    let config = yevice_tf::parse_tf_dir(config_dir).map_err(|source| EngineError::TfParse {
        path: config_dir.to_path_buf(),
        source,
    })?;

    let mut candidates: Vec<PathBuf> = Vec::new();
    let default_tfvars = config_dir.join("terraform.tfvars");
    if default_tfvars.is_file() {
        candidates.push(default_tfvars);
    }
    if let Ok(entries) = std::fs::read_dir(config_dir) {
        let mut autos: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with(".auto.tfvars"))
            })
            .collect();
        autos.sort();
        candidates.extend(autos);
    }
    let path_is_tfvars = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("tfvars"));
    if path_is_tfvars && path.is_file() && !candidates.iter().any(|c| c == path) {
        candidates.push(path.to_path_buf());
    }

    let mut merged = HashMap::new();
    for file in &candidates {
        let vars = yevice_tf::parse_tfvars(file).map_err(|source| EngineError::TfVarsParse {
            path: file.clone(),
            source,
        })?;
        merged.extend(vars);
    }
    let tfvars = if merged.is_empty() {
        None
    } else {
        Some(merged)
    };

    yevice_tf::resolve_config_with_policy(config, tfvars, policy).map_err(EngineError::TfResolve)
}

fn terraform_config_dir(path: &Path) -> Result<&Path, EngineError> {
    if path.is_dir() {
        return Ok(path);
    }

    path.parent().ok_or_else(|| EngineError::TfConfigDir {
        path: path.to_path_buf(),
    })
}

fn resolve_wrangler_input_path(path: &Path) -> Result<PathBuf, EngineError> {
    if path.is_dir() {
        return find_wrangler_config(path).ok_or_else(|| EngineError::WranglerConfigNotFound {
            path: path.to_path_buf(),
        });
    }

    Ok(path.to_path_buf())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TfProvider {
    Aws,
    Gcp,
    Mixed,
    Unknown,
}

fn detect_tf_provider(resolved: &yevice_tf::ResolvedConfig) -> TfProvider {
    let mut has_aws = false;
    let mut has_gcp = false;

    for resource in &resolved.resources {
        if resource.resource_type.starts_with("aws_") {
            has_aws = true;
        } else if resource.resource_type.starts_with("google_") {
            has_gcp = true;
        }
    }

    match (has_aws, has_gcp) {
        (true, true) => TfProvider::Mixed,
        (true, false) => TfProvider::Aws,
        (false, true) => TfProvider::Gcp,
        (false, false) => TfProvider::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use yevice_tf::parser::TfResource;

    fn tf_resource(resource_type: &str) -> TfResource {
        TfResource {
            resource_type: resource_type.to_string(),
            name: "sample".to_string(),
            attrs: HashMap::new(),
            blocks: HashMap::new(),
        }
    }

    fn resolved_config(resources: Vec<TfResource>) -> yevice_tf::ResolvedConfig {
        yevice_tf::ResolvedConfig {
            resources,
            vars: HashMap::new(),
            locals: HashMap::new(),
        }
    }

    #[test]
    fn detects_terraform_provider_from_resolved_config() {
        let aws = resolved_config(vec![tf_resource("aws_s3_bucket")]);
        assert_eq!(detect_tf_provider(&aws), TfProvider::Aws);

        let gcp = resolved_config(vec![tf_resource("google_storage_bucket")]);
        assert_eq!(detect_tf_provider(&gcp), TfProvider::Gcp);

        let mixed = resolved_config(vec![
            tf_resource("aws_s3_bucket"),
            tf_resource("google_storage_bucket"),
        ]);
        assert_eq!(detect_tf_provider(&mixed), TfProvider::Mixed);

        let unknown = resolved_config(vec![tf_resource("azurerm_storage_account")]);
        assert_eq!(detect_tf_provider(&unknown), TfProvider::Unknown);
    }

    #[test]
    fn resolve_cfn_template_str_resolves_parameters() {
        let template = "\
Parameters:
  TableName:
    Type: String
Resources:
  Table:
    Type: AWS::DynamoDB::Table
    Properties:
      TableName: !Ref TableName
";
        let mut inputs = CfnInputs::default();
        inputs
            .parameters
            .insert("TableName".to_string(), "orders".to_string());

        let resolved = resolve_cfn_template_str(template, &inputs).unwrap();
        assert!(resolved.resources.contains_key("Table"));
    }

    #[test]
    fn resolve_cfn_template_str_reports_missing_parameters() {
        let template = "\
Parameters:
  TableName:
    Type: String
Resources:
  Table:
    Type: AWS::DynamoDB::Table
";
        let result = resolve_cfn_template_str(template, &CfnInputs::default());
        let Err(err) = result else {
            panic!("expected missing-parameter error");
        };
        assert!(matches!(err, EngineError::CfnResolve(_)), "got: {err:?}");
    }

    #[test]
    fn wrangler_directory_without_config_is_an_error() {
        let dir = std::env::temp_dir().join(format!(
            "yevice-engine-wrangler-missing-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let err = resolve_wrangler_input_path(&dir).unwrap_err();
        assert!(matches!(err, EngineError::WranglerConfigNotFound { .. }));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
