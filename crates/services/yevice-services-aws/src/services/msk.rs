use serde::{Deserialize, Serialize};
use yevice_core::{
    cost::{CostComponent, ResourceCost, VariableInfo},
    expr::Expr,
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MskClusterSpec {
    pub broker_instance_type: String,
    pub broker_count: Option<f64>,
}

pub struct MskService;

const HOURS_PER_MONTH: f64 = 730.0;

impl Service for MskService {
    type Spec = MskClusterSpec;

    fn id(&self) -> &'static str {
        "aws.msk"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &MskClusterSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let sku = Sku::dynamic(format!("aws.msk.{}", spec.broker_instance_type));
        let storage_sku = Sku::dynamic(format!("aws.msk_storage.{}", spec.broker_instance_type));
        let hourly_price = pricing.lookup_f64(&sku)?;
        let storage_price = pricing.lookup_f64(&storage_sku)?;

        let broker_expr = match spec.broker_count {
            Some(n) => Expr::constant(n),
            None => Expr::variable(id.var("broker_count")),
        };

        let instance_cost = Expr::linear(hourly_price * HOURS_PER_MONTH, broker_expr, 0.0);
        let storage_cost = Expr::linear(storage_price, Expr::variable(id.var("storage_gb")), 0.0);

        let mut vars = vec![];
        if spec.broker_count.is_none() {
            vars.push(VariableInfo::new(
                id,
                "broker_count",
                "Number of brokers",
                "brokers",
            ));
        }
        vars.push(VariableInfo::new(
            id,
            "storage_gb",
            "Total broker storage",
            "GB",
        ));

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("MSK: {id} ({})", spec.broker_instance_type),
            expr: Expr::sum(vec![instance_cost.clone(), storage_cost.clone()]),
            components: vec![
                CostComponent {
                    name: format!("Brokers ({})", spec.broker_instance_type),
                    expr: instance_cost,
                },
                CostComponent {
                    name: "Storage".into(),
                    expr: storage_cost,
                },
            ],
            required_variables: vars,
        })
    }
}

const _: f64 = HOURS_PER_MONTH;
