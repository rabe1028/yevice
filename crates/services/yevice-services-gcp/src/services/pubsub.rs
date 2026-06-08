//! GCP Pub/Sub service implementation.

use serde::{Deserialize, Serialize};
use yevice_core::{
    cost::{CostComponent, Expr, ResourceCost, Tier, VariableInfo, VariableKind},
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::{PriceCatalog, Sku};
use yevice_service_api::{CostError, service::Service};

const FREE_TIER_GB: f64 = 10.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcpPubSubSpec {}

pub struct GcpPubSubService;

impl Service for GcpPubSubService {
    type Spec = GcpPubSubSpec;

    fn id(&self) -> &'static str {
        "gcp.pubsub"
    }

    fn provider(&self) -> Provider {
        Provider::Gcp
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        _spec: &GcpPubSubSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let data_gb = pricing.lookup_f64(&Sku::new("gcp.pubsub.data_gb"))?;

        let cost = Expr::tiered(
            vec![
                Tier {
                    upper_limit: Some(FREE_TIER_GB),
                    unit_price: 0.0,
                },
                Tier {
                    upper_limit: None,
                    unit_price: data_gb,
                },
            ],
            Expr::variable(id.var("data_gb")),
        );

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: "Pub/Sub".into(),
            expr: cost.clone(),
            components: vec![CostComponent {
                name: "Data Volume".into(),
                expr: cost,
            }],
            required_variables: vec![VariableInfo {
                name: id.var("data_gb"),
                description: "Message data volume per month".into(),
                unit: "GB".into(),
                kind: VariableKind::Usage,
            }],
        })
    }
}
