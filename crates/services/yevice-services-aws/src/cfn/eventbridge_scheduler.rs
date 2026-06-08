use crate::services::eventbridge_scheduler::EventBridgeSchedulerSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};
pub struct EventBridgeSchedulerCfnAdapter;
impl CfnAdapter for EventBridgeSchedulerCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::Scheduler::Schedule"]
    }
    fn convert(&self, _raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        Ok(ResourceShell::new(
            "aws.eventbridge_scheduler",
            Provider::Aws,
            &EventBridgeSchedulerSpec {},
        ))
    }
}
