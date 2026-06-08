//! Generic price catalog trait.
//!
//! `PriceCatalog` decouples service plugins from any specific pricing
//! provider implementation (file-based registry, bulk API, mock, etc.).

use std::borrow::Cow;

use crate::error::PricingError;
use yevice_core::cost::Tier;

/// A lookup key for a price record.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Sku(pub Cow<'static, str>);

impl Sku {
    /// Create a SKU from a static string (zero-allocation).
    pub const fn new(s: &'static str) -> Self {
        Self(Cow::Borrowed(s))
    }

    /// Create a SKU from a dynamic string.
    pub fn dynamic(s: impl Into<String>) -> Self {
        Self(Cow::Owned(s.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Sku {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A price record returned by a `PriceCatalog`.
#[derive(Debug, Clone)]
pub enum PriceRecord {
    /// A flat price per unit.
    Flat { value: f64 },
    /// A tiered price structure.
    Tiered { tiers: Vec<Tier> },
}

impl PriceRecord {
    /// Create a flat price record.
    pub fn flat(value: f64) -> Self {
        Self::Flat { value }
    }

    /// Create a tiered price record.
    pub fn tiered(tiers: Vec<Tier>) -> Self {
        Self::Tiered { tiers }
    }

    /// Extract the flat value, or return an error for tiered records.
    pub fn as_flat(&self) -> Result<f64, PricingError> {
        match self {
            Self::Flat { value } => Ok(*value),
            Self::Tiered { .. } => Err(PricingError::NotFound {
                service: "expected flat price but got tiered".into(),
                region: String::new(),
            }),
        }
    }

    /// Extract the tiers, or return an error for flat records.
    pub fn as_tiered(&self) -> Result<&[Tier], PricingError> {
        match self {
            Self::Tiered { tiers } => Ok(tiers),
            Self::Flat { .. } => Err(PricingError::NotFound {
                service: "expected tiered price but got flat".into(),
                region: String::new(),
            }),
        }
    }
}

/// Generic interface for pricing data lookups.
///
/// Implementations can be backed by flat files, an in-memory cache, a mock
/// (for testing), or any other source.
pub trait PriceCatalog: Send + Sync {
    /// The AWS region (or equivalent) this catalog provides prices for.
    fn region(&self) -> &str;

    /// Look up a price record by SKU.
    fn lookup(&self, sku: &Sku) -> Result<PriceRecord, PricingError>;

    /// Convenience method: look up a flat price value.
    fn lookup_f64(&self, sku: &Sku) -> Result<f64, PricingError> {
        self.lookup(sku)?.as_flat()
    }
}
