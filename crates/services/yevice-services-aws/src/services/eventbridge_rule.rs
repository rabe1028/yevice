use serde::{Deserialize, Serialize};
use yevice_core::{
    cost::{ResourceCost, VariableInfo},
    expr::Expr,
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EventBridgeRuleSpec {}

pub struct EventBridgeRuleService;

impl Service for EventBridgeRuleService {
    type Spec = EventBridgeRuleSpec;

    fn id(&self) -> &'static str {
        "aws.eventbridge_rule"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        _spec: &EventBridgeRuleSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let event_price = pricing.lookup_f64(&Sku::new(
            "aws.eventbridge_rule.custom_event_price_per_million",
        ))?;
        let cost = Expr::linear(
            event_price / 1_000_000.0,
            Expr::variable(id.var("events")),
            0.0,
        );

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("EventBridge Rule: {id}"),
            expr: cost.clone(),
            components: vec![],
            required_variables: vec![VariableInfo::new(
                id,
                "events",
                "Custom events per month",
                "events",
            )],
        })
    }
}
