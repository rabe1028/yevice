mod commands;
mod render;

use std::collections::HashMap;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use tracing_subscriber::EnvFilter;
use yevice_core::resource::Provider;

#[derive(Parser)]
#[command(
    name = "yevice",
    version,
    about = "Infrastructure cost function generator"
)]
struct Cli {
    /// Fail on unsupported resource types or unresolvable intrinsic functions.
    #[arg(long, global = true)]
    strict: bool,

    /// Region for pricing data.
    #[arg(long, global = true, default_value = "ap-northeast-1")]
    region: String,

    /// Input format. Defaults to auto-detection from the template path.
    #[arg(long, global = true, value_enum, default_value_t = CliInputFormat::Auto)]
    input_format: CliInputFormat,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum CliInputFormat {
    Auto,
    Cfn,
    Tf,
    Wrangler,
}

impl CliInputFormat {
    const fn to_command_format(self) -> Option<commands::InputFormat> {
        match self {
            Self::Auto => None,
            Self::Cfn => Some(commands::InputFormat::Cfn),
            Self::Tf => Some(commands::InputFormat::Tf),
            Self::Wrangler => Some(commands::InputFormat::Wrangler),
        }
    }
}

#[derive(Subcommand)]
enum Commands {
    /// Generate cost functions from CloudFormation, Terraform, or Wrangler input.
    Generate {
        /// Path to a CloudFormation file, Terraform file/directory, or Wrangler config.
        #[arg(short, long)]
        template: String,

        /// Path to parameter values file (CloudFormation only, YAML/JSON).
        #[arg(short, long)]
        parameters: Option<String>,

        /// Path to cross-stack import values file (CloudFormation only, YAML/JSON).
        #[arg(short, long)]
        imports: Option<String>,

        /// Path to user-defined variable bindings file (CloudFormation only, YAML).
        #[arg(short, long)]
        bindings: Option<String>,

        /// Architecture name.
        #[arg(short, long, default_value = "default")]
        name: String,

        /// Output file path for the cost model (JSON).
        #[arg(short, long)]
        output: String,

        /// Use list prices: ignore promotional AWS Free Tier allowances.
        /// Matches how AWS's own published estimates (e.g. CDP pages) are
        /// calculated. Product-included allocations are still applied.
        #[arg(long)]
        list_price: bool,

        /// Override the pricing region for a specific provider.
        /// Format: PROVIDER=REGION (e.g. `gcp=asia-northeast1`).
        /// May be repeated for multiple providers.
        /// Providers not listed fall back to --region.
        #[arg(long = "provider-region", value_name = "PROVIDER=REGION")]
        provider_region: Vec<String>,
    },

    /// Evaluate a generated cost model with usage parameters.
    Eval {
        /// Path to cost model file (JSON).
        cost_model: String,

        /// Path to usage parameters file (YAML/JSON).
        #[arg(short, long)]
        params: String,

        /// Show detailed cost breakdown per component.
        #[arg(long)]
        breakdown: bool,

        /// Optional display currency (e.g. `USD`, `JPY`). When unset and the
        /// model is single-currency, the native currency is shown as-is.
        /// When the model is multi-currency, an unset flag prints per-currency
        /// totals and emits a warning. A set flag converts everything to the
        /// target currency using `--exchange-rate` entries; missing rates are
        /// a hard error.
        #[arg(long = "display-currency", value_name = "CODE")]
        display_currency: Option<String>,

        /// Static exchange rate of the form `FROM=TO:RATE` (e.g.
        /// `JPY=USD:0.0067`). May be repeated.
        #[arg(long = "exchange-rate", value_name = "FROM=TO:RATE")]
        exchange_rate: Vec<String>,
    },

    /// Compare multiple cost models.
    Compare {
        /// Paths to cost model files (JSON).
        cost_models: Vec<String>,

        /// Path to usage parameters file (YAML/JSON).
        #[arg(short, long)]
        params: String,

        /// Show detailed cost breakdown per component.
        #[arg(long)]
        breakdown: bool,

        /// Optional display currency. See `eval --display-currency`.
        #[arg(long = "display-currency", value_name = "CODE")]
        display_currency: Option<String>,

        /// Static exchange rate. See `eval --exchange-rate`.
        #[arg(long = "exchange-rate", value_name = "FROM=TO:RATE")]
        exchange_rate: Vec<String>,
    },

    /// Sensitivity analysis: vary a parameter and show cost impact.
    Sensitivity {
        /// Path to cost model file (JSON).
        cost_model: String,

        /// Path to base usage parameters file (YAML/JSON).
        #[arg(short, long)]
        params: String,

        /// Variable to vary (e.g., "`IngestFunction_requests`").
        #[arg(short, long)]
        var: String,

        /// Minimum value.
        #[arg(long)]
        min: f64,

        /// Maximum value.
        #[arg(long)]
        max: f64,

        /// Number of steps.
        #[arg(long, default_value = "10")]
        steps: usize,

        /// Show per-resource cost breakdown in addition to totals.
        #[arg(long)]
        breakdown: bool,
    },

    /// Validate capacity constraints and quota limits.
    Validate {
        /// Path to a CloudFormation file, Terraform file/directory, or Wrangler config.
        #[arg(short, long)]
        template: String,

        /// Path to parameter values file (CloudFormation only, YAML/JSON).
        #[arg(short, long)]
        parameters: Option<String>,

        /// Path to cross-stack import values file (CloudFormation only, YAML/JSON).
        #[arg(short, long)]
        imports: Option<String>,

        /// Path to user-defined variable bindings file (CloudFormation only, YAML).
        #[arg(short, long)]
        bindings: Option<String>,

        /// Path to usage parameters file (YAML/JSON) with peak values.
        #[arg(long)]
        params: String,

        /// Path to load profile for peak auto-derivation (YAML).
        #[arg(long)]
        profile: Option<String>,

        /// Path to custom quotas override file (YAML).
        #[arg(short, long)]
        quotas: Option<String>,

        /// Output format: table (default) or json.
        #[arg(long, default_value = "table")]
        output_format: String,
    },

    /// Simulate cost over time with varying load patterns.
    Simulate {
        /// Paths to cost model files to compare (JSON).
        cost_models: Vec<String>,

        /// Path to load profile file (YAML) with hourly patterns.
        #[arg(short, long)]
        profile: String,

        /// Show per-resource cost breakdown in addition to totals.
        #[arg(long)]
        breakdown: bool,
    },

    /// Download and update AWS pricing data for the specified region.
    UpdatePricing {
        /// Output directory for pricing data files.
        #[arg(short, long, default_value = "pricing-data")]
        output_dir: String,
    },

    /// Render an architecture diagram from a generated cost-model JSON file.
    Diagram {
        /// Path to cost model file (JSON) produced by `generate`.
        cost_model: String,

        /// Diagram output format.
        #[arg(long, default_value = "drawio")]
        format: String,

        /// Output file path. Writes to stdout if omitted.
        #[arg(short, long)]
        output: Option<String>,
    },

    /// Find the optimal variable assignment that minimizes (or maximizes) total cost.
    Optimize {
        /// Path to cost model file (JSON) produced by `generate`.
        cost_model: String,

        /// Path to usage parameters file (YAML/JSON) for fixed (non-decision) variables.
        #[arg(short, long)]
        params: Option<String>,

        /// Decision variable with its candidate domain: NAME=v1,v2,...
        /// May be repeated for multiple decision variables.
        #[arg(long = "decision", value_name = "NAME=v1,v2,...")]
        decision: Vec<String>,

        /// Optimization direction: `min` to minimize cost (default), `max` to maximize.
        #[arg(long = "direction", default_value = "min", value_name = "min|max")]
        direction: String,

        /// Solver backend to use. Currently only `enumeration` is supported;
        /// future backends (e.g. LP/MIP) will plug in here.
        #[arg(long = "solver", default_value = "enumeration", value_name = "NAME")]
        solver: String,
    },
}

/// Parse a list of `PROVIDER=REGION` strings into a `HashMap<Provider, String>`.
///
/// Delegates per-entry parsing to [`commands::parse_provider_region`].
fn parse_provider_regions(specs: &[String]) -> Result<HashMap<Provider, String>> {
    let mut map = HashMap::new();
    for spec in specs {
        let (provider, region) = commands::parse_provider_region(spec)
            .with_context(|| format!("failed to parse --provider-region '{spec}'"))?;
        map.insert(provider, region);
    }
    Ok(map)
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(tracing_subscriber::filter::LevelFilter::WARN.into())
                .from_env_lossy(),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Generate {
            template,
            parameters,
            imports,
            bindings,
            name,
            output,
            list_price,
            provider_region,
        } => {
            let provider_regions = parse_provider_regions(&provider_region)?;
            commands::generate(
                &template,
                parameters.as_deref(),
                imports.as_deref(),
                bindings.as_deref(),
                &name,
                &output,
                &cli.region,
                &provider_regions,
                cli.input_format.to_command_format(),
                cli.strict,
                list_price,
            )
        }
        Commands::Eval {
            cost_model,
            params,
            breakdown,
            display_currency,
            exchange_rate,
        } => commands::evaluate(
            &cost_model,
            &params,
            breakdown,
            display_currency.as_deref(),
            &exchange_rate,
        ),
        Commands::Compare {
            cost_models,
            params,
            breakdown,
            display_currency,
            exchange_rate,
        } => commands::compare(
            &cost_models,
            &params,
            breakdown,
            display_currency.as_deref(),
            &exchange_rate,
        ),
        Commands::Sensitivity {
            cost_model,
            params,
            var,
            min,
            max,
            steps,
            breakdown,
        } => commands::sensitivity(&cost_model, &params, &var, min, max, steps, breakdown),
        Commands::Validate {
            template,
            parameters,
            imports,
            bindings,
            params,
            profile,
            quotas,
            output_format,
        } => {
            let status = commands::validate(
                &template,
                parameters.as_deref(),
                imports.as_deref(),
                &params,
                profile.as_deref(),
                bindings.as_deref(),
                quotas.as_deref(),
                &output_format,
                &cli.region,
                cli.input_format.to_command_format(),
            )?;
            // Constraint violations are a structured outcome, not an error:
            // the command's own Result only covers operational failures.
            // Translate the outcome into the process exit code here, so that
            // std::process::exit never appears outside main.rs.
            match status {
                commands::ValidationStatus::Passed => Ok(()),
                commands::ValidationStatus::Failed => std::process::exit(1),
            }
        }
        Commands::Simulate {
            cost_models,
            profile,
            breakdown,
        } => commands::simulate(&cost_models, &profile, breakdown),
        Commands::UpdatePricing { output_dir } => {
            commands::update_pricing(&cli.region, &output_dir)
        }
        Commands::Diagram {
            cost_model,
            format,
            output,
        } => commands::diagram(&cost_model, &format, output.as_deref()),
        Commands::Optimize {
            cost_model,
            params,
            decision,
            direction,
            solver,
        } => commands::optimize(
            &cost_model,
            params.as_deref(),
            &decision,
            &direction,
            &solver,
        ),
    }
}
