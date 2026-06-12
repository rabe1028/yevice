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

/// The fixture rds.json deliberately includes both Single-AZ ($0.1300, $0.0090)
/// and Multi-AZ ($0.2600, $0.0180) GP3 rows.  The lookup must select the
/// Single-AZ row so that RdsService can apply az_mult externally.
#[test]
fn file_registry_gp3_lookup_selects_single_az_row_not_multi_az() {
    let registry = FilePricingRegistry::load("us-east-1", fixture_dir());

    let storage_price = registry.rds_gp3_storage_price();
    assert_close(storage_price, 0.1300);
    assert!(
        (storage_price - 0.2600).abs() > 1e-6,
        "must not return the Multi-AZ storage price ($0.2600)"
    );

    let iops_price = registry.rds_gp3_iops_price();
    assert_close(iops_price, 0.0090);
    assert!(
        (iops_price - 0.0180).abs() > 1e-6,
        "must not return the Multi-AZ IOPS price ($0.0180)"
    );
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

/// Metadata is populated from top-level fields in the Bulk API JSON.
/// The fixture lambda.json now carries `publicationDate` and `version`.
#[test]
fn metadata_returns_publication_date_and_version_from_fixture() {
    let registry = FilePricingRegistry::load("ap-northeast-1", fixture_dir());

    let meta = registry
        .metadata("lambda")
        .expect("lambda metadata should be present");
    assert_eq!(meta.service_key, "lambda");
    assert_eq!(
        meta.publication_date.as_deref(),
        Some("2023-10-15T00:00:00Z"),
        "publication_date must match the fixture value"
    );
    assert_eq!(
        meta.version.as_deref(),
        Some("20231015"),
        "version must match the fixture value"
    );
    assert_eq!(meta.region, "ap-northeast-1");
    assert_eq!(meta.currency, "USD");
}

/// all_metadata returns one entry per successfully loaded service file.
#[test]
fn all_metadata_returns_one_entry_per_loaded_service() {
    let registry = FilePricingRegistry::load("ap-northeast-1", fixture_dir());
    // Fixture dir only has lambda.json and rds.json.
    let all = registry.all_metadata();
    assert_eq!(all.len(), 2, "expected exactly two loaded services");

    let keys: Vec<&str> = {
        let mut v: Vec<&str> = all.iter().map(|m| m.service_key.as_str()).collect();
        v.sort_unstable();
        v
    };
    assert_eq!(keys, ["lambda", "rds"]);
}

/// metadata returns None for a service whose file was not loaded.
#[test]
fn metadata_returns_none_for_missing_service() {
    let registry = FilePricingRegistry::load("ap-northeast-1", fixture_dir());
    assert!(
        registry.metadata("ec2").is_none(),
        "ec2 has no fixture file; metadata must be None"
    );
}

/// A Bulk API JSON without publicationDate/version must still parse successfully,
/// with those fields returning None in the metadata.
#[test]
fn metadata_fields_are_none_for_json_without_top_level_fields() {
    let dir = {
        let d = std::env::temp_dir().join(format!("yevice_meta_test_{}", std::process::id()));
        std::fs::create_dir_all(&d).expect("create temp dir");
        d
    };
    // Minimal JSON: no publicationDate, no version.
    let minimal = r#"{
        "offerCode": "AWSLambda",
        "products": {
            "SKU1": {
                "sku": "SKU1",
                "productFamily": "Serverless",
                "attributes": {"group": "AWS-Lambda-Requests"}
            }
        },
        "terms": {
            "OnDemand": {
                "SKU1": {
                    "SKU1.TERM": {
                        "sku": "SKU1",
                        "priceDimensions": {
                            "SKU1.DIM": {
                                "description": "per req",
                                "beginRange": "0",
                                "endRange": "Inf",
                                "unit": "Requests",
                                "pricePerUnit": {"USD": "0.0000002"}
                            }
                        }
                    }
                }
            }
        }
    }"#;
    std::fs::write(dir.join("lambda.json"), minimal).unwrap();

    let registry = FilePricingRegistry::load("us-east-1", &dir);
    let meta = registry
        .metadata("lambda")
        .expect("lambda metadata should be present even without top-level fields");
    assert!(
        meta.publication_date.is_none(),
        "publication_date must be None when absent from JSON"
    );
    assert!(
        meta.version.is_none(),
        "version must be None when absent from JSON"
    );
    assert_eq!(meta.currency, "USD");

    let _ = std::fs::remove_dir_all(&dir);
}
