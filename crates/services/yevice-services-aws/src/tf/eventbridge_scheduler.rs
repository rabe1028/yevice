use crate::services::eventbridge_scheduler::EventBridgeSchedulerSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{IacError, RawTfResource, TfAdapter};
pub struct EventBridgeSchedulerTfAdapter;
impl TfAdapter for EventBridgeSchedulerTfAdapter {
    fn handles(&self) -> &[&'static str] {
        &["aws_scheduler_schedule"]
    }
    fn convert(&self, _raw: &RawTfResource) -> Result<ResourceShell, IacError> {
        Ok(ResourceShell::new(
            "aws.eventbridge_scheduler",
            Provider::Aws,
            &EventBridgeSchedulerSpec {},
        ))
    }
}
