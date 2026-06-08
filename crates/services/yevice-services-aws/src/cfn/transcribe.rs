use crate::services::transcribe::TranscribeSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};
pub struct TranscribeCfnAdapter;
impl CfnAdapter for TranscribeCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::Transcribe::Vocabulary"]
    }
    fn convert(&self, _raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        Ok(ResourceShell::new(
            "aws.transcribe",
            Provider::Aws,
            &TranscribeSpec {},
        ))
    }
}
