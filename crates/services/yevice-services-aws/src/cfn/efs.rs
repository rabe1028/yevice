use crate::services::efs::EfsSpec;
use serde_json::Value;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};

pub struct EfsCfnAdapter;

impl CfnAdapter for EfsCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::EFS::FileSystem"]
    }

    fn convert(&self, raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        // `LifecyclePolicies` is a CloudFormation array of policy objects:
        //   - TransitionToIA: AFTER_30_DAYS
        //   - TransitionToPrimaryStorageClass: AFTER_1_ACCESS
        // Presence of any `TransitionToIA` entry means files transition out of
        // Standard into IA tier, which we price differently.
        let has_ia_lifecycle = raw
            .get_object("LifecyclePolicies")
            .and_then(Value::as_array)
            .is_some_and(|arr| arr.iter().any(|p| p.get("TransitionToIA").is_some()));
        Ok(ResourceShell::new(
            "aws.efs",
            Provider::Aws,
            &EfsSpec { has_ia_lifecycle },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Regression: `LifecyclePolicies` is an array, not a string. The previous
    /// `get_str` call always returned `None`, so IA-tier EFS filesystems were
    /// silently priced (and labelled) as Standard.
    #[test]
    fn detects_transition_to_ia_in_lifecycle_array() {
        let raw = RawCfnResource::new(
            "MyFS",
            "AWS::EFS::FileSystem",
            json!({
                "LifecyclePolicies": [
                    { "TransitionToIA": "AFTER_30_DAYS" },
                    { "TransitionToPrimaryStorageClass": "AFTER_1_ACCESS" }
                ]
            }),
        );
        let shell = EfsCfnAdapter.convert(&raw).expect("convert ok");
        let spec: EfsSpec = shell.decode().expect("decode spec");
        assert!(spec.has_ia_lifecycle, "should detect TransitionToIA");
    }

    #[test]
    fn no_lifecycle_policies_defaults_to_standard() {
        let raw = RawCfnResource::new("MyFS", "AWS::EFS::FileSystem", json!({}));
        let shell = EfsCfnAdapter.convert(&raw).expect("convert ok");
        let spec: EfsSpec = shell.decode().expect("decode spec");
        assert!(!spec.has_ia_lifecycle);
    }

    #[test]
    fn lifecycle_without_transition_to_ia_is_standard() {
        let raw = RawCfnResource::new(
            "MyFS",
            "AWS::EFS::FileSystem",
            json!({
                "LifecyclePolicies": [
                    { "TransitionToPrimaryStorageClass": "AFTER_1_ACCESS" }
                ]
            }),
        );
        let shell = EfsCfnAdapter.convert(&raw).expect("convert ok");
        let spec: EfsSpec = shell.decode().expect("decode spec");
        assert!(!spec.has_ia_lifecycle);
    }
}
