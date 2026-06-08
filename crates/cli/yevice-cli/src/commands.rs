use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use yevice_core::optimize::{DecisionVariable, ObjectiveDirection, OptimizationProblem};
use yevice_solver::{EnumerationSolver, Solver, SolverError};
use yevice_output::{ArchitectureRenderer, DrawIoRenderer, JsonRenderer, MermaidRenderer};
use comfy_table::{Cell, Color, Table, presets::UTF8_FULL};

use yevice_cfn::convert as cfn_convert;
use yevice_cfn::parser;
use yevice_core::bindings::{BindingsFile, derive_bindings, to_variable_bindings};
use yevice_core::capacity::{self, Quotas, Severity};
use yevice_core::cost::ArchitectureCost;
use yevice_core::evaluate::{self, Params, evaluate_architecture};
use yevice_core::resource::{Architecture, Provider};
use yevice_core::schema::{generate_usage_schema, generate_usage_template};
use yevice_core::types::VariableName;
use yevice_service_api::{
    CfnAdapterRegistry, MultiProviderCatalog, ProviderPlugin, Registration, ServiceCatalog,
    TfAdapterRegistry,
};
use yevice_services_aws::AwsPlugin;
use yevice_services_gcp::GcpPlugin;
use yevice_wrangler::CloudflarePlugin;

const DEFAULT_ARCHITECTURE_NAME: &str = "default";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputFormat {
    Cfn,
    Tf,
    Wrangler,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TfProvider {
    Aws,
    Gcp,
    Mixed,
    Unknown,
}

struct ParsedInput {
    architecture: Architecture,
}

fn resolve_cfn_template(
    template_path: &str,
    parameters_path: Option<&str>,
    imports_path: Option<&str>,
) -> Result<yevice_cfn::parser::CfnTemplate> {
    let template = parser::parse_template(Path::new(template_path))
        .context("failed to parse CloudFormation template")?;

    let param_values = match parameters_path {
        Some(p) => load_string_map(p).context("failed to load parameters file")?,
        None => HashMap::new(),
    };

    let import_values = match imports_path {
        Some(p) => load_string_map(p).context("failed to load imports file")?,
        None => HashMap::new(),
    };

    let resolved = parser::resolve_template(&template, &param_values, &import_values)
        .context("failed to resolve template")?;

    Ok(yevice_cfn::parser::CfnTemplate {
        parameters: template.parameters,
        mappings: template.mappings,
        conditions: template.conditions,
        resources: resolved,
    })
}

/// Returns the list of all provider plugins. Both `build_registries` and
/// `build_pricing_resolver` iterate over this single source of truth.
fn provider_plugins() -> Vec<Box<dyn ProviderPlugin>> {
    vec![
        Box::new(AwsPlugin),
        Box::new(GcpPlugin),
        Box::new(CloudflarePlugin),
    ]
}

fn build_registries() -> (ServiceCatalog, CfnAdapterRegistry, TfAdapterRegistry) {
    let mut catalog = ServiceCatalog::new();
    let mut cfn_adapters = CfnAdapterRegistry::new();
    let mut tf_adapters = TfAdapterRegistry::new();
    for plugin in provider_plugins() {
        let mut reg = Registration {
            catalog: &mut catalog,
            cfn_adapters: &mut cfn_adapters,
            tf_adapters: &mut tf_adapters,
        };
        plugin.register(&mut reg);
    }
    (catalog, cfn_adapters, tf_adapters)
}

pub fn generate(
    template_path: &str,
    parameters_path: Option<&str>,
    imports_path: Option<&str>,
    bindings_path: Option<&str>,
    name: &str,
    output_path: &str,
    region: &str,
    input_format: Option<InputFormat>,
    strict: bool,
    list_price: bool,
) -> Result<()> {
    let format = resolve_input_format(template_path, input_format)?;
    reject_cfn_only_options(format, parameters_path, imports_path, bindings_path)?;

    let (catalog, cfn_adapters, tf_adapters) = build_registries();
    let parsed_input = build_architecture_from_input(
        format,
        template_path,
        parameters_path,
        imports_path,
        Some(name),
        region,
        &cfn_adapters,
        &tf_adapters,
    )?;
    let pricing = build_pricing_resolver(&parsed_input.architecture, region, list_price);
    let mut cost_model = catalog
        .build_cost_model(&parsed_input.architecture, &pricing, strict)
        .context("failed to build cost model")?;

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
    Ok(())
}

pub fn evaluate(cost_model_path: &str, params_path: &str, breakdown: bool) -> Result<()> {
    let arch = load_cost_model(cost_model_path)?;
    let params = load_params(params_path)?;

    let result = evaluate_architecture(&arch, &params).context("failed to evaluate cost model")?;

    println!("\n{}: Monthly Cost Estimate", result.name);

    if breakdown {
        let mut table = Table::new();
        table.load_preset(UTF8_FULL);
        table.set_header(vec!["Resource / Component", "Monthly Cost (USD)"]);

        for r in &result.resources {
            table.add_row(vec![
                Cell::new(&r.label).fg(Color::Cyan),
                Cell::new(format!("${:.2}", r.monthly_cost)).fg(Color::Cyan),
            ]);
            for (name, cost) in &r.component_costs {
                table.add_row(vec![
                    Cell::new(format!("  └─ {name}")),
                    Cell::new(format!("${cost:.4}")),
                ]);
            }
        }

        table.add_row(vec![
            Cell::new("TOTAL").fg(Color::Green),
            Cell::new(format!("${:.2}", result.total_monthly_cost)).fg(Color::Green),
        ]);

        println!("{table}");
    } else {
        let mut table = Table::new();
        table.load_preset(UTF8_FULL);
        table.set_header(vec!["Resource", "Type", "Monthly Cost (USD)"]);

        for r in &result.resources {
            table.add_row(vec![
                Cell::new(&r.label),
                Cell::new(&r.resource_type),
                Cell::new(format!("${:.2}", r.monthly_cost)),
            ]);
        }

        table.add_row(vec![
            Cell::new("TOTAL").fg(Color::Green),
            Cell::new(""),
            Cell::new(format!("${:.2}", result.total_monthly_cost)).fg(Color::Green),
        ]);

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

    // Summary table
    let mut summary = Table::new();
    summary.load_preset(UTF8_FULL);

    let mut header = vec![Cell::new("Architecture")];
    for r in &results {
        header.push(Cell::new(&r.name));
    }
    summary.set_header(header);

    // Total row
    let mut total_row = vec![Cell::new("Total Monthly Cost").fg(Color::Green)];
    for r in &results {
        total_row.push(Cell::new(format!("${:.2}", r.total_monthly_cost)));
    }
    summary.add_row(total_row);

    // Collect all unique resource types across architectures
    let mut all_labels: Vec<String> = Vec::new();
    for r in &results {
        for res in &r.resources {
            if !all_labels.contains(&res.label) {
                all_labels.push(res.label.clone());
            }
        }
    }

    for label in &all_labels {
        let mut row = vec![Cell::new(label)];
        for r in &results {
            let cost = r
                .resources
                .iter()
                .find(|res| &res.label == label)
                .map_or_else(
                    || "-".to_string(),
                    |res| format!("${:.2}", res.monthly_cost),
                );
            row.push(Cell::new(cost));
        }
        summary.add_row(row);

        // Breakdown: component rows
        if breakdown {
            // Collect all component names across all architectures for this resource label
            let mut all_component_names: Vec<String> = Vec::new();
            for r in &results {
                if let Some(res) = r.resources.iter().find(|res| &res.label == label) {
                    for (name, _) in &res.component_costs {
                        if !all_component_names.contains(name) {
                            all_component_names.push(name.clone());
                        }
                    }
                }
            }
            for comp_name in &all_component_names {
                let mut comp_row = vec![Cell::new(format!("  └─ {comp_name}"))];
                for r in &results {
                    let comp_cost = r
                        .resources
                        .iter()
                        .find(|res| &res.label == label)
                        .and_then(|res| res.component_costs.iter().find(|(n, _)| n == comp_name))
                        .map_or_else(|| "-".to_string(), |(_, v)| format!("${v:.4}"));
                    comp_row.push(Cell::new(comp_cost));
                }
                summary.add_row(comp_row);
            }
        }
    }

    // Difference row (if exactly 2 architectures)
    if results.len() == 2 {
        let diff = results[1].total_monthly_cost - results[0].total_monthly_cost;
        let diff_str = if diff >= 0.0 {
            format!("+${diff:.2}")
        } else {
            format!("-${:.2}", diff.abs())
        };
        let color = if diff > 0.0 { Color::Red } else { Color::Green };
        summary.add_row(vec![
            Cell::new("Difference"),
            Cell::new("-"),
            Cell::new(diff_str).fg(color),
        ]);
    }

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

    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(vec![
        Cell::new(var_name),
        Cell::new("Total Monthly Cost"),
        Cell::new("Delta from Base"),
    ]);

    // When breakdown is true, collect step results for a second table.
    let mut breakdown_rows: Vec<(f64, Vec<f64>)> = Vec::new();

    for i in 0..=steps {
        let value = min + step_size * i as f64;
        let mut params = base_params.clone();
        params.insert(VariableName::new(var_name), value);

        match evaluate_architecture(&arch, &params) {
            Ok(result) => {
                let delta = result.total_monthly_cost - base_cost;
                let delta_str = if delta >= 0.0 {
                    format!("+${delta:.2}")
                } else {
                    format!("-${:.2}", delta.abs())
                };
                let color = if delta > 0.0 {
                    Color::Red
                } else if delta < 0.0 {
                    Color::Green
                } else {
                    Color::White
                };
                table.add_row(vec![
                    Cell::new(format_number(value)),
                    Cell::new(format!("${:.2}", result.total_monthly_cost)),
                    Cell::new(delta_str).fg(color),
                ]);

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
                table.add_row(vec![
                    Cell::new(format_number(value)),
                    Cell::new(format!("ERROR: {e}")),
                    Cell::new("-"),
                ]);
                if breakdown {
                    breakdown_rows.push((value, vec![0.0; resource_labels.len()]));
                }
            }
        }
    }

    println!("\nSensitivity Analysis: {var_name}");
    println!(
        "Base value: {}",
        format_number(
            base_params
                .get(&VariableName::new(var_name))
                .copied()
                .unwrap_or(0.0),
        )
    );
    println!("Base cost: ${base_cost:.2}");
    println!("{table}");

    if breakdown && !resource_labels.is_empty() {
        println!("\nResource Breakdown by Step:");
        let mut bd_table = Table::new();
        bd_table.load_preset(UTF8_FULL);
        let mut bd_header = vec![Cell::new(var_name)];
        for label in &resource_labels {
            bd_header.push(Cell::new(label));
        }
        bd_table.set_header(bd_header);

        for (value, costs) in &breakdown_rows {
            let mut row = vec![Cell::new(format_number(*value))];
            for cost in costs {
                row.push(Cell::new(format!("${cost:.2}")));
            }
            bd_table.add_row(row);
        }

        println!("{bd_table}");
    }

    Ok(())
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
) -> Result<()> {
    let format = resolve_input_format(template_path, input_format)?;
    reject_cfn_only_options(format, parameters_path, imports_path, bindings_path)?;

    let (catalog, cfn_adapters, tf_adapters) = build_registries();
    let parsed_input = build_architecture_from_input(
        format,
        template_path,
        parameters_path,
        imports_path,
        (format != InputFormat::Wrangler).then_some("validate"),
        region,
        &cfn_adapters,
        &tf_adapters,
    )?;
    let architecture = parsed_input.architecture;

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
        let mut table = Table::new();
        table.load_preset(UTF8_FULL);
        table.set_header(vec![
            Cell::new("Severity"),
            Cell::new("Resource"),
            Cell::new("Constraint"),
            Cell::new("Required"),
            Cell::new("Limit"),
            Cell::new("Message"),
        ]);

        for v in &result.violations {
            let color = match v.severity {
                Severity::Error => Color::Red,
                Severity::Warning => Color::Yellow,
                Severity::Info => Color::Cyan,
            };
            table.add_row(vec![
                Cell::new(v.severity.to_string()).fg(color),
                Cell::new(v.resource.to_string()),
                Cell::new(&v.dimension),
                Cell::new(format!("{:.0}", v.required)),
                Cell::new(format!("{:.0}", v.limit)),
                Cell::new(&v.message),
            ]);
        }

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
        std::process::exit(1);
    }

    Ok(())
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
pub fn optimize(
    cost_model_path: &str,
    params_path: Option<&str>,
    decisions: &[String],
) -> Result<()> {
    let arch = load_cost_model(cost_model_path)?;
    let objective = arch.total_expr();

    let fixed_params = match params_path {
        Some(p) => load_params(p)?,
        None => Params::new(),
    };

    // Parse --decision NAME=v1,v2,...
    let mut decision_variables: Vec<DecisionVariable> = Vec::new();
    for spec in decisions {
        let (name_part, values_part) = spec
            .split_once('=')
            .with_context(|| format!("invalid --decision value '{spec}': expected NAME=v1,v2,..."))?;
        let name = VariableName::new(name_part.trim());
        let domain: Vec<f64> = values_part
            .split(',')
            .map(|s| {
                s.trim()
                    .parse::<f64>()
                    .with_context(|| {
                        format!(
                            "invalid domain value '{s}' for decision variable '{name_part}'"
                        )
                    })
            })
            .collect::<Result<_>>()?;
        if domain.is_empty() {
            bail!("decision variable '{name_part}' has an empty domain");
        }
        decision_variables.push(DecisionVariable { name, domain });
    }

    // Every variable in the objective must be bound — either fixed via --params
    // or chosen as a --decision. Otherwise the objective cannot be evaluated and
    // the solver would (misleadingly) report INFEASIBLE. Surface it as a clear
    // error instead, listing exactly what is missing.
    let mut bound: std::collections::HashSet<VariableName> = fixed_params.keys().cloned().collect();
    for dv in &decision_variables {
        bound.insert(dv.name.clone());
    }
    let unbound: Vec<String> = objective
        .variables()
        .into_iter()
        .filter(|v| !bound.contains(v))
        .map(|v| v.to_string())
        .collect();
    if !unbound.is_empty() {
        bail!(
            "cannot optimize: {} objective variable(s) are unbound; provide them via --params \
             or as a --decision: {}",
            unbound.len(),
            unbound.join(", ")
        );
    }

    let problem = OptimizationProblem {
        objective,
        direction: ObjectiveDirection::Minimize,
        decision_variables,
        constraints: vec![],
        fixed_params: fixed_params.into_iter().collect(),
    };

    let sol = match EnumerationSolver.solve(&problem) {
        Ok(s) => s,
        Err(SolverError::TooManyCombinations { count, limit }) => {
            bail!(
                "too many combinations to enumerate ({count} > {limit}). \
                 Reduce the domain sizes passed to --decision."
            );
        }
        Err(e) => return Err(e.into()),
    };

    println!("\nOptimization Result ({}):", arch.name);
    if sol.feasible {
        // Print each decision variable's chosen value.
        for dv in &problem.decision_variables {
            if let Some(&val) = sol.assignments.get(&dv.name) {
                println!("  {} = {val}", dv.name);
            }
        }
        println!("  objective (total monthly cost) = ${:.4}", sol.objective_value);
    } else {
        println!("  Result: INFEASIBLE — no combination satisfied all constraints.");
    }

    Ok(())
}

fn resolve_input_format(
    template_path: &str,
    requested: Option<InputFormat>,
) -> Result<InputFormat> {
    match requested {
        Some(format) => Ok(format),
        None => detect_input_format(Path::new(template_path)),
    }
}

fn detect_input_format(path: &Path) -> Result<InputFormat> {
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
        _ => bail!(
            "could not detect input format for {}. Pass --input-format <cfn|tf|wrangler>.",
            path.display()
        ),
    }
}

fn directory_contains_tf_files(path: &Path) -> Result<bool> {
    if !path.is_dir() {
        return Ok(false);
    }

    for entry in std::fs::read_dir(path)
        .with_context(|| format!("failed to read directory: {}", path.display()))?
    {
        let entry = entry?;
        if entry.path().extension().is_some_and(|ext| ext == "tf") {
            return Ok(true);
        }
    }

    Ok(false)
}

fn find_wrangler_config(path: &Path) -> Option<PathBuf> {
    ["wrangler.toml", "wrangler.jsonc"]
        .into_iter()
        .map(|name| path.join(name))
        .find(|candidate| candidate.is_file())
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

fn build_architecture_from_input(
    format: InputFormat,
    template_path: &str,
    parameters_path: Option<&str>,
    imports_path: Option<&str>,
    architecture_name: Option<&str>,
    region: &str,
    cfn_adapters: &CfnAdapterRegistry,
    tf_adapters: &TfAdapterRegistry,
) -> Result<ParsedInput> {
    match format {
        InputFormat::Cfn => {
            let resolved_template =
                resolve_cfn_template(template_path, parameters_path, imports_path)?;
            let architecture = cfn_convert::build_architecture(
                architecture_name.unwrap_or(DEFAULT_ARCHITECTURE_NAME),
                region,
                &resolved_template,
                cfn_adapters,
            );
            Ok(ParsedInput { architecture })
        }
        InputFormat::Tf => {
            let resolved = resolve_tf_input(Path::new(template_path))?;
            let tf_provider = detect_tf_provider(&resolved);
            if tf_provider == TfProvider::Unknown {
                bail!(
                    "unable to detect a supported Terraform provider from {template_path}. Expected resources with aws_ or google_ prefixes."
                );
            }

            let architecture = yevice_tf::build_architecture(
                architecture_name.unwrap_or(DEFAULT_ARCHITECTURE_NAME),
                region,
                &resolved,
                tf_adapters,
            );
            Ok(ParsedInput { architecture })
        }
        InputFormat::Wrangler => {
            let wrangler_path = resolve_wrangler_input_path(Path::new(template_path))?;
            let mut architecture =
                yevice_wrangler::parse_wrangler(&wrangler_path).with_context(|| {
                    format!(
                        "failed to parse Wrangler config: {}",
                        wrangler_path.display()
                    )
                })?;

            if let Some(name_override) =
                architecture_name.filter(|name| *name != DEFAULT_ARCHITECTURE_NAME)
            {
                architecture.name = name_override.to_string();
            }

            Ok(ParsedInput { architecture })
        }
    }
}

fn resolve_tf_input(path: &Path) -> Result<yevice_tf::ResolvedConfig> {
    let config_dir = terraform_config_dir(path)?;
    let config = yevice_tf::parse_tf_dir(config_dir)
        .with_context(|| format!("failed to parse Terraform config: {}", config_dir.display()))?;

    // Variable files in Terraform precedence (lowest first; later wins):
    //   1. `terraform.tfvars`
    //   2. `*.auto.tfvars` (alphabetical)
    //   3. an explicit `*.tfvars` passed as the input path
    // (HCL `.tfvars` only; the JSON variants are not parsed here.)
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
        let vars = yevice_tf::parse_tfvars(file)
            .with_context(|| format!("failed to parse Terraform variables: {}", file.display()))?;
        merged.extend(vars);
    }
    let tfvars = if merged.is_empty() {
        None
    } else {
        Some(merged)
    };

    yevice_tf::resolve_config(config, tfvars).context("failed to resolve Terraform configuration")
}

fn terraform_config_dir(path: &Path) -> Result<&Path> {
    if path.is_dir() {
        return Ok(path);
    }

    path.parent()
        .context("failed to determine Terraform configuration directory")
}

fn resolve_wrangler_input_path(path: &Path) -> Result<PathBuf> {
    if path.is_dir() {
        return find_wrangler_config(path).with_context(|| {
            format!(
                "failed to locate wrangler.toml or wrangler.jsonc in {}",
                path.display()
            )
        });
    }

    Ok(path.to_path_buf())
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

/// Build a per-provider pricing resolver from the providers present in `arch`.
///
/// Iterates over all registered provider plugins and, for each provider that
/// appears in the architecture, inserts the plugin's pricing catalog into the
/// resolver. The `Provider::Other` variant has no corresponding plugin and is
/// handled separately with a [`yevice_pricing::NoopCatalog`].
fn build_pricing_resolver(
    arch: &Architecture,
    region: &str,
    list_price: bool,
) -> MultiProviderCatalog {
    let mut resolver = MultiProviderCatalog::new();

    for plugin in provider_plugins() {
        if arch.has_provider(plugin.provider()) {
            resolver.insert(plugin.provider(), plugin.pricing_catalog(region, list_price));
        }
    }

    // Provider::Other has no dedicated plugin; use a no-op catalog.
    if arch.has_provider(Provider::Other) {
        resolver.insert(
            Provider::Other,
            Box::new(yevice_pricing::NoopCatalog),
        );
    }

    resolver
}

fn load_quotas(path: &str) -> Result<Quotas> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("failed to read: {path}"))?;
    let quotas: Quotas =
        serde_yaml_ng::from_str(&content).context("failed to parse quotas file")?;
    Ok(quotas)
}

fn format_number(n: f64) -> String {
    if n >= 1_000_000.0 {
        format!("{:.1}M", n / 1_000_000.0)
    } else if n >= 1_000.0 {
        format!("{:.1}K", n / 1_000.0)
    } else {
        format!("{n:.2}")
    }
}

/// Simulate cost over time with varying load patterns.
///
/// Load profile format:
/// ```yaml
/// base_params:
///   IngestFunction_avg_duration_ms: 200
///   ...
/// hourly_pattern:
///   - hour: 0
///     multiplier: 0.1
///   - hour: 9
///     multiplier: 1.0
///   - hour: 12
///     multiplier: 0.8
///   - hour: 18
///     multiplier: 1.5  # peak
///   - hour: 22
///     multiplier: 0.3
/// scaled_variables:
///   - DataStream_put_records
///   - IngestFunction_requests
/// days_per_month: 30
/// ```
pub fn simulate(cost_model_paths: &[String], profile_path: &str, breakdown: bool) -> Result<()> {
    let profile = load_simulation_profile(profile_path)?;

    // (arch_name, total_monthly, hourly_costs, base_resource_costs)
    // base_resource_costs: Vec<(label, monthly_cost)> evaluated at base_params (for breakdown)
    let mut arch_results: Vec<(String, f64, Vec<(u32, f64)>, Vec<(String, f64)>)> = Vec::new();

    for path in cost_model_paths {
        let arch = load_cost_model(path)?;
        let arch_name = arch.name.to_string();
        let mut total_monthly = 0.0;
        let mut hourly_costs = Vec::new();

        // Evaluate at base_params once for the resource breakdown display.
        let base_resource_costs = if breakdown {
            match evaluate_architecture(&arch, &profile.base_params) {
                Ok(result) => result
                    .resources
                    .into_iter()
                    .map(|r| (r.label, r.monthly_cost))
                    .collect(),
                Err(_) => Vec::new(),
            }
        } else {
            Vec::new()
        };

        for hour in 0..24 {
            let multiplier = profile.multiplier_at(hour);
            let mut params = profile.base_params.clone();

            // Scale designated variables by the hourly multiplier
            for var_name in &profile.scaled_variables {
                if let Some(base_val) = params.get(var_name).copied() {
                    // Convert monthly value to hourly, apply multiplier
                    let hourly_val = base_val / (24.0 * profile.days_per_month) * multiplier;
                    params.insert(var_name.clone(), hourly_val);
                }
            }

            // Evaluate cost for this hour's load (as monthly equivalent at this rate)
            match evaluate_architecture(&arch, &params) {
                Ok(result) => {
                    // Scale hourly slice: this hour's rate * hours_in_month_at_this_hour
                    let hours_at_rate = profile.days_per_month;
                    let hour_cost =
                        result.total_monthly_cost * hours_at_rate / (24.0 * profile.days_per_month);
                    total_monthly += hour_cost;
                    hourly_costs.push((hour, result.total_monthly_cost));
                }
                Err(e) => {
                    tracing::warn!(hour, error = %e, "failed to evaluate hour, skipping");
                    hourly_costs.push((hour, 0.0));
                }
            }
        }

        arch_results.push((arch_name, total_monthly, hourly_costs, base_resource_costs));
    }

    // Print hourly breakdown table
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);

    let mut header = vec![Cell::new("Hour"), Cell::new("Multiplier")];
    for (name, _, _, _) in &arch_results {
        header.push(Cell::new(format!("{name} (rate/mo)")));
    }
    table.set_header(header);

    for hour in 0..24 {
        let mult = profile.multiplier_at(hour);
        let mut row = vec![
            Cell::new(format!("{hour:02}:00")),
            Cell::new(format!("{mult:.2}x")),
        ];
        for (_, _, hourly, _) in &arch_results {
            let cost = hourly
                .iter()
                .find(|(h, _)| *h == hour)
                .map_or(0.0, |(_, c)| *c);
            row.push(Cell::new(format!("${cost:.2}")));
        }
        table.add_row(row);
    }

    // Total row
    let mut total_row = vec![Cell::new("MONTHLY TOTAL").fg(Color::Green), Cell::new("")];
    for (_, total, _, _) in &arch_results {
        total_row.push(Cell::new(format!("${total:.2}")).fg(Color::Green));
    }
    table.add_row(total_row);

    println!("\nLoad Simulation ({} days/month)", profile.days_per_month);
    println!("{table}");

    // Winner
    if arch_results.len() == 2 {
        let diff = arch_results[1].1 - arch_results[0].1;
        if diff > 0.0 {
            println!(
                "\n{} is ${:.2}/month cheaper than {}",
                arch_results[0].0,
                diff.abs(),
                arch_results[1].0
            );
        } else {
            println!(
                "\n{} is ${:.2}/month cheaper than {}",
                arch_results[1].0,
                diff.abs(),
                arch_results[0].0
            );
        }
    }

    // Resource breakdown table (based on base_params evaluation)
    if breakdown {
        // Collect all unique resource labels across all architectures.
        let mut all_labels: Vec<String> = Vec::new();
        for (_, _, _, res_costs) in &arch_results {
            for (label, _) in res_costs {
                if !all_labels.contains(label) {
                    all_labels.push(label.clone());
                }
            }
        }

        if !all_labels.is_empty() {
            println!("\nResource Breakdown (base params estimate):");
            let mut bd_table = Table::new();
            bd_table.load_preset(UTF8_FULL);

            let mut bd_header = vec![Cell::new("Resource")];
            for (name, _, _, _) in &arch_results {
                bd_header.push(Cell::new(name));
            }
            bd_table.set_header(bd_header);

            for label in &all_labels {
                let mut row = vec![Cell::new(label)];
                for (_, _, _, res_costs) in &arch_results {
                    let cost = res_costs
                        .iter()
                        .find(|(l, _)| l == label)
                        .map_or_else(|| "-".to_string(), |(_, c)| format!("${c:.2}"));
                    row.push(Cell::new(cost));
                }
                bd_table.add_row(row);
            }

            println!("{bd_table}");
        }
    }

    Ok(())
}

#[derive(Debug)]
struct SimulationProfile {
    base_params: Params,
    hourly_pattern: Vec<(u32, f64)>,
    scaled_variables: Vec<VariableName>,
    days_per_month: f64,
}

impl SimulationProfile {
    fn multiplier_at(&self, hour: u32) -> f64 {
        // Find the last defined multiplier at or before this hour
        let mut result = self.hourly_pattern.first().map_or(1.0, |(_, m)| *m);
        for (h, m) in &self.hourly_pattern {
            if *h <= hour {
                result = *m;
            }
        }
        result
    }
}

fn load_simulation_profile(path: &str) -> Result<SimulationProfile> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("failed to read: {path}"))?;

    let raw: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&content).context("failed to parse profile")?;
    let map = raw.as_mapping().context("profile must be a mapping")?;

    // Load base_params
    let base_params_val = map
        .get(Value::String("base_params".into()))
        .context("profile must have base_params")?;
    let base_map: HashMap<String, serde_yaml_ng::Value> =
        serde_yaml_ng::from_value(base_params_val.clone())
            .context("failed to parse base_params")?;

    let mut base_params = Params::new();
    for (k, v) in base_map {
        match v {
            serde_yaml_ng::Value::Mapping(sub_map) => {
                for (sub_k, sub_v) in sub_map {
                    let Some(sub_key) = sub_k.as_str() else {
                        tracing::warn!(key = ?sub_k, "non-string key in profile base_params mapping; skipping");
                        continue;
                    };
                    let val = extract_f64(&sub_v)
                        .with_context(|| format!("profile base_param '{k}_{sub_key}': invalid value"))?;
                    base_params.insert(VariableName::new(format!("{k}_{sub_key}")), val);
                }
            }
            _ => {
                let val = extract_f64(&v)
                    .with_context(|| format!("profile base_param '{k}': invalid value"))?;
                base_params.insert(VariableName::new(k), val);
            }
        }
    }

    // Load hourly_pattern
    let pattern_val = map
        .get(Value::String("hourly_pattern".into()))
        .and_then(|v| v.as_sequence())
        .context("profile must have hourly_pattern array")?;

    let mut hourly_pattern: Vec<(u32, f64)> = Vec::new();
    for entry in pattern_val {
        let entry_map = entry
            .as_mapping()
            .context("hourly entry must be a mapping")?;
        let hour = entry_map
            .get(Value::String("hour".into()))
            .and_then(serde_yaml_ng::Value::as_u64)
            .context("hourly entry must have hour")? as u32;
        let multiplier = entry_map
            .get(Value::String("multiplier".into()))
            .and_then(serde_yaml_ng::Value::as_f64)
            .context("hourly entry must have multiplier")?;
        hourly_pattern.push((hour, multiplier));
    }
    hourly_pattern.sort_by_key(|(h, _)| *h);

    // Load scaled_variables
    let scaled = map
        .get(Value::String("scaled_variables".into()))
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(VariableName::new))
                .collect()
        })
        .unwrap_or_default();

    // Days per month
    let days = map
        .get(Value::String("days_per_month".into()))
        .and_then(serde_yaml_ng::Value::as_f64)
        .unwrap_or(30.0);

    Ok(SimulationProfile {
        base_params,
        hourly_pattern,
        scaled_variables: scaled,
        days_per_month: days,
    })
}

use serde_yaml_ng::Value;

/// AWS services to download pricing for.
const PRICING_SERVICES: &[(&str, &str)] = &[
    ("AmazonEC2", "ec2"),
    ("AWSLambda", "lambda"),
    ("AmazonRDS", "rds"),
    ("AmazonS3", "s3"),
    ("AmazonDynamoDB", "dynamodb"),
    ("AmazonECS", "ecs"),
    ("AmazonES", "opensearch"), // OpenSearch uses the old ES pricing code
    ("AmazonKinesis", "kinesis"),
    ("AWSQueueService", "sqs"),
    ("AmazonCloudWatch", "cloudwatch"),
];

pub fn update_pricing(region: &str, output_dir: &str) -> Result<()> {
    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("failed to create directory: {output_dir}"))?;

    let region_code = region;
    println!("Downloading pricing data for region: {region_code}");

    for (service_code, filename) in PRICING_SERVICES {
        print!("  {service_code} ...");

        let url = format!(
            "https://pricing.us-east-1.amazonaws.com/offers/v1.0/aws/{service_code}/current/{region_code}/index.json"
        );

        match download_pricing(&url) {
            Ok(data) => {
                let path = format!("{output_dir}/{filename}.json");
                std::fs::write(&path, &data).with_context(|| format!("failed to write {path}"))?;
                let size_kb = data.len() / 1024;
                println!(" {size_kb} KB");
            }
            Err(e) => {
                println!(" SKIP ({e})");
            }
        }
    }

    println!("\nPricing data saved to: {output_dir}/");
    Ok(())
}

fn download_pricing(url: &str) -> Result<Vec<u8>> {
    let response = ureq::get(url).call().context("HTTP request failed")?;
    let body = response
        .into_body()
        .read_to_vec()
        .context("failed to read response body")?;
    Ok(body)
}

/// Render an architecture diagram from a generated cost-model JSON file.
///
/// - `cost_model_path`: path to a cost-model JSON file produced by `generate`.
/// - `format`: one of `"drawio"`, `"mermaid"`, or `"json"`.
/// - `output`: optional file path; if `None` the diagram is written to stdout.
pub fn diagram(cost_model_path: &str, format: &str, output: Option<&str>) -> Result<()> {
    let cost = load_cost_model(cost_model_path)?;

    let rendered: String = match format {
        "drawio" => DrawIoRenderer.render(&cost).context("draw.io rendering failed")?,
        "mermaid" => MermaidRenderer.render(&cost).context("mermaid rendering failed")?,
        "json" => JsonRenderer.render(&cost).context("json rendering failed")?,
        other => bail!(
            "unknown diagram format '{other}'. Valid choices: drawio, mermaid, json"
        ),
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
    let arch: ArchitectureCost =
        serde_json::from_str(&content).context("failed to parse cost model JSON")?;
    Ok(arch)
}

/// Load usage parameters from a YAML file.
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
fn load_params(path: &str) -> Result<Params> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("failed to read: {path}"))?;

    let map: HashMap<String, serde_yaml_ng::Value> =
        serde_yaml_ng::from_str(&content).context("failed to parse params file")?;

    let mut params = Params::new();
    for (k, v) in map {
        match v {
            // Hierarchical: key is logical ID, value is a mapping of short var names
            serde_yaml_ng::Value::Mapping(sub_map) => {
                for (sub_k, sub_v) in sub_map {
                    let Some(sub_key) = sub_k.as_str() else {
                        tracing::warn!(key = ?sub_k, "non-string key in params mapping; skipping");
                        continue;
                    };
                    let val = extract_f64(&sub_v)
                        .with_context(|| format!("param '{k}_{sub_key}': invalid value"))?;
                    let full_name = format!("{k}_{sub_key}");
                    params.insert(VariableName::new(full_name), val);
                }
            }
            // Flat: key is the full variable name
            _ => {
                let val = extract_f64(&v)
                    .with_context(|| format!("param '{k}': invalid value"))?;
                params.insert(VariableName::new(k), val);
            }
        }
    }

    Ok(params)
}

fn extract_f64(v: &serde_yaml_ng::Value) -> anyhow::Result<f64> {
    match v {
        serde_yaml_ng::Value::Number(n) => n
            .as_f64()
            .ok_or_else(|| anyhow::anyhow!("cannot interpret number {v:?} as f64")),
        serde_yaml_ng::Value::String(s) => s
            .parse::<f64>()
            .with_context(|| format!("cannot interpret string {v:?} as f64")),
        _ => anyhow::bail!("cannot interpret value {v:?} as a number"),
    }
}

fn load_bindings(path: &str) -> Result<Vec<yevice_core::cost::VariableBinding>> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("failed to read: {path}"))?;
    let file: BindingsFile =
        serde_yaml_ng::from_str(&content).context("failed to parse bindings file")?;
    Ok(to_variable_bindings(&file.bindings))
}

fn load_string_map(path: &str) -> Result<HashMap<String, String>> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("failed to read: {path}"))?;

    let map: HashMap<String, serde_yaml_ng::Value> =
        serde_yaml_ng::from_str(&content).context("failed to parse file")?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use yevice_tf::parser::TfResource;

    fn temp_dir(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("yevice-cli-{label}-{unique}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn tf_resource(resource_type: &str) -> TfResource {
        TfResource {
            resource_type: resource_type.to_string(),
            name: "sample".to_string(),
            attrs: HashMap::new(),
            blocks: HashMap::new(),
        }
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
    fn detects_terraform_provider_from_resolved_config() {
        let aws = yevice_tf::ResolvedConfig {
            resources: vec![tf_resource("aws_s3_bucket")],
            vars: HashMap::new(),
            locals: HashMap::new(),
        };
        assert_eq!(detect_tf_provider(&aws), TfProvider::Aws);

        let gcp = yevice_tf::ResolvedConfig {
            resources: vec![tf_resource("google_storage_bucket")],
            vars: HashMap::new(),
            locals: HashMap::new(),
        };
        assert_eq!(detect_tf_provider(&gcp), TfProvider::Gcp);

        let mixed = yevice_tf::ResolvedConfig {
            resources: vec![
                tf_resource("aws_s3_bucket"),
                tf_resource("google_storage_bucket"),
            ],
            vars: HashMap::new(),
            locals: HashMap::new(),
        };
        assert_eq!(detect_tf_provider(&mixed), TfProvider::Mixed);

        let unknown = yevice_tf::ResolvedConfig {
            resources: vec![tf_resource("azurerm_storage_account")],
            vars: HashMap::new(),
            locals: HashMap::new(),
        };
        assert_eq!(detect_tf_provider(&unknown), TfProvider::Unknown);
    }
}
