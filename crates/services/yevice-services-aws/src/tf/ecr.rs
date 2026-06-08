use crate::services::ecr::EcrSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{IacError, RawTfResource, TfAdapter};
pub struct EcrTfAdapter;
impl TfAdapter for EcrTfAdapter {
    fn handles(&self) -> &[&'static str] {
        &["aws_ecr_repository"]
    }
    fn convert(&self, _raw: &RawTfResource) -> Result<ResourceShell, IacError> {
        Ok(ResourceShell::new(
            "aws.ecr",
            Provider::Aws,
            &EcrSpec { is_private: true },
        ))
    }
}
