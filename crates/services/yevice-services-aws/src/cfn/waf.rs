use crate::services::waf::WafSpec;
use serde_json::Value;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};
pub struct WafCfnAdapter;
impl CfnAdapter for WafCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::WAFv2::WebACL"]
    }
    fn convert(&self, raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        let rule_count = raw
            .get_object("Rules")
            .and_then(Value::as_array)
            .map(|a| a.len() as f64);
        Ok(ResourceShell::new(
            "aws.waf",
            Provider::Aws,
            &WafSpec { rule_count },
        ))
    }
}
