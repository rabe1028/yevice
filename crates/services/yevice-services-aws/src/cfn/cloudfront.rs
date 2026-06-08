use crate::services::cloudfront::CloudFrontSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};
pub struct CloudFrontCfnAdapter;
impl CfnAdapter for CloudFrontCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::CloudFront::Distribution"]
    }
    fn convert(&self, _raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        Ok(ResourceShell::new(
            "aws.cloudfront",
            Provider::Aws,
            &CloudFrontSpec {},
        ))
    }
}
