use serde_json::Value;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};

use crate::services::opensearch_service::OpenSearchServiceSpec;

pub struct OpenSearchServiceCfnAdapter;

impl CfnAdapter for OpenSearchServiceCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &[
            "AWS::OpenSearchService::Domain",
            "AWS::Elasticsearch::Domain",
        ]
    }

    fn convert(&self, raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        let instance_type = raw
            .get_object("ClusterConfig")
            .and_then(|v| v.get("InstanceType"))
            .and_then(Value::as_str)
            .unwrap_or("r6g.large.search")
            .to_string();
        let instance_count = raw
            .get_object("ClusterConfig")
            .and_then(|v| v.get("InstanceCount"))
            .and_then(Value::as_f64);
        let storage_gb = raw
            .get_object("EBSOptions")
            .and_then(|v| v.get("VolumeSize"))
            .and_then(Value::as_f64);

        let spec = OpenSearchServiceSpec {
            instance_type,
            instance_count,
            storage_gb,
        };
        Ok(ResourceShell::new(
            "aws.opensearch_service",
            Provider::Aws,
            &spec,
        ))
    }
}
