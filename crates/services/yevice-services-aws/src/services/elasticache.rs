use serde::{Deserialize, Serialize};
use yevice_core::{
    HOURS_PER_MONTH,
    cost::ResourceCost,
    expr::Expr,
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElastiCacheSpec {
    pub node_type: String,
    pub num_nodes: f64,
}

pub struct ElastiCacheService;

impl Service for ElastiCacheService {
    type Spec = ElastiCacheSpec;

    fn id(&self) -> &'static str {
        "aws.elasticache"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &ElastiCacheSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let sku = Sku::dynamic(format!("aws.elasticache.{}", spec.node_type));
        let hourly_price = pricing.lookup_f64(&sku)?;

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!(
                "ElastiCache: {id} ({}x{})",
                spec.num_nodes as u32, spec.node_type
            ),
            expr: Expr::constant(hourly_price * HOURS_PER_MONTH * spec.num_nodes),
            components: vec![],
            required_variables: vec![],

            currency: Some("USD".into()),
        })
    }
}
