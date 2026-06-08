use crate::services::msk::MskClusterSpec;
use serde_json::Value;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};
pub struct MskCfnAdapter;
impl CfnAdapter for MskCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::MSK::Cluster"]
    }
    fn convert(&self, raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        let instance_type = raw
            .get_object("BrokerNodeGroupInfo")
            .and_then(|v| v.get("InstanceType"))
            .and_then(Value::as_str)
            .unwrap_or("kafka.m5.large")
            .to_string();
        let broker_count = raw.get_f64("NumberOfBrokerNodes");
        let spec = MskClusterSpec {
            broker_instance_type: instance_type,
            broker_count,
        };
        Ok(ResourceShell::new("aws.msk", Provider::Aws, &spec))
    }
}
