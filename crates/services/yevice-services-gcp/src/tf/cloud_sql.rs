//! TF adapter for GCP Cloud SQL.

use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::iac::{IacError, RawTfResource, TfAdapter};

use crate::services::cloud_sql::GcpCloudSqlSpec;

pub struct GcpCloudSqlTfAdapter;

impl TfAdapter for GcpCloudSqlTfAdapter {
    fn handles(&self) -> &[&'static str] {
        &["google_sql_database_instance"]
    }

    fn convert(&self, raw: &RawTfResource) -> Result<ResourceShell, IacError> {
        let spec = GcpCloudSqlSpec {
            tier: raw
                .get_block("settings")
                .and_then(|b| b.get("tier"))
                .and_then(|v| v.as_str())
                .or_else(|| raw.get_str("tier"))
                .or_else(|| raw.get_str("database_type"))
                .unwrap_or("db-n1-standard-1")
                .to_string(),
            database_version: raw
                .get_str("database_version")
                .unwrap_or("POSTGRES_15")
                .to_string(),
            disk_size_gb: raw
                .get_block("settings")
                .and_then(|b| b.get("disk_size"))
                .and_then(serde_json::Value::as_f64)
                .or_else(|| raw.get_f64("disk_size")),
            availability_type: raw
                .get_block("settings")
                .and_then(|b| b.get("availability_type"))
                .and_then(|v| v.as_str())
                .or_else(|| raw.get_str("availability_type"))
                .unwrap_or("ZONAL")
                .to_string(),
        };
        Ok(ResourceShell::new("gcp.cloud_sql", Provider::Gcp, &spec))
    }
}
