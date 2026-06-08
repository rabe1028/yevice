use crate::services::cloudwatch::CloudWatchSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};

pub struct CloudWatchCfnAdapter;

impl CfnAdapter for CloudWatchCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::CloudWatch::Alarm"]
    }

    fn convert(&self, raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        // Each AWS::CloudWatch::Alarm is one alarm; an optional `AlarmCount`
        // lets one resource represent several identical alarms.
        let spec = CloudWatchSpec {
            alarm_count: raw.get_f64("AlarmCount"),
        };
        Ok(ResourceShell::new("aws.cloudwatch", Provider::Aws, &spec))
    }
}
