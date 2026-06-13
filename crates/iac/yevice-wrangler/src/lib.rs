//! Cloudflare Workers configuration parser and service plugin implementations.
//!
//! Parses Wrangler config files into a yevice-core `Architecture`, and provides
//! `Service` implementations for all Cloudflare resource types.

pub mod error;
pub mod parser;
pub mod plugin;
pub mod services;

pub use error::WranglerError;
pub use parser::{parse_wrangler, parse_wrangler_str, parse_wrangler_with_policy};
pub use plugin::CloudflarePlugin;

/// Register all Cloudflare services into the given catalog.
pub fn register(catalog: &mut yevice_service_api::ServiceCatalog) {
    catalog.register(services::CloudflareWorkerService);
    catalog.register(services::CloudflareKvService);
    catalog.register(services::CloudflareR2Service);
    catalog.register(services::CloudflareD1Service);
    catalog.register(services::CloudflareQueueService);
    catalog.register(services::CloudflareDurableObjectService);
}
