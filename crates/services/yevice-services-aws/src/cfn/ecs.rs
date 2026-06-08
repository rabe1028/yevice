use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};

use crate::services::{ecs_ec2::EcsEc2Spec, ecs_fargate::EcsFargateSpec};

pub struct EcsCfnAdapter;

impl CfnAdapter for EcsCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::ECS::Service"]
    }

    fn convert(&self, raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        let launch_type = raw
            .get_str("LaunchType")
            .map_or_else(|| "FARGATE".to_string(), str::to_ascii_uppercase);

        match launch_type.as_str() {
            "EC2" | "EXTERNAL" => {
                let spec = EcsEc2Spec {
                    instance_type: "t3.medium".to_string(),
                    instance_count: raw.get_f64("DesiredCount"),
                };
                Ok(ResourceShell::new("aws.ecs_ec2", Provider::Aws, &spec))
            }
            _ => {
                let spec = EcsFargateSpec {
                    desired_count: raw.get_f64("DesiredCount"),
                };
                Ok(ResourceShell::new("aws.ecs_fargate", Provider::Aws, &spec))
            }
        }
    }
}
