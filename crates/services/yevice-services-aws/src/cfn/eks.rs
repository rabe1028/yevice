use crate::services::eks::EksClusterSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};
pub struct EksCfnAdapter;
impl CfnAdapter for EksCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::EKS::Cluster"]
    }
    fn convert(&self, _raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        Ok(ResourceShell::new(
            "aws.eks",
            Provider::Aws,
            &EksClusterSpec {},
        ))
    }
}
