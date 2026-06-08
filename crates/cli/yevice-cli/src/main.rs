mod commands;
mod render;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use tracing_subscriber::EnvFilter;

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
    },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
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
        } => commands::generate(
            &template,
            parameters.as_deref(),
            imports.as_deref(),
            bindings.as_deref(),
            &name,
            &output,
            &cli.region,
            cli.input_format.to_command_format(),
            cli.strict,
            list_price,
        ),
        Commands::Eval {
            cost_model,
            params,
            breakdown,
        } => commands::evaluate(&cost_model, &params, breakdown),
        Commands::Compare {
            cost_models,
            params,
            breakdown,
        } => commands::compare(&cost_models, &params, breakdown),
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
        } => commands::validate(
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
        ),
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
        } => commands::optimize(&cost_model, params.as_deref(), &decision),
    }
}
