use crate::services::{ecs_ec2::EcsEc2Spec, ecs_fargate::EcsFargateSpec};
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{IacError, RawTfResource, TfAdapter};
pub struct EcsTfAdapter;
impl TfAdapter for EcsTfAdapter {
    fn handles(&self) -> &[&'static str] {
        &["aws_ecs_service"]
    }
    fn convert(&self, raw: &RawTfResource) -> Result<ResourceShell, IacError> {
        let launch_type = raw
            .get_str("launch_type")
            .map_or_else(|| "FARGATE".to_string(), str::to_ascii_uppercase);
        match launch_type.as_str() {
            "EC2" | "EXTERNAL" => {
                let spec = EcsEc2Spec {
                    instance_type: "t3.medium".to_string(),
                    instance_count: raw.get_f64("desired_count"),
                };
                Ok(ResourceShell::new("aws.ecs_ec2", Provider::Aws, &spec))
            }
            _ => {
                let spec = EcsFargateSpec {
                    desired_count: raw.get_f64("desired_count"),
                };
                Ok(ResourceShell::new("aws.ecs_fargate", Provider::Aws, &spec))
            }
        }
    }
}
