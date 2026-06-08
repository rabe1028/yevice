use serde::{Deserialize, Serialize};

/// Region-specific GCP pricing data (flat struct; serialisable to/from gcp.json).
///
/// Reference prices (as of 2024):
///   Cloud Run/Functions/Pub/Sub/BigQuery: uniform across regions
///   Cloud Storage standard: us-central1 $0.020/GB; asia-northeast1 $0.023/GB
///   Cloud SQL: significant regional variance (see gcp_registry.rs)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcpPricing {
    pub region: String,
    // Cloud Run
    pub cloud_run_request_per_million: f64,
    pub cloud_run_vcpu_second: f64,
    pub cloud_run_memory_gb_second: f64,
    /// Idle (min-instance, allocated-but-not-serving) vCPU-second rate.
    pub cloud_run_idle_vcpu_second: f64,
    // Cloud Functions Gen2
    pub cloud_function_invocation_per_million: f64,
    pub cloud_function_gb_second: f64,
    // BigQuery
    pub bigquery_active_storage_gb_month: f64,
    pub bigquery_query_per_tb: f64,
    // Cloud Storage
    pub cloud_storage_standard_gb_month: f64,
    pub cloud_storage_nearline_gb_month: f64,
    pub cloud_storage_coldline_gb_month: f64,
    pub cloud_storage_archive_gb_month: f64,
    // Pub/Sub
    pub pubsub_data_gb: f64,
    // Cloud SQL (per instance, before HA multiplier)
    pub cloud_sql_vcpu_hour: f64,
    pub cloud_sql_ram_gb_hour: f64,
    pub cloud_sql_ssd_gb_month: f64,
}
