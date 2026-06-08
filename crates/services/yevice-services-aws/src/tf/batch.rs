use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{IacError, RawTfResource, TfAdapter};

use crate::services::batch::{BatchJobDefinitionSpec, BatchLaunchType};

pub struct BatchTfAdapter;

impl TfAdapter for BatchTfAdapter {
    fn handles(&self) -> &[&'static str] {
        &["aws_batch_job_definition"]
    }

    fn convert(&self, raw: &RawTfResource) -> Result<ResourceShell, IacError> {
        let vcpu = raw
            .get_block("container_properties")
            .and_then(|b| b.get("vcpus"))
            .and_then(serde_json::Value::as_f64)
            .or_else(|| raw.get_f64("vcpus"))
            .unwrap_or(1.0);

        let memory_gb = raw
            .get_block("container_properties")
            .and_then(|b| b.get("memory"))
            .and_then(serde_json::Value::as_f64)
            .or_else(|| raw.get_f64("memory"))
            .map_or(2.0, |v| v / 1024.0);

        let spec = BatchJobDefinitionSpec {
            launch_type: BatchLaunchType::Fargate,
            vcpu,
            memory_gb,
            ephemeral_storage_gb: None,
            ebs: None,
        };
        Ok(ResourceShell::new("aws.batch", Provider::Aws, &spec))
    }
}
