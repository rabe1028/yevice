use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};

use crate::services::sqs::SqsSpec;

pub struct SqsCfnAdapter;

impl CfnAdapter for SqsCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::SQS::Queue"]
    }

    fn convert(&self, raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        let fifo = raw.get_bool("FifoQueue").unwrap_or(false);
        Ok(ResourceShell::new(
            "aws.sqs",
            Provider::Aws,
            &SqsSpec { fifo },
        ))
    }
}
