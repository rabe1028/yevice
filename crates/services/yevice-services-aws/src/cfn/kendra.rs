use crate::services::kendra::KendraSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};

pub struct KendraCfnAdapter;

impl CfnAdapter for KendraCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::Kendra::Index"]
    }

    fn convert(&self, raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        let spec = KendraSpec {
            edition: raw
                .get_str("Edition")
                .unwrap_or("DEVELOPER_EDITION")
                .to_string(),
        };
        Ok(ResourceShell::new("aws.kendra", Provider::Aws, &spec))
    }
}
