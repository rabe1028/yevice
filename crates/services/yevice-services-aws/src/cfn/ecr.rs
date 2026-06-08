use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};

use crate::services::ecr::EcrSpec;

pub struct EcrCfnAdapter;

impl CfnAdapter for EcrCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::ECR::Repository"]
    }

    fn convert(&self, _raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        Ok(ResourceShell::new(
            "aws.ecr",
            Provider::Aws,
            &EcrSpec { is_private: true },
        ))
    }
}
