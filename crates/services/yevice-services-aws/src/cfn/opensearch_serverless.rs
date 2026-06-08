use crate::services::opensearch_serverless::OpenSearchServerlessSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};
pub struct OpenSearchServerlessCfnAdapter;
impl CfnAdapter for OpenSearchServerlessCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::OpenSearchServerless::Collection"]
    }
    fn convert(&self, raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        let spec = OpenSearchServerlessSpec {
            collection_type: raw.get_str("Type").map(String::from),
        };
        Ok(ResourceShell::new(
            "aws.opensearch_serverless",
            Provider::Aws,
            &spec,
        ))
    }
}
