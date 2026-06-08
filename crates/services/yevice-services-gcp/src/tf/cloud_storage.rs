//! TF adapter for GCP Cloud Storage.

use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::iac::{IacError, RawTfResource, TfAdapter};

use crate::services::cloud_storage::GcpCloudStorageSpec;

pub struct GcpCloudStorageTfAdapter;

impl TfAdapter for GcpCloudStorageTfAdapter {
    fn handles(&self) -> &[&'static str] {
        &["google_storage_bucket"]
    }

    fn convert(&self, raw: &RawTfResource) -> Result<ResourceShell, IacError> {
        let spec = GcpCloudStorageSpec {
            storage_class: raw.get_str("storage_class").map(ToOwned::to_owned),
            location_type: raw.get_str("location").map(ToOwned::to_owned),
        };
        Ok(ResourceShell::new(
            "gcp.cloud_storage",
            Provider::Gcp,
            &spec,
        ))
    }
}
