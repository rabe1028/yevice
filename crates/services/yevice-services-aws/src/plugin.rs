//! [`ProviderPlugin`] implementation for AWS.

use yevice_core::resource::Provider;
use yevice_pricing::catalog::PriceCatalog;
use yevice_service_api::{ProviderPlugin, Registration};

use crate::pricing_adapter::AwsPricingCatalog;

/// Provider plugin for AWS.
pub struct AwsPlugin;

impl ProviderPlugin for AwsPlugin {
    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn register(&self, reg: &mut Registration<'_>) {
        crate::register(reg.catalog, reg.cfn_adapters, reg.tf_adapters);
    }

    fn pricing_catalog(&self, region: &str, list_price: bool) -> Box<dyn PriceCatalog> {
        Box::new(AwsPricingCatalog::auto(region).with_list_price(list_price))
    }
}
