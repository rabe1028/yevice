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
pub struct SecretsManagerSpec {}

pub struct SecretsManagerService;

impl Service for SecretsManagerService {
    type Spec = SecretsManagerSpec;

    fn id(&self) -> &'static str {
        "aws.secrets_manager"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        _spec: &SecretsManagerSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let secret_price =
            pricing.lookup_f64(&Sku::new("aws.secrets_manager.secret_month_price"))?;
        let api_price =
            pricing.lookup_f64(&Sku::new("aws.secrets_manager.api_call_price_per_10k"))?;

        let secret_cost = Expr::constant(secret_price);
        let api_cost = Expr::linear(
            api_price / 10_000.0,
            Expr::variable(id.var("api_calls")),
            0.0,
        );

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("Secrets Manager: {id}"),
            expr: Expr::sum(vec![secret_cost.clone(), api_cost.clone()]),
            components: vec![
                CostComponent {
                    name: "Secret storage".into(),
                    expr: secret_cost,
                },
                CostComponent {
                    name: "API calls".into(),
                    expr: api_cost,
                },
            ],
            required_variables: vec![VariableInfo::new(
                id,
                "api_calls",
                "API calls per month",
                "calls",
            )],
        })
    }
}
