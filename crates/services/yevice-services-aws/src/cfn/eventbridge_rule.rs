use crate::services::eventbridge_rule::EventBridgeRuleSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};
pub struct EventBridgeRuleCfnAdapter;
impl CfnAdapter for EventBridgeRuleCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::Events::Rule"]
    }
    fn convert(&self, _raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        Ok(ResourceShell::new(
            "aws.eventbridge_rule",
            Provider::Aws,
            &EventBridgeRuleSpec {},
        ))
    }
}
