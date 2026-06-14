use serde::{Deserialize, Serialize};
use yevice_core::{
    cost::{ResourceCost, VariableInfo},
    expr::{Expr, Tier},
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnsSpec {
    pub fifo: bool,
}

pub struct SnsService;

impl Service for SnsService {
    type Spec = SnsSpec;

    fn id(&self) -> &'static str {
        "aws.sns"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &SnsSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let delivery_price = pricing.lookup_f64(&Sku::new("aws.sns.delivery_price_per_million"))?;
        let free_tier = pricing.lookup_f64(&Sku::new("aws.sns.free_tier_deliveries"))?;

        let queue_type = if spec.fifo { "FIFO" } else { "Standard" };

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("SNS {queue_type}: {id}"),
            expr: Expr::tiered(
                vec![
                    Tier {
                        upper_limit: Some(free_tier),
                        unit_price: 0.0,
                    },
                    Tier {
                        upper_limit: None,
                        unit_price: delivery_price / 1_000_000.0,
                    },
                ],
                Expr::variable(id.var("deliveries")),
            ),
            components: vec![],
            required_variables: vec![VariableInfo::new(
                id,
                "deliveries",
                "Deliveries per month",
                "deliveries",
            )],

            currency: Some("USD".into()),
        })
    }
}
