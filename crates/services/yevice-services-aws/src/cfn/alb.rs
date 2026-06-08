use crate::services::alb::AlbSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};
pub struct AlbCfnAdapter;
impl CfnAdapter for AlbCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::ElasticLoadBalancingV2::LoadBalancer"]
    }
    fn convert(&self, raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        let spec = AlbSpec {
            load_balancer_type: raw.get_str("Type").unwrap_or("application").to_string(),
        };
        Ok(ResourceShell::new("aws.alb", Provider::Aws, &spec))
    }
}
