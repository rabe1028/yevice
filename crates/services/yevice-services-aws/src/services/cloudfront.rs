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
pub struct CloudFrontSpec {}

pub struct CloudFrontService;

impl Service for CloudFrontService {
    type Spec = CloudFrontSpec;

    fn id(&self) -> &'static str {
        "aws.cloudfront"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        _spec: &CloudFrontSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let req_price_10k =
            pricing.lookup_f64(&Sku::new("aws.cloudfront.request_price_per_10k"))?;
        let transfer_price =
            pricing.lookup_f64(&Sku::new("aws.cloudfront.data_transfer_price_per_gb"))?;
        let free_tier =
            pricing.lookup_f64(&Sku::new("aws.cloudfront.free_tier_data_transfer_gb"))?;

        let cf_requests = Expr::linear(
            req_price_10k / 10_000.0,
            Expr::variable(id.var("http_requests")),
            0.0,
        );
        let cf_transfer = Expr::tiered(
            vec![
                Tier {
                    upper_limit: Some(free_tier),
                    unit_price: 0.0,
                },
                Tier {
                    upper_limit: None,
                    unit_price: transfer_price,
                },
            ],
            Expr::variable(id.var("data_transfer_gb")),
        );

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("CloudFront: {id}"),
            expr: Expr::sum(vec![cf_requests.clone(), cf_transfer.clone()]),
            components: vec![
                CostComponent {
                    name: "Requests".into(),
                    expr: cf_requests,
                },
                CostComponent {
                    name: "Data Transfer".into(),
                    expr: cf_transfer,
                },
            ],
            required_variables: vec![
                VariableInfo::new(id, "http_requests", "HTTP requests per month", "requests"),
                VariableInfo::new(id, "data_transfer_gb", "Data transfer per month", "GB"),
            ],
        })
    }
}
