use crate::services::nat_gateway::NatGatewaySpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};
pub struct NatGatewayCfnAdapter;
impl CfnAdapter for NatGatewayCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::EC2::NatGateway"]
    }
    fn convert(&self, _raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        Ok(ResourceShell::new(
            "aws.nat_gateway",
            Provider::Aws,
            &NatGatewaySpec {},
        ))
    }
}
