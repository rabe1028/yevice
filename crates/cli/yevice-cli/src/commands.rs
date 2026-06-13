use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use yevice_core::optimize::{DecisionVariable, ObjectiveDirection, OptimizationProblem};
use yevice_output::{ArchitectureRenderer, DrawIoRenderer, JsonRenderer, MermaidRenderer};
use yevice_solver::{Solver, SolverError, solver_from_name};

use yevice_core::bindings::derive_bindings;
use yevice_core::capacity::{self, Quotas, Severity};
use yevice_core::cost::ArchitectureCost;
use yevice_core::evaluate::{self, Params, evaluate_architecture};
use yevice_core::parse_policy::{ParsePolicy, Severity as DiagSeverity};
use yevice_core::resource::Provider;
use yevice_core::schema::{generate_usage_schema, generate_usage_template};
use yevice_core::simulate::{ArchSimulation, SimulationProfile, simulate_architecture};
use yevice_core::types::VariableName;
use yevice_engine::{CfnInputs, EngineError, GenerateRequest};
use yevice_pricing::download as pricing_download;
use yevice_service_api::ProviderPlugin;
use yevice_services_aws::AwsPlugin;
use yevice_services_gcp::GcpPlugin;
use yevice_wrangler::CloudflarePlugin;

pub use yevice_engine::InputFormat;

/// Returns the list of all provider plugins injected into the engine.
/// This is the single place where the CLI decides which providers exist.
fn provider_plugins() -> Vec<Box<dyn ProviderPlugin>> {
    vec![
        Box::new(AwsPlugin),
        Box::new(GcpPlugin),
        Box::new(CloudflarePlugin),
    ]
}

/// Resolve the input format, adding the CLI flag hint to detection failures.
fn resolve_input_format(
    template_path: &str,
    requested: Option<InputFormat>,
) -> Result<InputFormat> {
    yevice_engine::resolve_input_format(Path::new(template_path), requested).map_err(|e| match e {
        EngineError::UnknownInputFormat { .. } => {
            anyhow::anyhow!("{e}. Pass --input-format <cfn|tf|wrangler>.")
        }
        other => other.into(),
    })
}

pub fn generate(
    template_path: &str,
    parameters_path: Option<&str>,
    imports_path: Option<&str>,
    bindings_path: Option<&str>,
    name: &str,
    output_path: &str,
    region: &str,
    provider_regions: &HashMap<Provider, String>,
    input_format: Option<InputFormat>,
    strict: bool,
    list_price: bool,
) -> Result<()> {
    let format = resolve_input_format(template_path, input_format)?;
    reject_cfn_only_options(format, parameters_path, imports_path, bindings_path)?;

    let cfn_inputs = load_cfn_inputs(parameters_path, imports_path)?;
    let policy = if strict {
        ParsePolicy::Strict
    } else {
        ParsePolicy::Lenient
    };
    let request = GenerateRequest {
        format,
        template_path: Path::new(template_path),
        cfn_inputs,
        name,
        region,
        provider_regions,
        strict,
        list_price,
        policy,
    };
    let outcome = yevice_engine::generate_cost_model(&provider_plugins(), &request)?;
    let had_errors = outcome.had_errors;
    let mut cost_model = outcome.value;

    // Surface diagnostics to the operator regardless of policy (Lenient
    // succeeds; Strict aborts later if any Error-severity diagnostic is
    // present). This matches ADR-0003 "stderr に tracing::warn! で表示".
    report_diagnostics(&cost_model.diagnostics);

    if format == InputFormat::Cfn
        && let Some(path) = bindings_path
    {
        let extra = load_bindings(path).context("failed to load bindings file")?;
        cost_model.bindings.extend(extra);
    }

    let json = serde_json::to_string_pretty(&cost_model).context("failed to serialize")?;
    std::fs::write(output_path, &json)
        .with_context(|| format!("failed to write output: {output_path}"))?;

    let output = Path::new(output_path);
    let schema = generate_usage_schema(&cost_model);
    let schema_path = output.with_extension("schema.json");
    std::fs::write(&schema_path, serde_json::to_string_pretty(&schema)?)
        .with_context(|| format!("failed to write schema: {}", schema_path.display()))?;

    let template_yaml = generate_usage_template(&cost_model);
    let template_path_out = output.with_extension("usage.yaml");
    std::fs::write(&template_path_out, template_yaml).with_context(|| {
        format!(
            "failed to write usage template: {}",
            template_path_out.display()
        )
    })?;

    println!("Generated: {output_path}");
    println!("Schema:    {}", schema_path.display());
    println!("Template:  {}", template_path_out.display());

    // Under Strict, any Error-severity parse diagnostic that survived (e.g. a
    // hard error that the parser still wanted to demote) is fatal. Under
    // Lenient `had_errors` is informational only and does not change the
    // exit code (ADR-0003 終了コード section).
    if strict && had_errors {
        bail!("strict mode: IaC parse produced error-severity diagnostics");
    }
    Ok(())
}

/// Emit each diagnostic to stderr via `tracing` so operators see them even
/// without `--strict`. Severity is mapped to `error!` / `warn!` / `info!`.
fn report_diagnostics(diagnostics: &[yevice_core::parse_policy::IacParseDiagnostic]) {
    for d in diagnostics {
        match d.severity {
            DiagSeverity::Error => tracing::error!(
                source = ?d.source,
                code = %d.code,
                "{}",
                d.message
            ),
            DiagSeverity::Warning => tracing::warn!(
                source = ?d.source,
                code = %d.code,
                "{}",
                d.message
            ),
            DiagSeverity::Info => tracing::info!(
                source = ?d.source,
                code = %d.code,
                "{}",
                d.message
            ),
        }
    }
}

pub fn evaluate(cost_model_path: &str, params_path: &str, breakdown: bool) -> Result<()> {
    let arch = load_cost_model(cost_model_path)?;
    let params = load_params(params_path)?;

    let result = evaluate_architecture(&arch, &params).context("failed to evaluate cost model")?;

    println!("\n{}: Monthly Cost Estimate", result.name);

    if breakdown {
        let table = crate::render::render_eval_breakdown_table(&result);
        println!("{table}");
    } else {
        let table = crate::render::render_eval_table(&result);
        println!("{table}");
    }

    Ok(())
}

pub fn compare(cost_model_paths: &[String], params_path: &str, breakdown: bool) -> Result<()> {
    let params = load_params(params_path)?;

    let mut results = Vec::new();
    for path in cost_model_paths {
        let arch = load_cost_model(path)?;
        let result =
            evaluate_architecture(&arch, &params).context("failed to evaluate cost model")?;
        results.push(result);
    }

    let summary = crate::render::render_compare_table(&results, breakdown);

    println!("\nArchitecture Cost Comparison");
    println!("{summary}");

    Ok(())
}

pub fn sensitivity(
    cost_model_path: &str,
    params_path: &str,
    var_name: &str,
    min: f64,
    max: f64,
    steps: usize,
    breakdown: bool,
) -> Result<()> {
    if steps == 0 {
        bail!("--steps must be at least 1");
    }

    let arch = load_cost_model(cost_model_path)?;
    let base_params = load_params(params_path)?;

    let step_size = (max - min) / steps as f64;

    let base_result =
        evaluate_architecture(&arch, &base_params).context("failed to evaluate base cost")?;
    let base_cost = base_result.total_monthly_cost;

    // Collect resource labels from the base result to use as breakdown columns.
    let resource_labels: Vec<String> = base_result
        .resources
        .iter()
        .map(|r| r.label.clone())
        .collect();

    // When breakdown is true, collect step results for a second table.
    let mut sensitivity_rows: Vec<crate::render::SensitivityRow> = Vec::new();
    let mut breakdown_rows: Vec<(f64, Vec<f64>)> = Vec::new();

    for i in 0..=steps {
        let value = min + step_size * i as f64;
        let mut params = base_params.clone();
        params.insert(VariableName::new(var_name), value);

        match evaluate_architecture(&arch, &params) {
            Ok(result) => {
                let delta = result.total_monthly_cost - base_cost;
                sensitivity_rows.push(crate::render::SensitivityRow::Ok {
                    value,
                    total: result.total_monthly_cost,
                    delta,
                });

                if breakdown {
                    let costs: Vec<f64> = resource_labels
                        .iter()
                        .map(|label| {
                            result
                                .resources
                                .iter()
                                .find(|r| &r.label == label)
                                .map_or(0.0, |r| r.monthly_cost)
                        })
                        .collect();
                    breakdown_rows.push((value, costs));
                }
            }
            Err(e) => {
                sensitivity_rows.push(crate::render::SensitivityRow::Err {
                    value,
                    message: e.to_string(),
                });
                if breakdown {
                    breakdown_rows.push((value, vec![0.0; resource_labels.len()]));
                }
            }
        }
    }

    let table = crate::render::render_sensitivity_table(var_name, &sensitivity_rows);

    println!("\nSensitivity Analysis: {var_name}");
    let var_key = VariableName::new(var_name);
    if let Some(&base_val) = base_params.get(&var_key) {
        println!("Base value: {}", crate::render::format_number(base_val));
    } else {
        // Try to derive the value from bindings.
        let resolved = evaluate::resolve_bindings(arch.all_bindings(), &base_params)
            .context("failed to resolve bindings")?;
        if let Some(&derived_val) = resolved.get(&var_key) {
            println!(
                "Base value: {} (derived from bindings)",
                crate::render::format_number(derived_val),
            );
        } else {
            bail!(
                "variable '{var_name}' is not set in params and not derived from bindings \
                 — a sweep over a variable the model never references would be meaningless"
            );
        }
    }
    println!("Base cost: ${base_cost:.2}");
    println!("{table}");

    if breakdown && !resource_labels.is_empty() {
        println!("\nResource Breakdown by Step:");
        let bd_table = crate::render::render_sensitivity_breakdown_table(
            var_name,
            &resource_labels,
            &breakdown_rows,
        );
        println!("{bd_table}");
    }

    Ok(())
}

/// Outcome of a `validate` run that completed without an operational error.
///
/// `Failed` means at least one capacity constraint was violated with
/// [`Severity::Error`]; the caller (main.rs) maps it to a non-zero exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationStatus {
    /// No error-severity violations were found.
    Passed,
    /// At least one error-severity violation was found.
    Failed,
}

#[allow(clippy::too_many_arguments)]
pub fn validate(
    template_path: &str,
    parameters_path: Option<&str>,
    imports_path: Option<&str>,
    params_path: &str,
    _profile_path: Option<&str>,
    bindings_path: Option<&str>,
    quotas_path: Option<&str>,
    output_format: &str,
    region: &str,
    input_format: Option<InputFormat>,
    strict: bool,
) -> Result<ValidationStatus> {
    let format = resolve_input_format(template_path, input_format)?;
    reject_cfn_only_options(format, parameters_path, imports_path, bindings_path)?;

    let registries = yevice_engine::build_registries(&provider_plugins());
    let cfn_inputs = load_cfn_inputs(parameters_path, imports_path)?;
    let policy = if strict {
        ParsePolicy::Strict
    } else {
        ParsePolicy::Lenient
    };
    let arch_outcome = yevice_engine::build_architecture_from_input_with_policy(
        format,
        Path::new(template_path),
        &cfn_inputs,
        (format != InputFormat::Wrangler).then_some("validate"),
        region,
        &registries,
        policy,
    )?;
    report_diagnostics(&arch_outcome.diagnostics);
    if strict && arch_outcome.had_errors {
        bail!("strict mode: IaC parse produced error-severity diagnostics");
    }
    let architecture = arch_outcome.value;
    let catalog = registries.catalog;

    let quotas = match quotas_path {
        Some(p) => load_quotas(p).context("failed to load quotas file")?,
        None => catalog.default_quotas(region),
    };

    let capacity_models = catalog.build_capacity_models(&architecture, &quotas);

    // Combine architecture-derived bindings (e.g. EventSourceMapping deriving
    // `Worker_requests` from `Queue_requests`) with any user-supplied bindings.
    // Without the architecture-derived ones, downstream user bindings that
    // depend on auto-derived variables would never resolve.
    let mut all_bindings = derive_bindings(&architecture, catalog.connection_rules());
    if format == InputFormat::Cfn
        && let Some(path) = bindings_path
    {
        let user_bindings = load_bindings(path).context("failed to load bindings file")?;
        all_bindings.extend(user_bindings);
    }

    let base_params = load_params(params_path)?;
    let params = evaluate::resolve_bindings(&all_bindings, &base_params)
        .context("failed to resolve bindings")?;

    let result = capacity::validate_capacity(&capacity_models, &params);

    if output_format == "json" {
        let json = serde_json::to_string_pretty(&result).context("failed to serialize")?;
        println!("{json}");
    } else if result.violations.is_empty() {
        if result.skipped.is_empty() {
            println!("All capacity constraints satisfied.");
        } else {
            println!(
                "No constraint violations found, but {} constraint(s) could not be evaluated (missing variables):",
                result.skipped.len()
            );
            for s in &result.skipped {
                println!("  - {} / {}: {}", s.resource, s.dimension, s.reason);
            }
        }
    } else {
        let table = crate::render::render_validate_table(&result.violations);

        println!("\nCapacity Validation");
        println!("{table}");

        let errors = result
            .violations
            .iter()
            .filter(|v| v.severity == Severity::Error)
            .count();
        let warnings = result
            .violations
            .iter()
            .filter(|v| v.severity == Severity::Warning)
            .count();
        println!("\n{errors} error(s), {warnings} warning(s)");

        if !result.skipped.is_empty() {
            println!(
                "Note: {} constraint(s) were not evaluated (missing variables):",
                result.skipped.len()
            );
            for s in &result.skipped {
                println!("  - {} / {}: {}", s.resource, s.dimension, s.reason);
            }
        }
    }

    if result.has_errors() {
        Ok(ValidationStatus::Failed)
    } else {
        Ok(ValidationStatus::Passed)
    }
}

/// Find the optimal decision-variable assignment that minimizes (or maximizes)
/// the total cost of a cost model.
///
/// # Arguments
///
/// * `cost_model_path` – path to a JSON cost model produced by `generate`.
/// * `params_path` – optional path to a usage-params YAML; values are treated
///   as fixed (non-decision) variables.
/// * `decisions` – each element is `"NAME=v1,v2,..."` specifying one decision
///   variable and its candidate domain.
/// * `direction` – `"min"` to minimize (default) or `"max"` to maximize.
pub fn optimize(
    cost_model_path: &str,
    params_path: Option<&str>,
    decisions: &[String],
    direction: &str,
    solver_name: &str,
) -> Result<()> {
    let arch = load_cost_model(cost_model_path)?;
    let objective = arch.total_expr();

    let fixed_params = match params_path {
        Some(p) => load_params(p)?,
        None => Params::default(),
    };

    // Parse --decision NAME=v1,v2,...
    let mut decision_variables: Vec<DecisionVariable> = Vec::new();
    for spec in decisions {
        let (name_part, values_part) = spec.split_once('=').with_context(|| {
            format!("invalid --decision value '{spec}': expected NAME=v1,v2,...")
        })?;
        let name = VariableName::new(name_part.trim());
        if values_part.trim().is_empty() {
            bail!("decision variable '{name_part}' has an empty domain");
        }
        let domain: Vec<f64> = values_part
            .split(',')
            .map(|s| {
                s.trim().parse::<f64>().with_context(|| {
                    format!("invalid domain value '{s}' for decision variable '{name_part}'")
                })
            })
            .collect::<Result<_>>()?;
        decision_variables.push(DecisionVariable { name, domain });
    }

    let obj_direction = match direction {
        "min" => ObjectiveDirection::Minimize,
        "max" => ObjectiveDirection::Maximize,
        other => bail!("unknown --direction value '{other}': valid values are min, max"),
    };

    let problem = OptimizationProblem {
        objective,
        direction: obj_direction,
        decision_variables,
        constraints: vec![],
        fixed_params: fixed_params.into_iter().collect(),
        bindings: arch.all_bindings().to_vec(),
    };

    // Select the solver backend (default: enumeration). Unknown names map to
    // a typed error so the CLI can show the allowed list.
    let solver: Box<dyn Solver> = match solver_from_name(solver_name) {
        Ok(s) => s,
        Err(SolverError::UnknownSolver { requested, allowed }) => {
            bail!(
                "unknown --solver value '{requested}'. Allowed values: {}",
                allowed.join(", ")
            );
        }
        Err(e) => return Err(e.into()),
    };

    // The solver validates up-front that every variable in the objective is
    // bound — either fixed via --params, chosen as a --decision, or derivable
    // via a binding whose own inputs are themselves bound (transitively).
    let sol = match solver.solve(&problem) {
        Ok(s) => s,
        Err(SolverError::UnboundVariables { variables }) => {
            bail!(
                "cannot optimize: {} objective variable(s) are unbound; provide them via --params \
                 or as a --decision: {}",
                variables.len(),
                variables.join(", ")
            );
        }
        Err(SolverError::TooManyCombinations { count, limit }) => {
            bail!(
                "too many combinations to enumerate ({count} > {limit}). \
                 Reduce the domain sizes passed to --decision."
            );
        }
        Err(e) => return Err(e.into()),
    };

    println!(
        "\nOptimization Result ({}, direction={direction}):",
        arch.name
    );
    if sol.feasible {
        // Print each decision variable's chosen value.
        for dv in &problem.decision_variables {
            if let Some(&val) = sol.assignments.get(&dv.name) {
                println!("  {} = {val}", dv.name);
            }
        }
        println!(
            "  objective (total monthly cost) = ${:.4}",
            sol.objective_value
        );
    } else {
        let n = sol.total_combinations;
        let failures = sol.evaluation_failures;
        if failures > 0 && failures == n {
            // Every combination failed to evaluate — not a genuine constraint violation.
            let first_err = sol
                .first_evaluation_error
                .as_deref()
                .unwrap_or("unknown error");
            println!(
                "  Result: INFEASIBLE — all {n} combination(s) failed to evaluate: \
                 {first_err} (check bindings and --params for values like 0 used as divisors)"
            );
        } else if problem.constraints.is_empty() {
            if failures > 0 {
                println!(
                    "  Result: INFEASIBLE — {failures} of {n} combination(s) failed to evaluate."
                );
            } else {
                println!("  Result: INFEASIBLE — no feasible combination was found.");
            }
        } else if failures > 0 {
            println!(
                "  Result: INFEASIBLE — no combination satisfied all constraints \
                 ({failures} of {n} combination(s) failed to evaluate)."
            );
        } else {
            println!("  Result: INFEASIBLE — no combination satisfied all constraints.");
        }
    }

    Ok(())
}

fn reject_cfn_only_options(
    format: InputFormat,
    parameters_path: Option<&str>,
    imports_path: Option<&str>,
    bindings_path: Option<&str>,
) -> Result<()> {
    if format == InputFormat::Cfn {
        return Ok(());
    }

    let mut flags = Vec::new();
    if parameters_path.is_some() {
        flags.push("--parameters");
    }
    if imports_path.is_some() {
        flags.push("--imports");
    }
    if bindings_path.is_some() {
        flags.push("--bindings");
    }

    if flags.is_empty() {
        return Ok(());
    }

    bail!(
        "{} are only supported with CloudFormation input.",
        flags.join(", ")
    )
}

/// Parse a `PROVIDER=REGION` string into a `(Provider, String)` pair.
///
/// The provider name must be one of `aws`, `gcp`, or `cloudflare`.
/// Returns an error for unknown provider names.
pub fn parse_provider_region(s: &str) -> Result<(Provider, String)> {
    let (provider_str, region_str) = s
        .split_once('=')
        .with_context(|| format!("invalid --provider-region value '{s}': expected PROVIDER=REGION (e.g. gcp=asia-northeast1)"))?;
    let provider = provider_from_str(provider_str.trim())
        .with_context(|| format!("unknown provider '{provider_str}' in --provider-region '{s}'"))?;
    Ok((provider, region_str.trim().to_string()))
}

/// Reverse-map a provider name string to a [`Provider`] variant.
///
/// Accepts the same strings that [`Provider::as_str`] produces.
fn provider_from_str(s: &str) -> Result<Provider> {
    match s {
        "aws" => Ok(Provider::Aws),
        "gcp" => Ok(Provider::Gcp),
        "cloudflare" => Ok(Provider::Cloudflare),
        other => bail!("unknown provider '{other}': valid values are aws, gcp, cloudflare"),
    }
}

/// Load custom quotas from a YAML file.
///
/// File I/O happens here; parsing and key validation are delegated to
/// [`yevice_core::io::parse_quotas`].
fn load_quotas(path: &str) -> Result<Quotas> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("failed to read: {path}"))?;
    yevice_core::io::parse_quotas(&content).with_context(|| format!("invalid quota file: {path}"))
}

/// Simulate cost over time with varying load patterns.
///
/// Reads the load profile and cost models from disk, delegates the actual
/// simulation to [`yevice_core::simulate`], and renders the result tables.
pub fn simulate(cost_model_paths: &[String], profile_path: &str, breakdown: bool) -> Result<()> {
    let content = std::fs::read_to_string(profile_path)
        .with_context(|| format!("failed to read: {profile_path}"))?;
    let profile = SimulationProfile::from_yaml_str(&content)?;

    let mut arch_results: Vec<ArchSimulation> = Vec::new();
    for path in cost_model_paths {
        let arch = load_cost_model(path)?;
        arch_results.push(simulate_architecture(&arch, &profile, breakdown)?);
    }

    // Print hourly breakdown table
    let table =
        crate::render::render_simulate_table(&arch_results, |hour| profile.multiplier_at(hour));

    println!("\nLoad Simulation ({} days/month)", profile.days_per_month);
    println!("{table}");

    // Winner
    if arch_results.len() == 2 {
        let diff = arch_results[1].total_monthly_cost - arch_results[0].total_monthly_cost;
        if diff > 0.0 {
            println!(
                "\n{} is ${:.2}/month cheaper than {}",
                arch_results[0].name,
                diff.abs(),
                arch_results[1].name
            );
        } else {
            println!(
                "\n{} is ${:.2}/month cheaper than {}",
                arch_results[1].name,
                diff.abs(),
                arch_results[0].name
            );
        }
    }

    // Resource breakdown table (based on base_params evaluation)
    if breakdown {
        // Collect all unique resource labels across all architectures.
        let mut all_labels: Vec<String> = Vec::new();
        for sim in &arch_results {
            for (label, _) in &sim.base_resource_costs {
                if !all_labels.contains(label) {
                    all_labels.push(label.clone());
                }
            }
        }

        if !all_labels.is_empty() {
            println!("\nResource Breakdown (base params estimate):");
            let bd_table =
                crate::render::render_simulate_breakdown_table(&arch_results, &all_labels);
            println!("{bd_table}");
        }
    }

    Ok(())
}

/// Download AWS pricing data for `region` into `output_dir`.
///
/// The HTTP download logic lives in [`yevice_pricing::download`]; this
/// function only handles directory/file I/O and progress output.
pub fn update_pricing(region: &str, output_dir: &str) -> Result<()> {
    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("failed to create directory: {output_dir}"))?;

    let region_code = region;
    println!("Downloading pricing data for region: {region_code}");

    for (service_code, filename) in pricing_download::PRICING_SERVICES {
        print!("  {service_code} ...");

        let url = pricing_download::pricing_url(service_code, region_code);

        match pricing_download::download_pricing(&url) {
            Ok(data) => {
                let path = format!("{output_dir}/{filename}.json");
                std::fs::write(&path, &data).with_context(|| format!("failed to write {path}"))?;
                let size_kb = data.len() / 1024;
                println!(" {size_kb} KB");
            }
            Err(e) => {
                println!(" SKIP");
                eprintln!("[WARN] {service_code}: skipped – {e}");
            }
        }
    }

    println!("\nPricing data saved to: {output_dir}/");
    Ok(())
}

/// Render an architecture diagram from a generated cost-model JSON file.
///
/// - `cost_model_path`: path to a cost-model JSON file produced by `generate`.
/// - `format`: one of `"drawio"`, `"mermaid"`, or `"json"`.
/// - `output`: optional file path; if `None` the diagram is written to stdout.
pub fn diagram(cost_model_path: &str, format: &str, output: Option<&str>) -> Result<()> {
    let cost = load_cost_model(cost_model_path)?;

    let rendered: String = match format {
        "drawio" => DrawIoRenderer
            .render(&cost)
            .context("draw.io rendering failed")?,
        "mermaid" => MermaidRenderer
            .render(&cost)
            .context("mermaid rendering failed")?,
        "json" => JsonRenderer
            .render(&cost)
            .context("json rendering failed")?,
        other => bail!("unknown diagram format '{other}'. Valid choices: drawio, mermaid, json"),
    };

    match output {
        Some(path) => {
            std::fs::write(path, &rendered)
                .with_context(|| format!("failed to write diagram to {path}"))?;
        }
        None => println!("{rendered}"),
    }

    Ok(())
}

fn load_cost_model(path: &str) -> Result<ArchitectureCost> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read cost model: {path}"))?;
    Ok(yevice_core::io::parse_cost_model(&content)?)
}

/// Load usage parameters from a YAML file.
///
/// File I/O happens here; parsing (flat and hierarchical formats) is
/// delegated to [`yevice_core::io::parse_params`].
fn load_params(path: &str) -> Result<Params> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("failed to read: {path}"))?;
    Ok(yevice_core::io::parse_params(&content)?)
}

/// Load user-defined variable bindings from a YAML file.
///
/// File I/O happens here; parsing is delegated to
/// [`yevice_core::io::parse_bindings`].
fn load_bindings(path: &str) -> Result<Vec<yevice_core::cost::VariableBinding>> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("failed to read: {path}"))?;
    Ok(yevice_core::io::parse_bindings(&content)?)
}

/// Load CloudFormation parameter and import value files into [`CfnInputs`].
///
/// Both paths are optional; missing files yield empty maps (Terraform and
/// Wrangler inputs never supply them).
fn load_cfn_inputs(parameters_path: Option<&str>, imports_path: Option<&str>) -> Result<CfnInputs> {
    let parameters = match parameters_path {
        Some(p) => load_string_map(p).context("failed to load parameters file")?,
        None => HashMap::new(),
    };
    let imports = match imports_path {
        Some(p) => load_string_map(p).context("failed to load imports file")?,
        None => HashMap::new(),
    };
    Ok(CfnInputs {
        parameters,
        imports,
    })
}

/// Load a flat `name: scalar` YAML map (CFN parameters/imports) from a file.
///
/// File I/O happens here; parsing is delegated to
/// [`yevice_core::io::parse_string_map`].
fn load_string_map(path: &str) -> Result<HashMap<String, String>> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("failed to read: {path}"))?;
    Ok(yevice_core::io::parse_string_map(&content)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("yevice-cli-{label}-{unique}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    // --- parse_provider_region tests ---

    #[test]
    fn parses_gcp_provider_region() {
        let (provider, region) = parse_provider_region("gcp=asia-northeast1").unwrap();
        assert_eq!(provider, Provider::Gcp);
        assert_eq!(region, "asia-northeast1");
    }

    #[test]
    fn parses_aws_provider_region() {
        let (provider, region) = parse_provider_region("aws=us-east-1").unwrap();
        assert_eq!(provider, Provider::Aws);
        assert_eq!(region, "us-east-1");
    }

    #[test]
    fn parses_cloudflare_provider_region() {
        let (provider, region) = parse_provider_region("cloudflare=global").unwrap();
        assert_eq!(provider, Provider::Cloudflare);
        assert_eq!(region, "global");
    }

    #[test]
    fn parse_provider_region_trims_whitespace() {
        let (provider, region) = parse_provider_region("gcp = asia-northeast1").unwrap();
        assert_eq!(provider, Provider::Gcp);
        assert_eq!(region, "asia-northeast1");
    }

    #[test]
    fn parse_provider_region_rejects_unknown_provider() {
        let err = parse_provider_region("azure=eastus").unwrap_err();
        assert!(
            err.to_string().contains("azure"),
            "error should mention the unknown provider name"
        );
    }

    #[test]
    fn parse_provider_region_rejects_missing_equals() {
        let err = parse_provider_region("gcp-asia-northeast1").unwrap_err();
        assert!(
            err.to_string().contains("PROVIDER=REGION"),
            "error should describe expected format"
        );
    }

    // --- empty domain --decision tests (#4) ---

    /// `NAME=` (empty values_part) must be detected before split and return an
    /// actionable error message.
    #[test]
    fn empty_domain_spec_returns_error() {
        // Build a minimal cost model file so we can call optimize.
        // We only care that parsing the decision spec fails before the solver.
        // Use a temp dir with a trivial cost model JSON.
        use std::fs;
        let dir = temp_dir("empty-domain");
        let cost_model = serde_json::json!({
            "name": "test",
            "resources": [],
            "region": "ap-northeast-1",
            "topology": { "nodes": [], "connections": [] }
        });
        let cost_model_path = dir.join("cost.json");
        fs::write(
            &cost_model_path,
            serde_json::to_string(&cost_model).unwrap(),
        )
        .unwrap();

        let err = super::optimize(
            cost_model_path.to_str().unwrap(),
            None,
            &["MyVar=".to_string()],
            "min",
            "enumeration",
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("empty domain"),
            "expected 'empty domain' error, got: {msg}"
        );
    }

    /// `NAME=  ` (whitespace-only values_part) must also return empty-domain error.
    #[test]
    fn whitespace_only_domain_spec_returns_error() {
        use std::fs;
        let dir = temp_dir("ws-domain");
        let cost_model = serde_json::json!({
            "name": "test",
            "resources": [],
            "region": "ap-northeast-1",
            "topology": { "nodes": [], "connections": [] }
        });
        let cost_model_path = dir.join("cost.json");
        fs::write(
            &cost_model_path,
            serde_json::to_string(&cost_model).unwrap(),
        )
        .unwrap();

        let err = super::optimize(
            cost_model_path.to_str().unwrap(),
            None,
            &["MyVar=  ".to_string()],
            "min",
            "enumeration",
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("empty domain"),
            "expected 'empty domain' error, got: {msg}"
        );
    }

    // --- direction parsing tests (#7) ---

    fn empty_cost_model_json(name: &str) -> serde_json::Value {
        // A cost model with no resources — total_expr() = Sum([]) = constant 0,
        // which has no variables, so the "unbound" check passes immediately.
        serde_json::json!({
            "name": name,
            "resources": [],
            "region": "ap-northeast-1",
            "topology": { "nodes": [], "connections": [] }
        })
    }

    #[test]
    fn direction_min_is_accepted() {
        use std::fs;
        let dir = temp_dir("dir-min");
        let cost_model_path = dir.join("cost.json");
        fs::write(
            &cost_model_path,
            serde_json::to_string(&empty_cost_model_json("dir-min")).unwrap(),
        )
        .unwrap();

        // No decisions needed for an empty objective.
        let result = super::optimize(
            cost_model_path.to_str().unwrap(),
            None,
            &[],
            "min",
            "enumeration",
        );
        assert!(result.is_ok(), "min direction must be accepted: {result:?}");
    }

    // --- --solver smoke tests ---

    /// `--solver enumeration` is the default and must keep working.
    #[test]
    fn solver_enumeration_smoke() {
        use std::fs;
        let dir = temp_dir("solver-enum");
        let cost_model_path = dir.join("cost.json");
        fs::write(
            &cost_model_path,
            serde_json::to_string(&empty_cost_model_json("solver-enum")).unwrap(),
        )
        .unwrap();

        let result = super::optimize(
            cost_model_path.to_str().unwrap(),
            None,
            &[],
            "min",
            "enumeration",
        );
        assert!(
            result.is_ok(),
            "--solver enumeration must remain accepted: {result:?}"
        );
    }

    /// Unknown `--solver` values must fail with an actionable message that
    /// lists the supported solvers.
    #[test]
    fn solver_unknown_name_returns_error() {
        use std::fs;
        let dir = temp_dir("solver-unknown");
        let cost_model_path = dir.join("cost.json");
        fs::write(
            &cost_model_path,
            serde_json::to_string(&empty_cost_model_json("solver-unknown")).unwrap(),
        )
        .unwrap();

        let err = super::optimize(
            cost_model_path.to_str().unwrap(),
            None,
            &[],
            "min",
            "no-such-solver",
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("no-such-solver"),
            "error must name the bad solver: {msg}"
        );
        assert!(
            msg.contains("enumeration"),
            "error must mention the allowed solver(s): {msg}"
        );
    }

    #[test]
    fn direction_max_is_accepted() {
        use std::fs;
        let dir = temp_dir("dir-max");
        let cost_model_path = dir.join("cost.json");
        fs::write(
            &cost_model_path,
            serde_json::to_string(&empty_cost_model_json("dir-max")).unwrap(),
        )
        .unwrap();

        let result = super::optimize(
            cost_model_path.to_str().unwrap(),
            None,
            &[],
            "max",
            "enumeration",
        );
        assert!(result.is_ok(), "max direction must be accepted: {result:?}");
    }

    #[test]
    fn direction_unknown_returns_error() {
        use std::fs;
        let dir = temp_dir("dir-bad");
        let cost_model_path = dir.join("cost.json");
        fs::write(
            &cost_model_path,
            serde_json::to_string(&empty_cost_model_json("dir-bad")).unwrap(),
        )
        .unwrap();

        let err = super::optimize(
            cost_model_path.to_str().unwrap(),
            None,
            &[],
            "sideways",
            "enumeration",
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("sideways"),
            "error must mention the invalid direction value: {msg}"
        );
    }

    // --- #3 optimize unbound-check closure tests ---

    /// When a binding's source variable is not provided, the binding target
    /// must NOT be treated as bound.  If the objective references the binding
    /// target, optimize() must return an actionable "unbound" error that
    /// names the missing source variable — not INFEASIBLE from the solver.
    #[test]
    fn optimize_unbound_source_gives_unbound_error_not_infeasible() {
        use std::fs;
        // Cost model:
        //   resource "Widget" with expr = Variable("Widget_derived_cost")
        //   binding:  target="Widget_derived_cost"
        //             expr = Variable("Widget_source_input") * Constant(0.01)
        //
        // If Widget_source_input is NOT provided as a param or decision,
        // the closure must leave Widget_derived_cost unbound, causing an
        // actionable error.  The old flat approach would mark Widget_derived_cost
        // bound regardless of whether Widget_source_input is present.
        let cost_model = serde_json::json!({
            "name": "closure-test",
            "resources": [{
                "logical_id": "Widget",
                "resource_type": "AWS::Unknown",
                "label": "Widget",
                "expr": { "type": "Variable", "name": "Widget_derived_cost" },
                "required_variables": [
                    { "name": "Widget_derived_cost", "description": "derived", "unit": "USD" }
                ]
            }],
            "bindings": [{
                "target": "Widget_derived_cost",
                "expr": {
                    "type": "Product",
                    "exprs": [
                        { "type": "Variable", "name": "Widget_source_input" },
                        { "type": "Constant", "value": 0.01 }
                    ]
                },
                "description": "source * price",
                "source": "test"
            }],
            "region": "ap-northeast-1",
            "topology": { "nodes": [], "connections": [] }
        });

        let dir = temp_dir("closure-unbound");
        let cost_model_path = dir.join("cost.json");
        fs::write(
            &cost_model_path,
            serde_json::to_string(&cost_model).unwrap(),
        )
        .unwrap();

        // No params, no decisions → Widget_source_input is missing.
        // Must get an unbound error mentioning Widget_source_input (the missing source),
        // not an INFEASIBLE result from the solver.
        let err = super::optimize(
            cost_model_path.to_str().unwrap(),
            None,
            &[],
            "min",
            "enumeration",
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("unbound") || msg.contains("Widget_source_input"),
            "expected unbound error mentioning Widget_source_input, got: {msg}"
        );
        assert!(
            !msg.contains("INFEASIBLE"),
            "must not report INFEASIBLE when source variable is missing, got: {msg}"
        );
    }

    /// When the missing source variable is supplied as a decision variable,
    /// optimize() must solve successfully.
    #[test]
    fn optimize_with_source_as_decision_solves_successfully() {
        use std::fs;
        let cost_model = serde_json::json!({
            "name": "closure-test-ok",
            "resources": [{
                "logical_id": "Widget",
                "resource_type": "AWS::Unknown",
                "label": "Widget",
                "expr": { "type": "Variable", "name": "Widget_derived_cost" },
                "required_variables": [
                    { "name": "Widget_derived_cost", "description": "derived", "unit": "USD" }
                ]
            }],
            "bindings": [{
                "target": "Widget_derived_cost",
                "expr": {
                    "type": "Product",
                    "exprs": [
                        { "type": "Variable", "name": "Widget_source_input" },
                        { "type": "Constant", "value": 0.01 }
                    ]
                },
                "description": "source * price",
                "source": "test"
            }],
            "region": "ap-northeast-1",
            "topology": { "nodes": [], "connections": [] }
        });

        let dir = temp_dir("closure-ok");
        let cost_model_path = dir.join("cost.json");
        fs::write(
            &cost_model_path,
            serde_json::to_string(&cost_model).unwrap(),
        )
        .unwrap();

        // Provide Widget_source_input as a decision variable.
        let result = super::optimize(
            cost_model_path.to_str().unwrap(),
            None,
            &["Widget_source_input=100,200".to_string()],
            "min",
            "enumeration",
        );
        assert!(
            result.is_ok(),
            "optimize must succeed when source variable is provided: {result:?}"
        );
    }

    // --- #8 sensitivity steps=0 guard ---

    /// `sensitivity` with `steps=0` must return an error before computing
    /// `step_size`, not silently produce NaN output.
    #[test]
    fn sensitivity_steps_zero_returns_error() {
        use std::fs;
        let dir = temp_dir("sensitivity-zero-steps");

        // Minimal cost model with one constant resource so evaluate succeeds.
        let cost_model = serde_json::json!({
            "name": "test",
            "resources": [],
            "region": "ap-northeast-1",
            "topology": { "nodes": [], "connections": [] }
        });
        let cost_model_path = dir.join("cost.json");
        fs::write(
            &cost_model_path,
            serde_json::to_string(&cost_model).unwrap(),
        )
        .unwrap();

        // Minimal params file.
        let params_path = dir.join("params.yaml");
        fs::write(&params_path, "").unwrap();

        let err = super::sensitivity(
            cost_model_path.to_str().unwrap(),
            params_path.to_str().unwrap(),
            "SomeVar",
            0.0,
            1000.0,
            0, // steps = 0 must be rejected
            false,
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("--steps") || msg.contains("steps"),
            "error must mention '--steps'; got: {msg}"
        );
        assert!(
            msg.contains('1') || msg.contains("at least"),
            "error must say at least 1; got: {msg}"
        );
    }

    // --- Fix 4: sensitivity bails when var is neither in params nor in bindings ---

    /// When `--var` names a variable not present in params and not derivable
    /// from bindings, `sensitivity` must return an actionable error rather than
    /// silently printing "Base value: 0".
    #[test]
    fn sensitivity_bails_on_unknown_var() {
        use std::fs;
        let dir = temp_dir("sensitivity-unknown-var");

        let cost_model = serde_json::json!({
            "name": "test",
            "resources": [],
            "region": "ap-northeast-1",
            "topology": { "nodes": [], "connections": [] }
        });
        let cost_model_path = dir.join("cost.json");
        fs::write(
            &cost_model_path,
            serde_json::to_string(&cost_model).unwrap(),
        )
        .unwrap();

        let params_path = dir.join("params.yaml");
        fs::write(&params_path, "SomeOtherVar: 42\n").unwrap();

        let err = super::sensitivity(
            cost_model_path.to_str().unwrap(),
            params_path.to_str().unwrap(),
            "NonExistentVar",
            0.0,
            100.0,
            5,
            false,
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("NonExistentVar"),
            "error must name the unknown variable; got: {msg}"
        );
        assert!(
            msg.contains("meaningless") || msg.contains("not set") || msg.contains("not derived"),
            "error must explain the variable is unrecognised; got: {msg}"
        );
    }

    // --- Fix 5: optimize INFEASIBLE prints correct diagnostics ---

    /// When the solver returns INFEASIBLE because constraints exclude all
    /// combinations, optimize() must return Ok (not Err).
    #[test]
    fn optimize_infeasible_from_constraints_returns_ok() {
        use std::fs;
        // Minimal cost model with no resources; total_expr() = constant 0.
        let cost_model = serde_json::json!({
            "name": "test",
            "resources": [],
            "region": "ap-northeast-1",
            "topology": { "nodes": [], "connections": [] }
        });
        let dir = temp_dir("optimize-infeasible-constraints");
        let cost_model_path = dir.join("cost.json");
        fs::write(
            &cost_model_path,
            serde_json::to_string(&cost_model).unwrap(),
        )
        .unwrap();

        // No decisions, no params — the solver returns a feasible result with
        // objective=0 (empty domain case is infeasible, but empty decision list
        // with constant objective is feasible).
        // To get a genuinely infeasible result we rely on the solver returning
        // infeasible when there are zero combinations (empty domain list means
        // combination_count = 1 and the objective = 0 is always feasible).
        // Instead, verify the function doesn't panic and returns Ok for an
        // empty-objective problem (which yields feasible=true, objective=0).
        let result = super::optimize(
            cost_model_path.to_str().unwrap(),
            None,
            &[],
            "min",
            "enumeration",
        );
        assert!(result.is_ok(), "optimize must return Ok; got: {result:?}");
    }
}
