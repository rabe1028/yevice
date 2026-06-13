//! Orchestration engine for the yevice cost-model toolkit.
//!
//! `yevice-engine` owns the end-to-end *generate* pipeline — input-format
//! detection, IaC parsing (CloudFormation / Terraform / Wrangler),
//! [`Architecture`](yevice_core::resource::Architecture) construction, and
//! cost-model generation — as a plain library, so that non-CLI hosts (e.g. a
//! web service) can drive the same pipeline without spawning a process. It
//! never prints, never exits the process, and reports failures through
//! [`EngineError`]. The dependency direction is:
//!
//! ```text
//! yevice-cli ──▶ yevice-engine ──▶ {yevice-cfn, yevice-tf, yevice-wrangler}
//!     │               │                          │
//!     │               └────────▶ yevice-service-api ──▶ yevice-core
//!     │
//!     └──▶ yevice-services-{aws,gcp} / yevice-wrangler  (ProviderPlugin impls)
//! ```
//!
//! The engine deliberately does **not** depend on the provider service crates
//! (`yevice-services-aws` / `yevice-services-gcp`): callers inject providers
//! as `&[Box<dyn ProviderPlugin>]`, keeping the provider set open-ended.

pub mod architecture;
pub mod error;
pub mod generate;
pub mod input;
pub mod registry;

pub use architecture::{
    CfnInputs, build_architecture_from_input, build_architecture_from_input_with_policy,
    resolve_cfn_template, resolve_cfn_template_str, resolve_cfn_template_str_with_policy,
    resolve_cfn_template_with_policy, resolve_tf_input, resolve_tf_input_with_policy,
};
pub use error::EngineError;
pub use generate::{GenerateRequest, generate_cost_model};
pub use input::{InputFormat, detect_input_format, resolve_input_format};
pub use registry::{Registries, build_pricing_resolver, build_registries};

/// Architecture name used when the caller does not supply one.
pub const DEFAULT_ARCHITECTURE_NAME: &str = "default";
