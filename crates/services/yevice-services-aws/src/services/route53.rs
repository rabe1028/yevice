use serde::{Deserialize, Serialize};
use yevice_core::{
    cost::{CostComponent, ResourceCost, VariableInfo},
    expr::Expr,
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Route53HostedZoneSpec {
    pub zone_type: String,
}

pub struct Route53Service;

impl Service for Route53Service {
    type Spec = Route53HostedZoneSpec;

    fn id(&self) -> &'static str {
        "aws.route53"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        _spec: &Route53HostedZoneSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let zone_price = pricing.lookup_f64(&Sku::new("aws.route53.hosted_zone_month_price"))?;
        let query_price = pricing.lookup_f64(&Sku::new("aws.route53.query_price_per_million"))?;

        let zone_cost = Expr::constant(zone_price);
        let query_cost = Expr::linear(
            query_price / 1_000_000.0,
            Expr::variable(id.var("queries")),
            0.0,
        );
        let total = Expr::sum(vec![zone_cost.clone(), query_cost.clone()]);

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("Route53: {id}"),
            expr: total,
            components: vec![
                CostComponent {
                    name: "Hosted Zone".into(),
                    expr: zone_cost,

                    currency: None,
                },
                CostComponent {
                    name: "Queries".into(),
                    expr: query_cost,

                    currency: None,
                },
            ],
            required_variables: vec![VariableInfo::new(
                id,
                "queries",
                "DNS queries per month",
                "requests",
            )],

            currency: Some("USD".into()),
        })
    }
}
