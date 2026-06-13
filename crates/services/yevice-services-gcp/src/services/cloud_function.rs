//! GCP Cloud Functions service implementation.

use serde::{Deserialize, Serialize};
use yevice_core::{
    cost::{CostComponent, Expr, ResourceCost, Tier, VariableInfo, VariableKind},
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::{PriceCatalog, Sku};
use yevice_service_api::{CostError, service::Service};

// Free tier constants
const FREE_TIER_INVOCATIONS: f64 = 2_000_000.0;
const FREE_TIER_GB_SECONDS: f64 = 400_000.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcpCloudFunctionSpec {
    pub memory_mb: f64,
    pub generation: u8,
}

impl Default for GcpCloudFunctionSpec {
    fn default() -> Self {
        Self {
            memory_mb: 256.0,
            generation: 2,
        }
    }
}

pub struct GcpCloudFunctionService;

impl Service for GcpCloudFunctionService {
    type Spec = GcpCloudFunctionSpec;

    fn id(&self) -> &'static str {
        "gcp.cloud_function"
    }

    fn provider(&self) -> Provider {
        Provider::Gcp
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &GcpCloudFunctionSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let inv_per_million =
            pricing.lookup_f64(&Sku::new("gcp.cloud_function.invocation_per_million"))?;
        let gb_second = pricing.lookup_f64(&Sku::new("gcp.cloud_function.gb_second"))?;

        let memory_gb = spec.memory_mb / 1024.0;
        let price_per_invocation = inv_per_million / 1_000_000.0;

        let invocation_cost = Expr::tiered(
            vec![
                Tier {
                    upper_limit: Some(FREE_TIER_INVOCATIONS),
                    unit_price: 0.0,
                },
                Tier {
                    upper_limit: None,
                    unit_price: price_per_invocation,
                },
            ],
            Expr::variable(id.var("monthly_invocations")),
        );

        let gb_seconds_expr = Expr::product(vec![
            Expr::variable(id.var("monthly_invocations")),
            Expr::linear(1.0 / 1000.0, Expr::variable(id.var("avg_duration_ms")), 0.0),
            Expr::constant(memory_gb),
        ]);

        let compute_cost = Expr::tiered(
            vec![
                Tier {
                    upper_limit: Some(FREE_TIER_GB_SECONDS),
                    unit_price: 0.0,
                },
                Tier {
                    upper_limit: None,
                    unit_price: gb_second,
                },
            ],
            gb_seconds_expr,
        );

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!(
                "Cloud Function ({}MB Gen{})",
                spec.memory_mb as u32, spec.generation
            ),
            expr: Expr::sum(vec![invocation_cost.clone(), compute_cost.clone()]),
            components: vec![
                CostComponent {
                    name: "Invocations".into(),
                    expr: invocation_cost,

                    currency: None,
                },
                CostComponent {
                    name: "Compute (GB-seconds)".into(),
                    expr: compute_cost,

                    currency: None,
                },
            ],
            required_variables: vec![
                VariableInfo {
                    name: id.var("monthly_invocations"),
                    description: "Function invocations per month".into(),
                    unit: "count".into(),
                    kind: VariableKind::Usage,
                },
                VariableInfo {
                    name: id.var("avg_duration_ms"),
                    description: "Average execution duration".into(),
                    unit: "ms".into(),
                    kind: VariableKind::Usage,
                },
            ],

            currency: Some("USD".into()),
        })
    }
}
