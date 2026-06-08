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
pub enum ApiGatewayType {
    Rest,
    Http,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiGatewaySpec {
    pub api_type: ApiGatewayType,
}

pub struct ApiGatewayService;

impl Service for ApiGatewayService {
    type Spec = ApiGatewaySpec;

    fn id(&self) -> &'static str {
        "aws.api_gateway"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &ApiGatewaySpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let rest_price = pricing.lookup_f64(&Sku::new("aws.api_gateway.rest_api_request_price"))?;
        let http_price = pricing.lookup_f64(&Sku::new("aws.api_gateway.http_api_request_price"))?;
        let free_tier = pricing.lookup_f64(&Sku::new("aws.api_gateway.free_tier_requests"))?;

        let (unit_price, api_type) = match spec.api_type {
            ApiGatewayType::Http => (http_price, "HTTP API"),
            ApiGatewayType::Rest => (rest_price, "REST API"),
        };

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("API Gateway {api_type}: {id}"),
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
                Expr::variable(id.var("api_requests")),
            ),
            components: vec![],
            required_variables: vec![VariableInfo::new(
                id,
                "api_requests",
                "Requests per month",
                "requests",
            )],
        })
    }
}
