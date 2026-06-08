//! Adapts `GcpPricing` from `yevice-pricing` to the generic `PriceCatalog` trait.

use yevice_pricing::{GcpPricing, PriceCatalog, PriceRecord, Sku, error::PricingError};

/// Wraps `GcpPricing` and exposes it as a `PriceCatalog`.
pub struct GcpPricingCatalog(pub GcpPricing);

impl PriceCatalog for GcpPricingCatalog {
    fn region(&self) -> &str {
        &self.0.region
    }

    fn lookup(&self, sku: &Sku) -> Result<PriceRecord, PricingError> {
        let key = sku.as_str();
        match key {
            "gcp.cloud_function.invocation_per_million" => Ok(PriceRecord::flat(
                self.0.cloud_function_invocation_per_million,
            )),
            "gcp.cloud_function.gb_second" => {
                Ok(PriceRecord::flat(self.0.cloud_function_gb_second))
            }
            "gcp.cloud_run.request_per_million" => {
                Ok(PriceRecord::flat(self.0.cloud_run_request_per_million))
            }
            "gcp.cloud_run.vcpu_second" => Ok(PriceRecord::flat(self.0.cloud_run_vcpu_second)),
            "gcp.cloud_run.memory_gb_second" => {
                Ok(PriceRecord::flat(self.0.cloud_run_memory_gb_second))
            }
            "gcp.cloud_run.idle_vcpu_second" => {
                Ok(PriceRecord::flat(self.0.cloud_run_idle_vcpu_second))
            }
            "gcp.bigquery.active_storage_gb_month" => {
                Ok(PriceRecord::flat(self.0.bigquery_active_storage_gb_month))
            }
            "gcp.bigquery.query_per_tb" => Ok(PriceRecord::flat(self.0.bigquery_query_per_tb)),
            "gcp.cloud_storage.standard_gb_month" => {
                Ok(PriceRecord::flat(self.0.cloud_storage_standard_gb_month))
            }
            "gcp.cloud_storage.nearline_gb_month" => {
                Ok(PriceRecord::flat(self.0.cloud_storage_nearline_gb_month))
            }
            "gcp.cloud_storage.coldline_gb_month" => {
                Ok(PriceRecord::flat(self.0.cloud_storage_coldline_gb_month))
            }
            "gcp.cloud_storage.archive_gb_month" => {
                Ok(PriceRecord::flat(self.0.cloud_storage_archive_gb_month))
            }
            "gcp.pubsub.data_gb" => Ok(PriceRecord::flat(self.0.pubsub_data_gb)),
            "gcp.cloud_sql.vcpu_hour" => Ok(PriceRecord::flat(self.0.cloud_sql_vcpu_hour)),
            "gcp.cloud_sql.ram_gb_hour" => Ok(PriceRecord::flat(self.0.cloud_sql_ram_gb_hour)),
            "gcp.cloud_sql.ssd_gb_month" => Ok(PriceRecord::flat(self.0.cloud_sql_ssd_gb_month)),
            _ => Err(PricingError::NotFound {
                service: key.to_string(),
                region: self.0.region.clone(),
            }),
        }
    }
}
