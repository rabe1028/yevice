//! Absolute-amount tests for service `build_cost` formulas.
//!
//! These pin the exact monetary result of each service's cost expression given
//! round-number test prices and explicit variable bindings, so that mutations
//! to the cost arithmetic are caught.

use yevice_core::{
    evaluate::{Params, evaluate},
    expr::Expr,
    types::{LogicalId, ResourceType, VariableName},
};
use yevice_pricing::{
    catalog::{PriceCatalog, PriceRecord, Sku},
    error::PricingError,
};
use yevice_service_api::Service;

use yevice_services_gcp::services::{
    bigquery::{GcpBigQueryService, GcpBigQuerySpec},
    cloud_function::{GcpCloudFunctionService, GcpCloudFunctionSpec},
    cloud_run::{GcpCloudRunService, GcpCloudRunSpec},
    cloud_sql::{GcpCloudSqlService, GcpCloudSqlSpec},
    cloud_storage::{GcpCloudStorageService, GcpCloudStorageSpec},
    pubsub::{GcpPubSubService, GcpPubSubSpec},
};

struct TestCatalog;

impl PriceCatalog for TestCatalog {
    fn region(&self) -> &'static str {
        "test"
    }

    fn lookup(&self, sku: &Sku) -> Result<PriceRecord, PricingError> {
        let price = match sku.as_str() {
            "gcp.bigquery.active_storage_gb_month" => PriceRecord::flat(2.0),
            "gcp.bigquery.query_per_tb" => PriceRecord::flat(1_000.0),
            "gcp.cloud_function.invocation_per_million" => PriceRecord::flat(1_000_000.0),
            "gcp.cloud_function.gb_second" => PriceRecord::flat(2.0),
            "gcp.cloud_run.request_per_million" => PriceRecord::flat(1_000_000.0),
            "gcp.cloud_run.vcpu_second" => PriceRecord::flat(2.0),
            "gcp.cloud_run.memory_gb_second" => PriceRecord::flat(3.0),
            "gcp.cloud_run.idle_vcpu_second" => PriceRecord::flat(0.2),
            "gcp.cloud_sql.vcpu_hour" => PriceRecord::flat(1.0),
            "gcp.cloud_sql.ram_gb_hour" => PriceRecord::flat(2.0),
            "gcp.cloud_sql.ssd_gb_month" => PriceRecord::flat(3.0),
            "gcp.cloud_storage.standard_gb_month" => PriceRecord::flat(1.0),
            "gcp.cloud_storage.nearline_gb_month" => PriceRecord::flat(2.0),
            "gcp.cloud_storage.coldline_gb_month" => PriceRecord::flat(3.0),
            "gcp.cloud_storage.archive_gb_month" => PriceRecord::flat(4.0),
            "gcp.pubsub.data_gb" => PriceRecord::flat(2.0),
            other => {
                return Err(PricingError::NotFound {
                    service: other.to_string(),
                    region: "test".to_string(),
                });
            }
        };
        Ok(price)
    }
}

#[track_caller]
fn approx(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-9,
        "expected {expected}, got {actual}"
    );
}

#[track_caller]
fn approx_expr(expr: &Expr, params: &Params, expected: f64) {
    approx(evaluate(expr, params).expect("eval"), expected);
}

fn rt(name: &str) -> ResourceType {
    ResourceType::new(name)
}

fn params<const N: usize>(entries: [(VariableName, f64); N]) -> Params {
    entries.into_iter().collect()
}

#[test]
fn bigquery_cost_is_storage_plus_scanned_queries() {
    let id = LogicalId::new("bq");
    let cost = GcpBigQueryService
        .build_cost(
            &id,
            &rt("google_bigquery_dataset"),
            &GcpBigQuerySpec {},
            &TestCatalog,
        )
        .expect("build cost");

    let params = params([
        (id.var("storage_gb"), 10.0),
        (id.var("query_gb_scanned"), 1500.0),
    ]);

    approx_expr(&cost.expr, &params, 520.0);
    approx_expr(&cost.components[0].expr, &params, 20.0);
    approx_expr(&cost.components[1].expr, &params, 500.0);
}

#[test]
fn cloud_function_cost_is_invocations_plus_compute_over_free_tiers() {
    let id = LogicalId::new("function");
    let spec = GcpCloudFunctionSpec {
        memory_mb: 1024.0,
        generation: 2,
    };
    let cost = GcpCloudFunctionService
        .build_cost(
            &id,
            &rt("google_cloudfunctions2_function"),
            &spec,
            &TestCatalog,
        )
        .expect("build cost");

    let params = params([
        (id.var("monthly_invocations"), 2_000_100.0),
        (id.var("avg_duration_ms"), 200.0),
    ]);

    approx_expr(&cost.expr, &params, 140.0);
    approx_expr(&cost.components[0].expr, &params, 100.0);
    approx_expr(&cost.components[1].expr, &params, 40.0);
}

#[test]
fn cloud_run_cost_is_requests_vcpu_and_memory() {
    let id = LogicalId::new("run");
    let spec = GcpCloudRunSpec {
        cpu: 1.0,
        memory_mb: 1024.0,
        min_instances: None,
    };
    let cost = GcpCloudRunService
        .build_cost(&id, &rt("google_cloud_run_v2_service"), &spec, &TestCatalog)
        .expect("build cost");

    let params = params([
        (id.var("monthly_requests"), 2_000_100.0),
        (id.var("avg_duration_ms"), 200.0),
    ]);

    approx_expr(&cost.expr, &params, 560_200.0);
    approx_expr(&cost.components[0].expr, &params, 100.0);
    approx_expr(&cost.components[1].expr, &params, 440_040.0);
    approx_expr(&cost.components[2].expr, &params, 120_060.0);
}

#[test]
fn cloud_sql_cost_is_instance_plus_storage() {
    let id = LogicalId::new("sql");
    let spec = GcpCloudSqlSpec {
        tier: "db-n1-standard-2".to_string(),
        database_version: "POSTGRES_15".to_string(),
        disk_size_gb: None,
        availability_type: "REGIONAL".to_string(),
    };
    let cost = GcpCloudSqlService
        .build_cost(
            &id,
            &rt("google_sql_database_instance"),
            &spec,
            &TestCatalog,
        )
        .expect("build cost");

    let params = params([(id.var("storage_gb"), 10.0)]);

    approx_expr(&cost.expr, &params, 24_850.0);
    approx_expr(&cost.components[0].expr, &params, 24_820.0);
    approx_expr(&cost.components[1].expr, &params, 30.0);
}

#[test]
fn cloud_storage_archive_cost_uses_selected_storage_class_rate() {
    let id = LogicalId::new("bucket");
    let spec = GcpCloudStorageSpec {
        storage_class: Some("ARCHIVE".to_string()),
        location_type: Some("MULTI_REGION".to_string()),
    };
    let cost = GcpCloudStorageService
        .build_cost(&id, &rt("google_storage_bucket"), &spec, &TestCatalog)
        .expect("build cost");

    let params = params([(id.var("storage_gb"), 10.0)]);

    approx_expr(&cost.expr, &params, 40.0);
    approx_expr(&cost.components[0].expr, &params, 40.0);
}

#[test]
fn pubsub_cost_matches_data_volume_over_free_tier() {
    let id = LogicalId::new("topic");
    let cost = GcpPubSubService
        .build_cost(
            &id,
            &rt("google_pubsub_topic"),
            &GcpPubSubSpec {},
            &TestCatalog,
        )
        .expect("build cost");

    let params = params([(id.var("data_gb"), 25.0)]);

    approx_expr(&cost.expr, &params, 30.0);
    approx_expr(&cost.components[0].expr, &params, 30.0);
}
