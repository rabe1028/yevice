use crate::services::cognito::CognitoUserPoolSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};
pub struct CognitoCfnAdapter;
impl CfnAdapter for CognitoCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::Cognito::UserPool"]
    }
    fn convert(&self, _raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        Ok(ResourceShell::new(
            "aws.cognito",
            Provider::Aws,
            &CognitoUserPoolSpec {},
        ))
    }
}
