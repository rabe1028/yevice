use crate::services::firehose::KinesisFirehoseSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};
pub struct FirehoseCfnAdapter;
impl CfnAdapter for FirehoseCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::KinesisFirehose::DeliveryStream"]
    }
    fn convert(&self, _raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        Ok(ResourceShell::new(
            "aws.firehose",
            Provider::Aws,
            &KinesisFirehoseSpec {},
        ))
    }
}
