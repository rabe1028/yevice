//! TF adapters for GCP BigQuery.

use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::iac::{IacError, RawTfResource, TfAdapter};

use crate::services::bigquery::GcpBigQuerySpec;

pub struct GcpBigQueryTfAdapter;

impl TfAdapter for GcpBigQueryTfAdapter {
    fn handles(&self) -> &[&'static str] {
        &["google_bigquery_dataset", "google_bigquery_table"]
    }

    fn convert(&self, _raw: &RawTfResource) -> Result<ResourceShell, IacError> {
        Ok(ResourceShell::new(
            "gcp.bigquery",
            Provider::Gcp,
            &GcpBigQuerySpec {},
        ))
    }
}
