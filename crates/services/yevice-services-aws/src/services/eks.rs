use serde::{Deserialize, Serialize};
use yevice_core::{
    cost::{CostComponent, ResourceCost},
    expr::Expr,
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EksClusterSpec {}

pub struct EksService;

const HOURS_PER_MONTH: f64 = 730.0;

impl Service for EksService {
    type Spec = EksClusterSpec;

    fn id(&self) -> &'static str {
        "aws.eks"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        _spec: &EksClusterSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let cluster_hour = pricing.lookup_f64(&Sku::new("aws.eks.cluster_hour_price"))?;
        let cluster_fee = Expr::constant(cluster_hour * HOURS_PER_MONTH);

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("EKS Cluster: {id}"),
            expr: cluster_fee.clone(),
            components: vec![CostComponent {
                name: "Cluster Management".into(),
                expr: cluster_fee,
            }],
            required_variables: vec![],
        })
    }
}

const _: f64 = HOURS_PER_MONTH;
