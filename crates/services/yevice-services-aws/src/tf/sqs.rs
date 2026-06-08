use crate::services::sqs::SqsSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{IacError, RawTfResource, TfAdapter};
pub struct SqsTfAdapter;
impl TfAdapter for SqsTfAdapter {
    fn handles(&self) -> &[&'static str] {
        &["aws_sqs_queue"]
    }
    fn convert(&self, raw: &RawTfResource) -> Result<ResourceShell, IacError> {
        let fifo = raw.get_bool("fifo_queue").unwrap_or(false);
        Ok(ResourceShell::new(
            "aws.sqs",
            Provider::Aws,
            &SqsSpec { fifo },
        ))
    }
}
