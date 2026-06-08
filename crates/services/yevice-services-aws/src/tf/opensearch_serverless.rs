use crate::services::opensearch_serverless::OpenSearchServerlessSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{IacError, RawTfResource, TfAdapter};
pub struct OpenSearchServerlessTfAdapter;
impl TfAdapter for OpenSearchServerlessTfAdapter {
    fn handles(&self) -> &[&'static str] {
        &["aws_opensearchserverless_collection"]
    }
    fn convert(&self, raw: &RawTfResource) -> Result<ResourceShell, IacError> {
        let spec = OpenSearchServerlessSpec {
            collection_type: raw.get_str("type").map(String::from),
        };
        Ok(ResourceShell::new(
            "aws.opensearch_serverless",
            Provider::Aws,
            &spec,
        ))
    }
}
