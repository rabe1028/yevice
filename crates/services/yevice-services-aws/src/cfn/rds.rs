use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};

use crate::services::rds::{RdsEngine, RdsSpec};

pub struct RdsCfnAdapter;

impl CfnAdapter for RdsCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::RDS::DBInstance", "AWS::RDS::DBCluster"]
    }

    fn convert(&self, raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        let engine_str = raw.get_str("Engine").unwrap_or("mysql");
        let multi_az = raw.get_bool("MultiAZ").unwrap_or(false);

        let spec = RdsSpec {
            instance_type: raw
                .get_str("DBInstanceClass")
                .unwrap_or("db.t3.micro")
                .to_string(),
            engine: RdsEngine::from_cfn(engine_str),
            allocated_storage_gb: raw.get_f64("AllocatedStorage").unwrap_or(20.0),
            storage_type: raw.get_str("StorageType").unwrap_or("gp2").to_string(),
            iops: raw.get_f64("Iops"),
            multi_az,
        };
        Ok(ResourceShell::new("aws.rds", Provider::Aws, &spec))
    }
}
