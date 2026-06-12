//! The `generate` orchestration: IaC input → cost model.

use std::collections::HashMap;
use std::path::Path;

use yevice_core::cost::ArchitectureCost;
use yevice_core::resource::Provider;
use yevice_service_api::ProviderPlugin;

use crate::architecture::{CfnInputs, build_architecture_from_input};
use crate::error::EngineError;
use crate::input::InputFormat;
use crate::registry::{build_pricing_resolver, build_registries};

/// Inputs for [`generate_cost_model`].
pub struct GenerateRequest<'a> {
    /// Resolved input format (see [`crate::resolve_input_format`]).
    pub format: InputFormat,
    /// Path to the IaC template file or directory.
    pub template_path: &'a Path,
    /// Pre-parsed CloudFormation parameter/import values (CFN only).
    pub cfn_inputs: CfnInputs,
    /// Architecture name embedded in the cost model.
    pub name: &'a str,
    /// Default pricing region.
    pub region: &'a str,
    /// Per-provider pricing-region overrides; absent providers use `region`.
    pub provider_regions: &'a HashMap<Provider, String>,
    /// Fail on unsupported resource types instead of skipping them.
    pub strict: bool,
    /// Ignore promotional free-tier allowances in pricing catalogs.
    pub list_price: bool,
}

/// Run the full generate pipeline: parse the IaC input, build the
/// [`Architecture`](yevice_core::resource::Architecture), assemble pricing
/// catalogs for the providers present, and build the cost model.
///
/// Provider support is injected through `plugins`; the engine itself is
/// provider-agnostic.
pub fn generate_cost_model(
    plugins: &[Box<dyn ProviderPlugin>],
    request: &GenerateRequest<'_>,
) -> Result<ArchitectureCost, EngineError> {
    let registries = build_registries(plugins);
    let architecture = build_architecture_from_input(
        request.format,
        request.template_path,
        &request.cfn_inputs,
        Some(request.name),
        request.region,
        &registries,
    )?;

    let pricing = build_pricing_resolver(
        plugins,
        &architecture,
        request.region,
        request.provider_regions,
        request.list_price,
    );

    registries
        .catalog
        .build_cost_model(&architecture, &pricing, request.strict)
        .map_err(EngineError::CostModel)
}
