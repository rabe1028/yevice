use serde::{Deserialize, Serialize};
use yevice_core::{
    cost::{ResourceCost, VariableInfo},
    expr::{Expr, Tier},
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EventBridgeSchedulerSpec {}

pub struct EventBridgeSchedulerService;

impl Service for EventBridgeSchedulerService {
    type Spec = EventBridgeSchedulerSpec;

    fn id(&self) -> &'static str {
        "aws.eventbridge_scheduler"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        _spec: &EventBridgeSchedulerSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let invocation_price =
            pricing.lookup_f64(&Sku::new("aws.eventbridge_scheduler.invocation_price"))?;
        let free_tier =
            pricing.lookup_f64(&Sku::new("aws.eventbridge_scheduler.free_tier_invocations"))?;

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("EventBridge Scheduler: {id}"),
            expr: Expr::tiered(
                vec![
                    Tier {
                        upper_limit: Some(free_tier),
                        unit_price: 0.0,
                    },
                    Tier {
                        upper_limit: None,
                        unit_price: invocation_price,
                    },
                ],
                Expr::variable(id.var("invocations")),
            ),
            components: vec![],
            required_variables: vec![VariableInfo::new(
                id,
                "invocations",
                "Invocations per month",
                "invocations",
            )],
        })
    }
}
