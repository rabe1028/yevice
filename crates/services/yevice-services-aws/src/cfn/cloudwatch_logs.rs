use crate::services::cloudwatch_logs::CloudWatchLogsSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};
pub struct CloudWatchLogsCfnAdapter;
impl CfnAdapter for CloudWatchLogsCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::Logs::LogGroup"]
    }
    fn convert(&self, raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        let spec = CloudWatchLogsSpec {
            retention_days: raw.get_f64("RetentionInDays").map(|n| n as u32),
        };
        Ok(ResourceShell::new(
            "aws.cloudwatch_logs",
            Provider::Aws,
            &spec,
        ))
    }
}
