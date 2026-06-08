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
pub struct CognitoUserPoolSpec {}

pub struct CognitoService;

impl Service for CognitoService {
    type Spec = CognitoUserPoolSpec;

    fn id(&self) -> &'static str {
        "aws.cognito"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        _spec: &CognitoUserPoolSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let free_mau = pricing.lookup_f64(&Sku::new("aws.cognito.free_tier_mau"))?;
        let tier1 = pricing.lookup_f64(&Sku::new("aws.cognito.tier1_price"))?;
        let tier2 = pricing.lookup_f64(&Sku::new("aws.cognito.tier2_price"))?;
        let tier3 = pricing.lookup_f64(&Sku::new("aws.cognito.tier3_price"))?;

        let cost = Expr::tiered(
            vec![
                Tier {
                    upper_limit: Some(free_mau),
                    unit_price: 0.0,
                },
                Tier {
                    upper_limit: Some(100_000.0),
                    unit_price: tier1,
                },
                Tier {
                    upper_limit: Some(1_000_000.0),
                    unit_price: tier2,
                },
                Tier {
                    upper_limit: None,
                    unit_price: tier3,
                },
            ],
            Expr::variable(id.var("mau")),
        );

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("Cognito: {id}"),
            expr: cost.clone(),
            components: vec![CostComponent {
                name: "MAU".into(),
                expr: cost,
            }],
            required_variables: vec![VariableInfo::new(
                id,
                "mau",
                "Monthly Active Users",
                "users",
            )],
        })
    }
}
