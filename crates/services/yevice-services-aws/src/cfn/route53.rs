use crate::services::route53::Route53HostedZoneSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};
pub struct Route53CfnAdapter;
impl CfnAdapter for Route53CfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::Route53::HostedZone"]
    }
    fn convert(&self, _raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        let spec = Route53HostedZoneSpec {
            zone_type: "public".to_string(),
        };
        Ok(ResourceShell::new("aws.route53", Provider::Aws, &spec))
    }
}
