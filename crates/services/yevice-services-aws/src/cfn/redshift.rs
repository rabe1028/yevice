use crate::services::redshift::RedshiftSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};
pub struct RedshiftCfnAdapter;
impl CfnAdapter for RedshiftCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::Redshift::Cluster"]
    }
    fn convert(&self, raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        let spec = RedshiftSpec {
            node_type: raw.get_str("NodeType").unwrap_or("dc2.large").to_string(),
            node_count: raw.get_f64("NumberOfNodes"),
            // Optional run-hours override (default = full month in the service).
            hours: raw.get_f64("Hours"),
        };
        Ok(ResourceShell::new("aws.redshift", Provider::Aws, &spec))
    }
}
