use crate::services::vpn::VpnSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};

pub struct VpnCfnAdapter;

impl CfnAdapter for VpnCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::EC2::VPNConnection"]
    }

    fn convert(&self, _raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        Ok(ResourceShell::new("aws.vpn", Provider::Aws, &VpnSpec {}))
    }
}
