use crate::services::nat_gateway::NatGatewaySpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{IacError, RawTfResource, TfAdapter};
pub struct NatGatewayTfAdapter;
impl TfAdapter for NatGatewayTfAdapter {
    fn handles(&self) -> &[&'static str] {
        &["aws_nat_gateway"]
    }
    fn convert(&self, _raw: &RawTfResource) -> Result<ResourceShell, IacError> {
        Ok(ResourceShell::new(
            "aws.nat_gateway",
            Provider::Aws,
            &NatGatewaySpec {},
        ))
    }
}
