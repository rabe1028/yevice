//! GCP pricing registry with regional hardcoded defaults.
//!
//! Regional prices (as of 2024, USD):
//!
//! | Region           | SQL vCPU/hr | SQL RAM GB/hr | SQL SSD GB/mo | GCS Standard GB/mo |
//! |------------------|-------------|---------------|---------------|--------------------|
//! | us-central1      | 0.0413      | 0.0070        | 0.170         | 0.020              |
//! | us-east1         | 0.0413      | 0.0070        | 0.170         | 0.020              |
//! | europe-west1     | 0.0513      | 0.0087        | 0.187         | 0.020              |
//! | europe-west4     | 0.0513      | 0.0087        | 0.187         | 0.020              |
//! | asia-northeast1  | 0.0559      | 0.0095        | 0.221         | 0.023              |
//! | asia-northeast2  | 0.0559      | 0.0095        | 0.221         | 0.023              |
//! | asia-southeast1  | 0.0526      | 0.0089        | 0.187         | 0.023              |

use crate::gcp_model::GcpPricing;

// Uniform global prices for services without meaningful regional variance.
const CLOUD_RUN_REQUEST_PER_MILLION: f64 = 0.40;
const CLOUD_RUN_VCPU_SECOND: f64 = 0.000024;
const CLOUD_RUN_MEMORY_GB_SECOND: f64 = 0.0000025;
// Idle (min-instance) vCPU rate — ~10x lower than the active serving rate.
const CLOUD_RUN_IDLE_VCPU_SECOND: f64 = 0.0000025;
const CLOUD_FUNCTION_INVOCATION_PER_MILLION: f64 = 0.40;
const CLOUD_FUNCTION_GB_SECOND: f64 = 0.0000025;
const BIGQUERY_QUERY_PER_TB: f64 = 5.0;
const PUBSUB_DATA_GB: f64 = 0.04;

struct RegionalSqlPrice {
    vcpu_hour: f64,
    ram_gb_hour: f64,
    ssd_gb_month: f64,
}

#[allow(clippy::struct_field_names)]
struct RegionalStoragePrice {
    standard_gb_month: f64,
    bigquery_active_gb_month: f64,
    nearline_gb_month: f64,
    coldline_gb_month: f64,
    archive_gb_month: f64,
}

#[allow(clippy::match_same_arms)]
fn regional_sql(region: &str) -> RegionalSqlPrice {
    match region {
        "us-central1" | "us-east1" | "us-east4" | "us-west1" | "us-west2" => RegionalSqlPrice {
            vcpu_hour: 0.0413,
            ram_gb_hour: 0.0070,
            ssd_gb_month: 0.170,
        },
        "europe-west1" | "europe-west4" | "europe-north1" => RegionalSqlPrice {
            vcpu_hour: 0.0513,
            ram_gb_hour: 0.0087,
            ssd_gb_month: 0.187,
        },
        "europe-west2" | "europe-west3" | "europe-central2" => RegionalSqlPrice {
            vcpu_hour: 0.0534,
            ram_gb_hour: 0.0090,
            ssd_gb_month: 0.187,
        },
        "asia-east1" | "asia-east2" => RegionalSqlPrice {
            vcpu_hour: 0.0488,
            ram_gb_hour: 0.0083,
            ssd_gb_month: 0.187,
        },
        "asia-northeast1" | "asia-northeast2" | "asia-northeast3" => RegionalSqlPrice {
            vcpu_hour: 0.0559,
            ram_gb_hour: 0.0095,
            ssd_gb_month: 0.221,
        },
        "asia-southeast1" | "asia-southeast2" => RegionalSqlPrice {
            vcpu_hour: 0.0526,
            ram_gb_hour: 0.0089,
            ssd_gb_month: 0.187,
        },
        "asia-south1" | "asia-south2" => RegionalSqlPrice {
            vcpu_hour: 0.0465,
            ram_gb_hour: 0.0079,
            ssd_gb_month: 0.187,
        },
        "australia-southeast1" => RegionalSqlPrice {
            vcpu_hour: 0.0659,
            ram_gb_hour: 0.0112,
            ssd_gb_month: 0.221,
        },
        "southamerica-east1" => RegionalSqlPrice {
            vcpu_hour: 0.0695,
            ram_gb_hour: 0.0118,
            ssd_gb_month: 0.221,
        },
        "me-central1" => RegionalSqlPrice {
            vcpu_hour: 0.0605,
            ram_gb_hour: 0.0103,
            ssd_gb_month: 0.221,
        },
        _ => RegionalSqlPrice {
            vcpu_hour: 0.0559,
            ram_gb_hour: 0.0095,
            ssd_gb_month: 0.221,
        },
    }
}

#[allow(clippy::match_same_arms)]
fn regional_storage(region: &str) -> RegionalStoragePrice {
    match region {
        "us-central1" | "us-east1" | "us-east4" | "us-east5" | "us-west1" | "us-west2"
        | "us-west3" | "us-west4" | "europe-west1" | "europe-west4" | "europe-north1"
        | "asia-east1" | "asia-east2" => RegionalStoragePrice {
            standard_gb_month: 0.020,
            bigquery_active_gb_month: 0.020,
            nearline_gb_month: 0.013,
            coldline_gb_month: 0.006,
            archive_gb_month: 0.0025,
        },
        // Higher-cost Europe, all Asia/Pacific (Standard $0.023)
        "europe-west2"
        | "europe-west3"
        | "europe-west6"
        | "europe-west8"
        | "europe-west9"
        | "europe-central2"
        | "asia-northeast1"
        | "asia-northeast2"
        | "asia-northeast3"
        | "asia-southeast1"
        | "asia-southeast2"
        | "asia-south1"
        | "asia-south2"
        | "australia-southeast1"
        | "australia-southeast2" => RegionalStoragePrice {
            standard_gb_month: 0.023,
            bigquery_active_gb_month: 0.023,
            nearline_gb_month: 0.016,
            coldline_gb_month: 0.006,
            archive_gb_month: 0.0025,
        },
        "southamerica-east1" => RegionalStoragePrice {
            standard_gb_month: 0.035,
            bigquery_active_gb_month: 0.035,
            nearline_gb_month: 0.020,
            coldline_gb_month: 0.007,
            archive_gb_month: 0.0025,
        },
        "me-central1" | "me-west1" => RegionalStoragePrice {
            standard_gb_month: 0.026,
            bigquery_active_gb_month: 0.026,
            nearline_gb_month: 0.019,
            coldline_gb_month: 0.007,
            archive_gb_month: 0.0028,
        },
        _ => RegionalStoragePrice {
            standard_gb_month: 0.023,
            bigquery_active_gb_month: 0.023,
            nearline_gb_month: 0.016,
            coldline_gb_month: 0.006,
            archive_gb_month: 0.0025,
        },
    }
}

/// Returns hardcoded GCP pricing for the given region.
pub fn hardcoded_pricing(region: &str) -> GcpPricing {
    let sql = regional_sql(region);
    let storage = regional_storage(region);

    GcpPricing {
        region: region.to_string(),
        cloud_run_request_per_million: CLOUD_RUN_REQUEST_PER_MILLION,
        cloud_run_vcpu_second: CLOUD_RUN_VCPU_SECOND,
        cloud_run_memory_gb_second: CLOUD_RUN_MEMORY_GB_SECOND,
        cloud_run_idle_vcpu_second: CLOUD_RUN_IDLE_VCPU_SECOND,
        cloud_function_invocation_per_million: CLOUD_FUNCTION_INVOCATION_PER_MILLION,
        cloud_function_gb_second: CLOUD_FUNCTION_GB_SECOND,
        bigquery_active_storage_gb_month: storage.bigquery_active_gb_month,
        bigquery_query_per_tb: BIGQUERY_QUERY_PER_TB,
        cloud_storage_standard_gb_month: storage.standard_gb_month,
        cloud_storage_nearline_gb_month: storage.nearline_gb_month,
        cloud_storage_coldline_gb_month: storage.coldline_gb_month,
        cloud_storage_archive_gb_month: storage.archive_gb_month,
        pubsub_data_gb: PUBSUB_DATA_GB,
        cloud_sql_vcpu_hour: sql.vcpu_hour,
        cloud_sql_ram_gb_hour: sql.ram_gb_hour,
        cloud_sql_ssd_gb_month: sql.ssd_gb_month,
    }
}
