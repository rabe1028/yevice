//! Engine error type.

use std::path::PathBuf;

use thiserror::Error;
use yevice_cfn::CfnError;
use yevice_service_api::CostError;
use yevice_tf::TfError;
use yevice_wrangler::WranglerError;

/// Errors raised while orchestrating the IaC-input → cost-model pipeline.
#[derive(Debug, Error)]
pub enum EngineError {
    /// The input format could not be inferred from the template path.
    #[error(
        "could not detect input format for {}: expected a CloudFormation template \
         (.yaml/.yml/.json), Terraform input (.tf/.tfvars or a directory containing .tf files), \
         or a Wrangler config (wrangler.toml/wrangler.jsonc)",
        path.display()
    )]
    UnknownInputFormat {
        /// The template path whose format could not be detected.
        path: PathBuf,
    },

    /// A directory passed as input could not be read.
    #[error("failed to read directory: {}", path.display())]
    ReadDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The CloudFormation template could not be parsed.
    #[error("failed to parse CloudFormation template")]
    CfnParse(#[source] CfnError),

    /// Intrinsic functions in the CloudFormation template could not be resolved.
    #[error("failed to resolve template")]
    CfnResolve(#[source] CfnError),

    /// The Terraform configuration could not be parsed.
    #[error("failed to parse Terraform config: {}", path.display())]
    TfParse {
        path: PathBuf,
        #[source]
        source: TfError,
    },

    /// A Terraform variables file could not be parsed.
    #[error("failed to parse Terraform variables: {}", path.display())]
    TfVarsParse {
        path: PathBuf,
        #[source]
        source: TfError,
    },

    /// The Terraform configuration could not be resolved.
    #[error("failed to resolve Terraform configuration")]
    TfResolve(#[source] TfError),

    /// No supported Terraform provider was detected in the configuration.
    #[error(
        "unable to detect a supported Terraform provider from {}. \
         Expected resources with aws_ or google_ prefixes.",
        path.display()
    )]
    UnknownTfProvider { path: PathBuf },

    /// The Terraform configuration directory could not be determined.
    #[error("failed to determine Terraform configuration directory for {}", path.display())]
    TfConfigDir { path: PathBuf },

    /// No Wrangler config file was found in the given directory.
    #[error("failed to locate wrangler.toml or wrangler.jsonc in {}", path.display())]
    WranglerConfigNotFound { path: PathBuf },

    /// The Wrangler config could not be parsed.
    #[error("failed to parse Wrangler config: {}", path.display())]
    WranglerParse {
        path: PathBuf,
        #[source]
        source: WranglerError,
    },

    /// The service catalog failed to build a cost model from the architecture.
    #[error("failed to build cost model")]
    CostModel(#[source] CostError),
}
