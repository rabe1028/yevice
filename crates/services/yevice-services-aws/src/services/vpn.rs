use serde::{Deserialize, Serialize};
use yevice_core::{
    cost::{CostComponent, ResourceCost},
    expr::Expr,
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

const HOURS_PER_MONTH: f64 = 730.0;

/// Site-to-Site VPN connection (`AWS::EC2::VPNConnection`).
///
/// Billed per connection-hour; data transfer is billed by the egress model.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VpnSpec {}

pub struct VpnService;

impl Service for VpnService {
    type Spec = VpnSpec;

    fn id(&self) -> &'static str {
        "aws.vpn"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        _spec: &VpnSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let hour_price = pricing.lookup_f64(&Sku::new("aws.vpn.connection_hour"))?;
        let connection = Expr::constant(hour_price * HOURS_PER_MONTH);

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("Site-to-Site VPN: {id}"),
            expr: connection.clone(),
            components: vec![CostComponent {
                name: "Connection Hours".into(),
                expr: connection,
            }],
            required_variables: vec![],
        })
    }
}
