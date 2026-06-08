use crate::services::sns::SnsSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};
pub struct SnsCfnAdapter;
impl CfnAdapter for SnsCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::SNS::Topic"]
    }
    fn convert(&self, raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        let spec = SnsSpec {
            fifo: raw.get_bool("FifoTopic").unwrap_or(false),
        };
        Ok(ResourceShell::new("aws.sns", Provider::Aws, &spec))
    }
}
