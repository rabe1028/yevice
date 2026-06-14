//! Adapts `GcpPricing` from `yevice-pricing` to the generic `PriceCatalog` trait.
//!
//! GCP currently has no file-backed pricing registry (the AWS Bulk-Pricing
//! equivalent); all prices are sourced from the hardcoded `GcpPricing` table.
//! Following the standard provider pattern documented in
//! `docs/adr/0004-provider-implementation-pattern.md`, a file registry would
//! be an additive optional component; the `PriceCatalog` impl is the
//! mandatory boundary, so callers continue to work either way.

use yevice_core::resource::Provider;
use yevice_pricing::{GcpPricing, PriceCatalog, PricedValue, Sku, error::PricingError};
use yevice_service_api::PriceCatalogResolver;

/// Wraps `GcpPricing` and exposes it as a `PriceCatalog`.
pub struct GcpPricingCatalog(pub GcpPricing);

impl PriceCatalog for GcpPricingCatalog {
    fn region(&self) -> &str {
        &self.0.region
    }

    fn lookup(&self, sku: &Sku) -> Result<PricedValue, PricingError> {
        let key = sku.as_str();
        match key {
            "gcp.cloud_function.invocation_per_million" => Ok(PricedValue::scalar(
                self.0.cloud_function_invocation_per_million,
                "USD",
            )),
            "gcp.cloud_function.gb_second" => {
                Ok(PricedValue::scalar(self.0.cloud_function_gb_second, "USD"))
            }
            "gcp.cloud_run.request_per_million" => Ok(PricedValue::scalar(
                self.0.cloud_run_request_per_million,
                "USD",
            )),
            "gcp.cloud_run.vcpu_second" => {
                Ok(PricedValue::scalar(self.0.cloud_run_vcpu_second, "USD"))
            }
            "gcp.cloud_run.memory_gb_second" => Ok(PricedValue::scalar(
                self.0.cloud_run_memory_gb_second,
                "USD",
            )),
            "gcp.cloud_run.idle_vcpu_second" => Ok(PricedValue::scalar(
                self.0.cloud_run_idle_vcpu_second,
                "USD",
            )),
            "gcp.bigquery.active_storage_gb_month" => Ok(PricedValue::scalar(
                self.0.bigquery_active_storage_gb_month,
                "USD",
            )),
            "gcp.bigquery.query_per_tb" => {
                Ok(PricedValue::scalar(self.0.bigquery_query_per_tb, "USD"))
            }
            "gcp.cloud_storage.standard_gb_month" => Ok(PricedValue::scalar(
                self.0.cloud_storage_standard_gb_month,
                "USD",
            )),
            "gcp.cloud_storage.nearline_gb_month" => Ok(PricedValue::scalar(
                self.0.cloud_storage_nearline_gb_month,
                "USD",
            )),
            "gcp.cloud_storage.coldline_gb_month" => Ok(PricedValue::scalar(
                self.0.cloud_storage_coldline_gb_month,
                "USD",
            )),
            "gcp.cloud_storage.archive_gb_month" => Ok(PricedValue::scalar(
                self.0.cloud_storage_archive_gb_month,
                "USD",
            )),
            "gcp.pubsub.data_gb" => Ok(PricedValue::scalar(self.0.pubsub_data_gb, "USD")),
            "gcp.cloud_sql.vcpu_hour" => Ok(PricedValue::scalar(self.0.cloud_sql_vcpu_hour, "USD")),
            "gcp.cloud_sql.ram_gb_hour" => {
                Ok(PricedValue::scalar(self.0.cloud_sql_ram_gb_hour, "USD"))
            }
            "gcp.cloud_sql.ssd_gb_month" => {
                Ok(PricedValue::scalar(self.0.cloud_sql_ssd_gb_month, "USD"))
            }
            _ => Err(PricingError::NotFound {
                service: key.to_string(),
                region: self.0.region.clone(),
            }),
        }
    }
}

impl PriceCatalogResolver for GcpPricingCatalog {
    fn resolve(&self, provider: Provider) -> Option<&dyn PriceCatalog> {
        (provider == Provider::Gcp).then_some(self as &dyn PriceCatalog)
    }
}
