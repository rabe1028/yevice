//! [`ProviderPlugin`] implementation for Cloudflare.

use yevice_core::resource::Provider;
use yevice_pricing::{NoopCatalog, catalog::PriceCatalog};
use yevice_service_api::{ProviderPlugin, Registration};

/// Provider plugin for Cloudflare.
pub struct CloudflarePlugin;

impl ProviderPlugin for CloudflarePlugin {
    fn provider(&self) -> Provider {
        Provider::Cloudflare
    }

    fn register(&self, reg: &mut Registration<'_>) {
        crate::register(reg.catalog);
    }

    fn pricing_catalog(&self, _region: &str, _list_price: bool) -> Box<dyn PriceCatalog> {
        Box::new(NoopCatalog)
    }
}
