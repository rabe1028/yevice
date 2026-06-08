use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{IacError, RawTfResource, TfAdapter};

use crate::services::lambda::LambdaSpec;

pub struct LambdaTfAdapter;

impl TfAdapter for LambdaTfAdapter {
    fn handles(&self) -> &[&'static str] {
        &["aws_lambda_function"]
    }

    fn convert(&self, raw: &RawTfResource) -> Result<ResourceShell, IacError> {
        let spec = LambdaSpec {
            memory_mb: raw.get_f64("memory_size").unwrap_or(128.0),
            timeout_sec: raw.get_f64("timeout").unwrap_or(3.0),
            runtime: raw.get_str("runtime").map(String::from),
        };
        Ok(ResourceShell::new("aws.lambda", Provider::Aws, &spec))
    }
}
