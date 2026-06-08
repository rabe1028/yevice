use serde_json::Value;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};

use crate::services::kinesis::{KinesisSpec, KinesisStreamMode};

pub struct KinesisCfnAdapter;

impl CfnAdapter for KinesisCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::Kinesis::Stream"]
    }

    fn convert(&self, raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        let mode = raw
            .get_object("StreamModeDetails")
            .and_then(|v| v.get("StreamMode"))
            .and_then(Value::as_str)
            .unwrap_or("PROVISIONED");

        let stream_mode = if mode == "ON_DEMAND" {
            KinesisStreamMode::OnDemand
        } else {
            KinesisStreamMode::Provisioned {
                shard_count: raw.get_f64("ShardCount"),
            }
        };

        let spec = KinesisSpec {
            stream_mode,
            retention_hours: raw.get_f64("RetentionPeriodHours").unwrap_or(24.0),
        };
        Ok(ResourceShell::new("aws.kinesis", Provider::Aws, &spec))
    }
}
