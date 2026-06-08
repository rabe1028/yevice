use yevice_core::cost::Tier;
use yevice_pricing::{
    PriceRecord, Sku, catalog::PriceCatalog, error::PricingError, gcp_hardcoded_pricing,
};

fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-12,
        "expected {expected}, got {actual}"
    );
}

struct LookupCatalog;

impl PriceCatalog for LookupCatalog {
    fn region(&self) -> &'static str {
        "test"
    }

    fn lookup(&self, sku: &Sku) -> Result<PriceRecord, PricingError> {
        match sku.as_str() {
            "flat" => Ok(PriceRecord::flat(2.7)),
            "tiered" => Ok(PriceRecord::tiered(vec![
                Tier {
                    upper_limit: Some(4.0),
                    unit_price: 0.2,
                },
                Tier {
                    upper_limit: None,
                    unit_price: 0.6,
                },
            ])),
            other => Err(PricingError::NotFound {
                service: other.to_string(),
                region: "test".to_string(),
            }),
        }
    }
}

#[test]
fn lookup_f64_uses_catalog_lookup_result_instead_of_stub_values() {
    assert_close(LookupCatalog.lookup_f64(&Sku::new("flat")).unwrap(), 2.7);

    let err = LookupCatalog.lookup_f64(&Sku::new("tiered")).unwrap_err();
    assert!(matches!(err, PricingError::NotFound { .. }));
}

#[test]
fn gcp_hardcoded_pricing_keeps_asia_northeast_and_high_cost_storage_groups() {
    let asia_northeast = gcp_hardcoded_pricing("asia-northeast2");
    assert_eq!(asia_northeast.region, "asia-northeast2");
    assert_close(asia_northeast.cloud_sql_vcpu_hour, 0.0559);
    assert_close(asia_northeast.cloud_sql_ram_gb_hour, 0.0095);
    assert_close(asia_northeast.cloud_sql_ssd_gb_month, 0.221);
    assert_close(asia_northeast.cloud_storage_standard_gb_month, 0.023);
    assert_close(asia_northeast.cloud_storage_nearline_gb_month, 0.016);
    assert_close(asia_northeast.cloud_storage_archive_gb_month, 0.0025);

    let australia = gcp_hardcoded_pricing("australia-southeast2");
    assert_eq!(australia.region, "australia-southeast2");
    assert_close(australia.cloud_sql_vcpu_hour, 0.0559);
    assert_close(australia.cloud_sql_ram_gb_hour, 0.0095);
    assert_close(australia.cloud_sql_ssd_gb_month, 0.221);
    assert_close(australia.cloud_storage_standard_gb_month, 0.023);
    assert_close(australia.cloud_storage_nearline_gb_month, 0.016);
    assert_close(australia.cloud_storage_archive_gb_month, 0.0025);

    let defaulted = gcp_hardcoded_pricing("custom-region-9");
    assert_eq!(defaulted.region, "custom-region-9");
    assert_close(defaulted.cloud_sql_vcpu_hour, 0.0559);
    assert_close(defaulted.cloud_sql_ram_gb_hour, 0.0095);
    assert_close(defaulted.cloud_sql_ssd_gb_month, 0.221);
    assert_close(defaulted.cloud_storage_standard_gb_month, 0.023);
    assert_close(defaulted.cloud_storage_nearline_gb_month, 0.016);
    assert_close(defaulted.cloud_storage_archive_gb_month, 0.0025);
}
