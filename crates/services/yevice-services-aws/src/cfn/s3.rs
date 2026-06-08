use serde_json::Value;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};

use crate::services::s3::S3Spec;

pub struct S3CfnAdapter;

impl CfnAdapter for S3CfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::S3::Bucket"]
    }

    fn convert(&self, raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        let versioning = raw
            .get_object("VersioningConfiguration")
            .and_then(|v| v.get("Status"))
            .and_then(Value::as_str)
            .is_some_and(|s| s == "Enabled");

        let spec = S3Spec {
            versioning_enabled: versioning,
            storage_class: None,
        };
        Ok(ResourceShell::new("aws.s3", Provider::Aws, &spec))
    }
}
