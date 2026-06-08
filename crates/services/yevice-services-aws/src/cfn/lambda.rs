use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};

use crate::services::lambda::LambdaSpec;

pub struct LambdaCfnAdapter;

impl CfnAdapter for LambdaCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::Lambda::Function", "AWS::Serverless::Function"]
    }

    fn convert(&self, raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        let spec = LambdaSpec {
            memory_mb: raw.get_f64("MemorySize").unwrap_or(128.0),
            timeout_sec: raw.get_f64("Timeout").unwrap_or(3.0),
            runtime: raw.get_str("Runtime").map(String::from),
        };
        Ok(ResourceShell::new("aws.lambda", Provider::Aws, &spec))
    }
}
