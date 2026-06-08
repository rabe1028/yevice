use crate::services::secrets_manager::SecretsManagerSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};
pub struct SecretsManagerCfnAdapter;
impl CfnAdapter for SecretsManagerCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::SecretsManager::Secret"]
    }
    fn convert(&self, _raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        Ok(ResourceShell::new(
            "aws.secrets_manager",
            Provider::Aws,
            &SecretsManagerSpec {},
        ))
    }
}
