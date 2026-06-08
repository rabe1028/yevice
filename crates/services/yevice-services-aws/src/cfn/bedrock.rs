use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};

use crate::services::bedrock::BedrockSpec;

pub struct BedrockCfnAdapter;

impl CfnAdapter for BedrockCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        // Bedrock invocation cost is usage-driven. We attach the cost shell to a
        // placeholder Bedrock resource the user lists in the template; the token
        // volumes come from usage.yaml.
        &["AWS::Bedrock::Agent"]
    }

    fn convert(&self, _raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        Ok(ResourceShell::new(
            "aws.bedrock",
            Provider::Aws,
            &BedrockSpec {},
        ))
    }
}
