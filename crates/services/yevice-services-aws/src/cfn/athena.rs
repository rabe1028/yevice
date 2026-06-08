use crate::services::athena::AthenaSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};
pub struct AthenaCfnAdapter;
impl CfnAdapter for AthenaCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::Athena::WorkGroup"]
    }
    fn convert(&self, _raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        Ok(ResourceShell::new(
            "aws.athena",
            Provider::Aws,
            &AthenaSpec {},
        ))
    }
}
