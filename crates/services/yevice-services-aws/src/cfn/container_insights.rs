use crate::services::container_insights::ContainerInsightsSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};

pub struct EcsClusterCfnAdapter;

impl CfnAdapter for EcsClusterCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::ECS::Cluster"]
    }

    fn convert(&self, raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        // ClusterSettings: [{ Name: containerInsights, Value: enabled|enhanced }]
        let enabled = raw
            .get_object("ClusterSettings")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|settings| {
                settings.iter().any(|s| {
                    s.get("Name").and_then(serde_json::Value::as_str) == Some("containerInsights")
                        && matches!(
                            s.get("Value").and_then(serde_json::Value::as_str),
                            Some("enabled" | "enhanced")
                        )
                })
            });

        let spec = ContainerInsightsSpec { enabled };
        Ok(ResourceShell::new(
            "aws.container_insights",
            Provider::Aws,
            &spec,
        ))
    }
}
