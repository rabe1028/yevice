use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};

use crate::services::lightsail::LightsailSpec;

pub struct LightsailCfnAdapter;

impl CfnAdapter for LightsailCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::Lightsail::Instance", "AWS::Lightsail::Disk"]
    }

    fn convert(&self, raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        let is_disk = raw.resource_type.as_str() == "AWS::Lightsail::Disk";

        // The instance plan (price determinant) is `BundleId`; `BlueprintId` is
        // the OS/app image and does not affect the bundle price.
        let bundle_id = raw
            .get_str("BundleId")
            .map(std::string::ToString::to_string);

        // Only a standalone `AWS::Lightsail::Disk` carries a billable size
        // (`SizeInGb`). An instance's root SSD is already covered by its bundle
        // price, so instances never carry a separate disk size.
        let disk_size_gb = if is_disk {
            Some(raw.get_f64("SizeInGb").unwrap_or(0.0))
        } else {
            None
        };

        let spec = LightsailSpec {
            bundle_id,
            disk_size_gb,
            is_disk,
        };
        Ok(ResourceShell::new("aws.lightsail", Provider::Aws, &spec))
    }
}
