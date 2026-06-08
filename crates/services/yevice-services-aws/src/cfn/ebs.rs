use crate::services::ebs::EbsSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};

pub struct EbsCfnAdapter;

impl CfnAdapter for EbsCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::EC2::Volume"]
    }

    fn convert(&self, raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        let spec = EbsSpec {
            // CloudFormation defaults AWS::EC2::Volume to gp2 when VolumeType is omitted.
            volume_type: raw.get_str("VolumeType").unwrap_or("gp2").to_string(),
            size_gb: raw.get_f64("Size"),
            iops: raw.get_f64("Iops"),
            throughput: raw.get_f64("Throughput"),
        };
        Ok(ResourceShell::new("aws.ebs", Provider::Aws, &spec))
    }
}
