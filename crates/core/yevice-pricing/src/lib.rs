//! Provider-neutral pricing primitives.
//!
//! The cross-provider abstraction is [`catalog::PriceCatalog`]; every
//! [`ProviderPlugin`](../yevice_service_api/trait.ProviderPlugin.html)
//! returns its own implementation. The remaining modules are AWS-shaped
//! internals (Bulk API parser, hardcoded fallback registry, downloaded-file
//! registry, AWS price-model structs) used by `yevice-services-aws`. They
//! live here so the download/parse plumbing is not duplicated, but no
//! cross-provider code path depends on them — provider-neutral access goes
//! through [`PriceCatalog`].
//!
//! The AWS-specific `PricingProvider` trait that exposed those internals lives
//! in `yevice-services-aws::pricing_provider`. See
//! `docs/adr/0004-provider-implementation-pattern.md`.

pub mod bulk_api;
pub mod catalog;
pub mod download;
pub mod error;
pub mod file_registry;
pub mod gcp_model;
pub mod gcp_registry;
pub mod model;
pub mod noop;
pub mod registry;

pub use catalog::{PriceCatalog, PriceRecord, Sku};
pub use file_registry::PricingMetadata;
pub use gcp_model::GcpPricing;
pub use gcp_registry::hardcoded_pricing as gcp_hardcoded_pricing;
pub use noop::NoopCatalog;
