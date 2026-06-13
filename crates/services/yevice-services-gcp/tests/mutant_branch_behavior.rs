use yevice_core::{
    evaluate::{Params, evaluate},
    expr::Expr,
    types::{LogicalId, ResourceType, VariableName},
};
use yevice_pricing::{
    catalog::{PriceCatalog, PricedValue, Sku},
    error::PricingError,
};
use yevice_service_api::Service;
use yevice_services_gcp::services::{
    cloud_sql::{GcpCloudSqlService, GcpCloudSqlSpec},
    cloud_storage::{GcpCloudStorageService, GcpCloudStorageSpec},
};

struct BranchCatalog;

impl PriceCatalog for BranchCatalog {
    fn region(&self) -> &'static str {
        "test"
    }

    fn lookup(&self, sku: &Sku) -> Result<PricedValue, PricingError> {
        let price = match sku.as_str() {
            "gcp.cloud_sql.vcpu_hour" => 2.3,
            "gcp.cloud_sql.ram_gb_hour" => 3.7,
            "gcp.cloud_sql.ssd_gb_month" => 4.9,
            "gcp.cloud_storage.standard_gb_month" => 2.2,
            "gcp.cloud_storage.nearline_gb_month" => 3.3,
            "gcp.cloud_storage.coldline_gb_month" => 4.4,
            "gcp.cloud_storage.archive_gb_month" => 5.5,
            other => {
                return Err(PricingError::NotFound {
                    service: other.to_string(),
                    region: "test".to_string(),
                });
            }
        };

        Ok(PricedValue::scalar(price, "USD"))
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

fn cloud_sql_instance_monthly(vcpu: f64, ram_gb: f64, ha_multiplier: f64) -> f64 {
    (vcpu * 2.3 + ram_gb * 3.7) * 730.0 * ha_multiplier
}

#[test]
fn service_ids_match_expected_catalog_keys() {
    assert_eq!(GcpCloudSqlService.id(), "gcp.cloud_sql");
    assert_eq!(GcpCloudStorageService.id(), "gcp.cloud_storage");
}

#[test]
fn cloud_storage_cost_covers_all_storage_class_arms() {
    let id = LogicalId::new("bucket");
    let storage = params([(id.var("storage_gb"), 7.0)]);

    for (storage_class, expected_label, expected_rate) in [
        (None, "STANDARD", 2.2),
        (Some("nearline"), "NEARLINE", 3.3),
        (Some("coldline"), "COLDLINE", 4.4),
        (Some("archive"), "ARCHIVE", 5.5),
    ] {
        let spec = GcpCloudStorageSpec {
            storage_class: storage_class.map(str::to_string),
            location_type: Some("dual_region".to_string()),
        };
        let cost = GcpCloudStorageService
            .build_cost(&id, &rt("google_storage_bucket"), &spec, &BranchCatalog)
            .expect("build cost");

        assert_eq!(cost.label, format!("Cloud Storage ({expected_label})"));
        assert_eq!(
            cost.components[0].name,
            format!("Storage ({expected_label})")
        );
        approx_expr(&cost.expr, &storage, 7.0 * expected_rate);
        approx_expr(&cost.components[0].expr, &storage, 7.0 * expected_rate);
    }
}

#[test]
fn cloud_sql_cost_labels_cover_regional_and_zonal_branches() {
    let id = LogicalId::new("sql");
    let storage = params([(id.var("storage_gb"), 6.0)]);

    let zonal = GcpCloudSqlService
        .build_cost(
            &id,
            &rt("google_sql_database_instance"),
            &GcpCloudSqlSpec {
                tier: "db-n1-standard-2".to_string(),
                database_version: "POSTGRES_16".to_string(),
                disk_size_gb: None,
                availability_type: "ZONAL".to_string(),
            },
            &BranchCatalog,
        )
        .expect("build cost");
    assert_eq!(zonal.label, "Cloud SQL POSTGRES_16 (db-n1-standard-2)");
    assert_eq!(zonal.components[0].name, "Instance (db-n1-standard-2)");
    approx_expr(
        &zonal.components[0].expr,
        &storage,
        cloud_sql_instance_monthly(2.0, 7.5, 1.0),
    );
    approx_expr(&zonal.components[1].expr, &storage, 29.4);

    let regional = GcpCloudSqlService
        .build_cost(
            &id,
            &rt("google_sql_database_instance"),
            &GcpCloudSqlSpec {
                tier: "db-n1-standard-2".to_string(),
                database_version: "POSTGRES_16".to_string(),
                disk_size_gb: Some(9.0),
                availability_type: "regional".to_string(),
            },
            &BranchCatalog,
        )
        .expect("build cost");
    assert_eq!(
        regional.label,
        "Cloud SQL POSTGRES_16 (db-n1-standard-2 HA)"
    );
    assert_eq!(
        regional.components[0].name,
        "Instance (db-n1-standard-2 HA)"
    );
    approx_expr(
        &regional.components[0].expr,
        &storage,
        cloud_sql_instance_monthly(2.0, 7.5, 2.0),
    );
    approx_expr(&regional.components[1].expr, &storage, 29.4);
}

#[test]
fn cloud_sql_cost_covers_named_and_custom_tier_match_arms() {
    let id = LogicalId::new("sql");
    let storage = params([(id.var("storage_gb"), 6.0)]);

    for (tier, expected_vcpu, expected_ram_gb) in [
        ("db-f1-micro", 0.2, 0.6),
        ("db-g1-small", 0.5, 1.7),
        ("db-n1-standard-4", 4.0, 15.0),
        ("db-n1-highmem-6", 6.0, 39.0),
        ("db-n1-highcpu-8", 8.0, 7.2),
        ("db-custom-6-5632", 6.0, 5.5),
    ] {
        let cost = GcpCloudSqlService
            .build_cost(
                &id,
                &rt("google_sql_database_instance"),
                &GcpCloudSqlSpec {
                    tier: tier.to_string(),
                    database_version: "MYSQL_8_0".to_string(),
                    disk_size_gb: Some(12.0),
                    availability_type: "ZONAL".to_string(),
                },
                &BranchCatalog,
            )
            .expect("build cost");

        assert_eq!(cost.label, format!("Cloud SQL MYSQL_8_0 ({tier})"));
        approx_expr(
            &cost.components[0].expr,
            &storage,
            cloud_sql_instance_monthly(expected_vcpu, expected_ram_gb, 1.0),
        );
        approx_expr(&cost.components[1].expr, &storage, 29.4);
    }
}

#[test]
fn cloud_sql_unknown_four_segment_tier_uses_fallback_arm() {
    let id = LogicalId::new("sql");
    let storage = params([(id.var("storage_gb"), 6.0)]);
    let tier = "mystery-plan-6-5632";
    let cost = GcpCloudSqlService
        .build_cost(
            &id,
            &rt("google_sql_database_instance"),
            &GcpCloudSqlSpec {
                tier: tier.to_string(),
                database_version: "MYSQL_8_0".to_string(),
                disk_size_gb: Some(12.0),
                availability_type: "ZONAL".to_string(),
            },
            &BranchCatalog,
        )
        .expect("build cost");

    assert_eq!(cost.label, format!("Cloud SQL MYSQL_8_0 ({tier})"));
    approx_expr(
        &cost.components[0].expr,
        &storage,
        cloud_sql_instance_monthly(1.0, 3.75, 1.0),
    );
    approx_expr(&cost.components[1].expr, &storage, 29.4);
}
