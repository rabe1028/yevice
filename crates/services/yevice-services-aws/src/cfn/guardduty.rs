use crate::services::guardduty::GuardDutySpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};

pub struct GuardDutyCfnAdapter;

impl CfnAdapter for GuardDutyCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::GuardDuty::Detector"]
    }

    fn convert(&self, _raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        // The detector is a usage-only marker; volumes come from usage.yaml.
        Ok(ResourceShell::new(
            "aws.guardduty",
            Provider::Aws,
            &GuardDutySpec {},
        ))
    }
}
