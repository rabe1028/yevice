use crate::services::cloudtrail::CloudTrailSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};

pub struct CloudTrailCfnAdapter;

impl CfnAdapter for CloudTrailCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::CloudTrail::Trail"]
    }

    fn convert(&self, _raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        Ok(ResourceShell::new(
            "aws.cloudtrail",
            Provider::Aws,
            &CloudTrailSpec {},
        ))
    }
}
