use crate::services::cloudfront::CloudFrontSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{IacError, RawTfResource, TfAdapter};
pub struct CloudFrontTfAdapter;
impl TfAdapter for CloudFrontTfAdapter {
    fn handles(&self) -> &[&'static str] {
        &["aws_cloudfront_distribution"]
    }
    fn convert(&self, _raw: &RawTfResource) -> Result<ResourceShell, IacError> {
        Ok(ResourceShell::new(
            "aws.cloudfront",
            Provider::Aws,
            &CloudFrontSpec {},
        ))
    }
}
