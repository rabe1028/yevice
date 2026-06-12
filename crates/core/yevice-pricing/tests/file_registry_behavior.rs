use std::path::PathBuf;

use yevice_pricing::file_registry::FilePricingRegistry;

fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-12,
        "expected {expected}, got {actual}"
    );
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/file-registry")
}

#[test]
fn file_registry_uses_loaded_lambda_prices_instead_of_fallback_defaults() {
    let registry = FilePricingRegistry::load("ap-northeast-1", fixture_dir());
    let price = registry.lambda_price();

    assert_close(price.request_price, 0.0000009);
    assert_close(price.gb_second_price, 0.0000999);
    assert_close(price.free_tier_requests, 1_000_000.0);
    assert_close(price.free_tier_gb_seconds, 400_000.0);
}

/// The fixture rds.json contains a "Database Storage" SKU with
/// `volumeType = "General Purpose-GP3"` and price $0.1300, and a
/// "System Operation" SKU with `group = "RDS-GP3-IOPS"` and price $0.0090.
/// Both prices differ from the hardcoded ap-northeast-1 constants
/// (0.1216 and 0.008), so if the file-backed lookup falls back to hardcoded
/// values the assertions will fail.
#[test]
fn file_registry_returns_gp3_prices_from_rds_json_not_hardcoded_fallback() {
    let registry = FilePricingRegistry::load("us-east-1", fixture_dir());

    let storage_price = registry.rds_gp3_storage_price();
    assert_close(storage_price, 0.1300);

    let iops_price = registry.rds_gp3_iops_price();
    assert_close(iops_price, 0.0090);
}

#[test]
fn file_registry_maps_rds_engine_aliases_to_bulk_api_database_engine_names() {
    let registry = FilePricingRegistry::load("ap-northeast-1", fixture_dir());

    let mariadb = registry.rds_price("db.t3.small", "mariadb").unwrap();
    assert_close(mariadb.hourly_price, 0.111);
    assert_close(mariadb.storage_price_per_gb, 0.138);

    let postgres = registry.rds_price("db.t3.medium", "postgres").unwrap();
    assert_close(postgres.hourly_price, 0.222);
    assert_close(postgres.storage_price_per_gb, 0.138);

    let aurora_mysql = registry.rds_price("db.r5.large", "aurora-mysql").unwrap();
    assert_close(aurora_mysql.hourly_price, 0.333);
    assert_close(aurora_mysql.storage_price_per_gb, 0.138);

    let aurora_postgresql = registry
        .rds_price("db.r5.xlarge", "aurora-postgresql")
        .unwrap();
    assert_close(aurora_postgresql.hourly_price, 0.444);
    assert_close(aurora_postgresql.storage_price_per_gb, 0.138);
}
