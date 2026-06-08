use crate::services::fsx_windows::FsxWindowsSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};

pub struct FsxWindowsCfnAdapter;

impl CfnAdapter for FsxWindowsCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::FSx::FileSystem"]
    }

    fn convert(&self, raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        // `AWS::FSx::FileSystem` is shared across all FSx flavours; only the
        // Windows File Server type is priced here. Other types (LUSTRE,
        // ONTAP, OPENZFS) are left unpriced by reporting no usable shell.
        let fs_type = raw.get_str("FileSystemType").unwrap_or("");
        if !fs_type.eq_ignore_ascii_case("windows") {
            return Err(IacError::InvalidValue {
                field: "FileSystemType".into(),
                cause: format!("expected WINDOWS, got '{fs_type}'"),
            });
        }

        // StorageType is optional in CFN and defaults to SSD.
        let storage_type = raw.get_str("StorageType").unwrap_or("SSD").to_string();
        let storage_capacity_gb = raw.get_f64("StorageCapacity");

        // Throughput capacity and deployment type live in the nested
        // WindowsConfiguration block.
        let windows_config = raw.get_object("WindowsConfiguration");
        let throughput_capacity_mbps = windows_config
            .and_then(|c| c.get("ThroughputCapacity"))
            .and_then(|v| match v {
                serde_json::Value::Number(n) => n.as_f64(),
                serde_json::Value::String(s) => s.parse::<f64>().ok(),
                _ => None,
            });
        // DeploymentType: MULTI_AZ_1 -> Multi-AZ; SINGLE_AZ_1/SINGLE_AZ_2 -> Single-AZ.
        let multi_az = windows_config
            .and_then(|c| c.get("DeploymentType"))
            .and_then(serde_json::Value::as_str)
            .is_some_and(|d| d.eq_ignore_ascii_case("MULTI_AZ_1"));

        let spec = FsxWindowsSpec {
            storage_type,
            multi_az,
            storage_capacity_gb,
            throughput_capacity_mbps,
        };
        Ok(ResourceShell::new("aws.fsx_windows", Provider::Aws, &spec))
    }
}
