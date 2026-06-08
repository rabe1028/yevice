use crate::services::cloudwatch_logs::CloudWatchLogsSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{IacError, RawTfResource, TfAdapter};
pub struct CloudWatchLogsTfAdapter;
impl TfAdapter for CloudWatchLogsTfAdapter {
    fn handles(&self) -> &[&'static str] {
        &["aws_cloudwatch_log_group"]
    }
    fn convert(&self, raw: &RawTfResource) -> Result<ResourceShell, IacError> {
        let spec = CloudWatchLogsSpec {
            retention_days: raw.get_f64("retention_in_days").map(|n| n as u32),
        };
        Ok(ResourceShell::new(
            "aws.cloudwatch_logs",
            Provider::Aws,
            &spec,
        ))
    }
}
