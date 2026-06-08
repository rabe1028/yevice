use crate::services::appsync::AppSyncSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};
pub struct AppSyncCfnAdapter;
impl CfnAdapter for AppSyncCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::AppSync::GraphQLApi"]
    }
    fn convert(&self, _raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        Ok(ResourceShell::new(
            "aws.appsync",
            Provider::Aws,
            &AppSyncSpec {},
        ))
    }
}
