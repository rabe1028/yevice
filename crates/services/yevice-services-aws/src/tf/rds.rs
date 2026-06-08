use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{IacError, RawTfResource, TfAdapter};

use crate::services::rds::{RdsEngine, RdsSpec};

pub struct RdsTfAdapter;

impl TfAdapter for RdsTfAdapter {
    fn handles(&self) -> &[&'static str] {
        &["aws_db_instance", "aws_rds_cluster"]
    }

    fn convert(&self, raw: &RawTfResource) -> Result<ResourceShell, IacError> {
        if raw.resource_type.as_str() == "aws_rds_cluster" {
            let spec = RdsSpec {
                instance_type: raw
                    .get_str("instance_class")
                    .unwrap_or("db.r5.large")
                    .to_string(),
                engine: RdsEngine::from_cfn(raw.get_str("engine").unwrap_or("aurora-mysql")),
                allocated_storage_gb: raw.get_f64("allocated_storage").unwrap_or(0.0),
                storage_type: "aurora".to_string(),
                iops: None,
                multi_az: false,
            };
            Ok(ResourceShell::new("aws.rds", Provider::Aws, &spec))
        } else {
            let spec = RdsSpec {
                instance_type: raw
                    .get_str("instance_class")
                    .unwrap_or("db.t3.micro")
                    .to_string(),
                engine: RdsEngine::from_cfn(raw.get_str("engine").unwrap_or("mysql")),
                allocated_storage_gb: raw.get_f64("allocated_storage").unwrap_or(20.0),
                storage_type: raw.get_str("storage_type").unwrap_or("gp2").to_string(),
                iops: raw.get_f64("iops"),
                multi_az: raw.get_bool("multi_az").unwrap_or(false),
            };
            Ok(ResourceShell::new("aws.rds", Provider::Aws, &spec))
        }
    }
}
