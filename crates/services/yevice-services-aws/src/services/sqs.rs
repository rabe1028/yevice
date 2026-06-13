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
pub struct SqsSpec {
    pub fifo: bool,
}

pub struct SqsService;

impl Service for SqsService {
    type Spec = SqsSpec;

    fn id(&self) -> &'static str {
        "aws.sqs"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &SqsSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let standard_price = pricing.lookup_f64(&Sku::new("aws.sqs.standard_request_price"))?;
        let fifo_price = pricing.lookup_f64(&Sku::new("aws.sqs.fifo_request_price"))?;
        let free_tier = pricing.lookup_f64(&Sku::new("aws.sqs.free_tier_requests"))?;

        let unit_price = if spec.fifo {
            fifo_price
        } else {
            standard_price
        };
        let queue_type = if spec.fifo { "FIFO" } else { "Standard" };

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("SQS {queue_type}: {id}"),
            expr: Expr::tiered(
                vec![
                    Tier {
                        upper_limit: Some(free_tier),
                        unit_price: 0.0,
                    },
                    Tier {
                        upper_limit: None,
                        unit_price,
                    },
                ],
                Expr::variable(id.var("requests")),
            ),
            components: vec![],
            required_variables: vec![VariableInfo::new(
                id,
                "requests",
                "Requests per month",
                "requests",
            )],

            currency: Some("USD".into()),
        })
    }
}
