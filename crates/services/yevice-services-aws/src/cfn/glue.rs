use crate::services::glue::{GlueDpuType, GlueJobSpec};
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};
pub struct GlueCfnAdapter;
impl CfnAdapter for GlueCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::Glue::Job"]
    }
    fn convert(&self, raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        let max_dpu = raw
            .get_f64("MaxCapacity")
            .or_else(|| raw.get_f64("NumberOfWorkers"));
        let spec = GlueJobSpec {
            dpu_type: GlueDpuType::Standard,
            max_dpu,
        };
        Ok(ResourceShell::new("aws.glue", Provider::Aws, &spec))
    }
}
