use crate::services::kinesis::{KinesisSpec, KinesisStreamMode};
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{IacError, RawTfResource, TfAdapter};
pub struct KinesisTfAdapter;
impl TfAdapter for KinesisTfAdapter {
    fn handles(&self) -> &[&'static str] {
        &["aws_kinesis_stream"]
    }
    fn convert(&self, raw: &RawTfResource) -> Result<ResourceShell, IacError> {
        let mode = raw
            .get_block("stream_mode_details")
            .and_then(|b| b.get("stream_mode"))
            .and_then(serde_json::Value::as_str);
        let stream_mode = if mode.is_some_and(|m| m.eq_ignore_ascii_case("ON_DEMAND")) {
            KinesisStreamMode::OnDemand
        } else {
            KinesisStreamMode::Provisioned {
                shard_count: raw.get_f64("shard_count"),
            }
        };
        let spec = KinesisSpec {
            stream_mode,
            retention_hours: raw.get_f64("retention_period").unwrap_or(24.0),
        };
        Ok(ResourceShell::new("aws.kinesis", Provider::Aws, &spec))
    }
}
