use crate::services::ec2::{Ec2Os, Ec2Spec};
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{IacError, RawTfResource, TfAdapter};
pub struct Ec2TfAdapter;
impl TfAdapter for Ec2TfAdapter {
    fn handles(&self) -> &[&'static str] {
        &["aws_instance"]
    }
    fn convert(&self, raw: &RawTfResource) -> Result<ResourceShell, IacError> {
        let spec = Ec2Spec {
            instance_type: raw
                .get_str("instance_type")
                .unwrap_or("t3.micro")
                .to_string(),
            os: Ec2Os::Linux,
        };
        Ok(ResourceShell::new("aws.ec2", Provider::Aws, &spec))
    }
}
