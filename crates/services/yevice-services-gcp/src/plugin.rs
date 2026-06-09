//! [`ProviderPlugin`] implementation for GCP.

use yevice_core::resource::Provider;
use yevice_pricing::{catalog::PriceCatalog, gcp_hardcoded_pricing};
use yevice_service_api::{ProviderPlugin, Registration};

use crate::pricing_adapter::GcpPricingCatalog;

/// Provider plugin for GCP.
pub struct GcpPlugin;

impl ProviderPlugin for GcpPlugin {
    fn provider(&self) -> Provider {
        Provider::Gcp
    }

    fn register(&self, reg: &mut Registration<'_>) {
        crate::register(reg.catalog, reg.tf_adapters);
    }

    fn pricing_catalog(&self, region: &str, _list_price: bool) -> Box<dyn PriceCatalog> {
        Box::new(GcpPricingCatalog(gcp_hardcoded_pricing(region)))
    }
}
