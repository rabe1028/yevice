use serde::{Deserialize, Serialize};
use yevice_core::{
    cost::{CostComponent, ResourceCost, VariableInfo},
    expr::Expr,
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum GlueDpuType {
    #[default]
    Standard,
    Flex,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlueJobSpec {
    #[serde(default)]
    pub dpu_type: GlueDpuType,
    pub max_dpu: Option<f64>,
}

pub struct GlueService;

impl Service for GlueService {
    type Spec = GlueJobSpec;

    fn id(&self) -> &'static str {
        "aws.glue"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &GlueJobSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let standard_price = pricing.lookup_f64(&Sku::new("aws.glue.standard_dpu_hour_price"))?;
        let flex_price = pricing.lookup_f64(&Sku::new("aws.glue.flex_dpu_hour_price"))?;

        let dpu_price = match spec.dpu_type {
            GlueDpuType::Standard => standard_price,
            GlueDpuType::Flex => flex_price,
        };
        let max_dpu = spec.max_dpu.unwrap_or(10.0);

        let cost = Expr::linear(
            dpu_price * max_dpu,
            Expr::variable(id.var("job_hours")),
            0.0,
        );

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("Glue Job: {id}"),
            expr: cost.clone(),
            components: vec![CostComponent {
                name: "DPU-Hours".into(),
                expr: cost,
            }],
            required_variables: vec![VariableInfo::new(
                id,
                "job_hours",
                "Job hours per month",
                "hours",
            )],
        })
    }
}
