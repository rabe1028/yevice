use serde::{Deserialize, Serialize};
use yevice_core::{
    cost::{CostComponent, ResourceCost, VariableInfo},
    expr::{Expr, Tier},
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppSyncSpec {}

pub struct AppSyncService;

impl Service for AppSyncService {
    type Spec = AppSyncSpec;

    fn id(&self) -> &'static str {
        "aws.appsync"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        _spec: &AppSyncSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let op_price = pricing.lookup_f64(&Sku::new("aws.appsync.operation_price_per_million"))?;
        let free_tier = pricing.lookup_f64(&Sku::new("aws.appsync.free_tier_operations"))?;

        let cost = Expr::tiered(
            vec![
                Tier {
                    upper_limit: Some(free_tier),
                    unit_price: 0.0,
                },
                Tier {
                    upper_limit: None,
                    unit_price: op_price / 1_000_000.0,
                },
            ],
            Expr::variable(id.var("operations")),
        );

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("AppSync: {id}"),
            expr: cost.clone(),
            components: vec![CostComponent {
                name: "Operations".into(),
                expr: cost,

                currency: None,
            }],
            required_variables: vec![VariableInfo::new(
                id,
                "operations",
                "Query/mutation operations per month",
                "operations",
            )],

            currency: Some("USD".into()),
        })
    }
}
