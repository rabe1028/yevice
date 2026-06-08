use crate::services::documentdb::DocumentDbSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};
pub struct DocumentDbCfnAdapter;
impl CfnAdapter for DocumentDbCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::DocDB::DBCluster", "AWS::DocDB::DBInstance"]
    }
    fn convert(&self, raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        let (instance_type, instance_count) =
            if raw.resource_type.as_str() == "AWS::DocDB::DBInstance" {
                (
                    raw.get_str("DBInstanceClass")
                        .unwrap_or("db.r6g.large")
                        .to_string(),
                    Some(1.0),
                )
            } else {
                ("db.r6g.large".to_string(), None)
            };
        let spec = DocumentDbSpec {
            instance_type,
            instance_count,
        };
        Ok(ResourceShell::new("aws.documentdb", Provider::Aws, &spec))
    }
}
