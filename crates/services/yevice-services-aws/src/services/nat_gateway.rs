use serde::{Deserialize, Serialize};
use yevice_core::{
    HOURS_PER_MONTH,
    cost::{CostComponent, ResourceCost, VariableInfo},
    expr::Expr,
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NatGatewaySpec {}

pub struct NatGatewayService;

impl Service for NatGatewayService {
    type Spec = NatGatewaySpec;

    fn id(&self) -> &'static str {
        "aws.nat_gateway"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        _spec: &NatGatewaySpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let hourly_price = pricing.lookup_f64(&Sku::new("aws.nat_gateway.hourly_price"))?;
        let data_price =
            pricing.lookup_f64(&Sku::new("aws.nat_gateway.data_processing_price_per_gb"))?;

        let nat_fixed = Expr::constant(hourly_price * HOURS_PER_MONTH);
        let nat_data = Expr::linear(data_price, Expr::variable(id.var("data_processed_gb")), 0.0);

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("NAT Gateway: {id}"),
            expr: Expr::sum(vec![nat_fixed.clone(), nat_data.clone()]),
            components: vec![
                CostComponent {
                    name: "Gateway Hours".into(),
                    expr: nat_fixed,

                    currency: None,
                },
                CostComponent {
                    name: "Data Processing".into(),
                    expr: nat_data,

                    currency: None,
                },
            ],
            required_variables: vec![VariableInfo::new(
                id,
                "data_processed_gb",
                "Data processed per month",
                "GB",
            )],

            currency: Some("USD".into()),
        })
    }
}
