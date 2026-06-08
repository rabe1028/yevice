use crate::services::s3::S3Spec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{IacError, RawTfResource, TfAdapter};
pub struct S3TfAdapter;
impl TfAdapter for S3TfAdapter {
    fn handles(&self) -> &[&'static str] {
        &["aws_s3_bucket"]
    }
    fn convert(&self, raw: &RawTfResource) -> Result<ResourceShell, IacError> {
        let versioning = raw
            .get_block("versioning")
            .and_then(|b| b.get("enabled"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        Ok(ResourceShell::new(
            "aws.s3",
            Provider::Aws,
            &S3Spec {
                versioning_enabled: versioning,
                storage_class: None,
            },
        ))
    }
}
