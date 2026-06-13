//! The `generate` orchestration: IaC input → cost model.

use std::collections::HashMap;
use std::path::Path;

use yevice_core::cost::ArchitectureCost;
use yevice_core::parse_policy::{ParseOutcome, ParsePolicy};
use yevice_core::resource::Provider;
use yevice_service_api::ProviderPlugin;

use crate::architecture::{CfnInputs, build_architecture_from_input_with_policy};
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
    /// Parse-failure policy applied to IaC parsers (ADR-0003).
    pub policy: ParsePolicy,
}

/// Run the full generate pipeline: parse the IaC input, build the
/// [`Architecture`](yevice_core::resource::Architecture), assemble pricing
/// catalogs for the providers present, and build the cost model.
///
/// Provider support is injected through `plugins`; the engine itself is
/// provider-agnostic. The returned [`ParseOutcome`] carries any
/// IaC parse diagnostics collected under
/// [`ParsePolicy::Lenient`].
pub fn generate_cost_model(
    plugins: &[Box<dyn ProviderPlugin>],
    request: &GenerateRequest<'_>,
) -> Result<ParseOutcome<ArchitectureCost>, EngineError> {
    let registries = build_registries(plugins);
    let arch_outcome = build_architecture_from_input_with_policy(
        request.format,
        request.template_path,
        &request.cfn_inputs,
        Some(request.name),
        request.region,
        &registries,
        request.policy,
    )?;

    let pricing = build_pricing_resolver(
        plugins,
        &arch_outcome.value,
        request.region,
        request.provider_regions,
        request.list_price,
    );

    let mut cost_model = registries
        .catalog
        .build_cost_model(&arch_outcome.value, &pricing, request.strict)
        .map_err(EngineError::CostModel)?;
    // Embed diagnostics into the cost model so `cost_model.json` carries
    // them (ADR-0003 JSON 出力 section).
    cost_model.diagnostics.clone_from(&arch_outcome.diagnostics);
    Ok(ParseOutcome {
        value: cost_model,
        diagnostics: arch_outcome.diagnostics,
        had_errors: arch_outcome.had_errors,
    })
}
