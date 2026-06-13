//! A no-op pricing catalog for providers whose services compute costs inline.
//!
//! The [`NoopCatalog`] is used for providers such as Cloudflare whose services
//! price themselves inline and never call into a `PriceCatalog`. The `lookup`
//! method returns a `NotFound` error as a safety net in case it is called
//! unexpectedly.

use crate::{
    catalog::{PriceCatalog, PricedValue, Sku},
    error::PricingError,
};

/// A no-op pricing catalog.
///
/// Every `lookup` call returns [`PricingError::NotFound`]. Intended for
/// providers whose services never invoke the catalog (e.g. Cloudflare).
pub struct NoopCatalog;

impl PriceCatalog for NoopCatalog {
    fn region(&self) -> &'static str {
        "global"
    }

    fn lookup(&self, sku: &Sku) -> Result<PricedValue, PricingError> {
        Err(PricingError::NotFound {
            service: sku.as_str().to_string(),
            region: "global".to_string(),
        })
    }
}
