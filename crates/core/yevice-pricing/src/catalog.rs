//! Generic price catalog trait and erased / typed price records.
//!
//! `PriceCatalog` decouples service plugins from any specific pricing
//! provider implementation (file-based registry, bulk API, mock, etc.).
//!
//! ADR-0001 introduced a two-tier trait stack:
//! - [`PriceCatalog`] (dyn-friendly): `lookup` returns a runtime-tagged
//!   [`PricedValue`] (Scalar/Tiered enum carrying `currency: String`).
//! - [`TypedPricingProvider<C>`] (generic, not dyn-safe): returns
//!   [`TypedPriceRecord<C>`] with values wrapped in
//!   [`yevice_core::Currency<f64, C>`].

use std::borrow::Cow;
use std::marker::PhantomData;

use yevice_core::cost::Tier;
use yevice_core::currency::{Currency, CurrencyCode};

use crate::error::PricingError;

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

/// A single tier in an erased (runtime-tagged) tiered record.
#[derive(Debug, Clone, PartialEq)]
pub struct PricedTier {
    pub upper_limit: Option<f64>,
    pub unit_price: f64,
}

/// A price record returned by a `PriceCatalog`. Runtime-tagged with the
/// declared currency (one tag per record — same-currency invariant within
/// a tiered record is enforced upstream).
#[derive(Debug, Clone)]
pub enum PricedValue {
    /// A flat price per unit.
    Scalar { value: f64, currency: String },
    /// A tiered price structure. All tiers share `currency`.
    Tiered {
        tiers: Vec<PricedTier>,
        currency: String,
    },
}

impl PricedValue {
    /// Convenience constructor.
    pub fn scalar(value: f64, currency: impl Into<String>) -> Self {
        Self::Scalar {
            value,
            currency: currency.into(),
        }
    }

    /// Convenience constructor.
    pub fn tiered(tiers: Vec<PricedTier>, currency: impl Into<String>) -> Self {
        Self::Tiered {
            tiers,
            currency: currency.into(),
        }
    }

    /// Record-level currency tag.
    pub fn currency(&self) -> &str {
        match self {
            Self::Scalar { currency, .. } | Self::Tiered { currency, .. } => currency,
        }
    }

    /// Backward-compatible alias for [`Self::as_scalar`] from the pre-ADR-0001
    /// API. New code should prefer `as_scalar`.
    #[deprecated(note = "Renamed to `as_scalar` (ADR-0001).")]
    pub fn as_flat(&self) -> Result<f64, PricingError> {
        self.as_scalar()
    }

    /// Extract the scalar value, or return an error for tiered records.
    pub fn as_scalar(&self) -> Result<f64, PricingError> {
        match self {
            Self::Scalar { value, .. } => Ok(*value),
            Self::Tiered { .. } => Err(PricingError::NotFound {
                service: "expected scalar price but got tiered".into(),
                region: String::new(),
            }),
        }
    }

    /// Extract the tiers (without currency tag) for service-side `Expr::tiered`
    /// construction.
    pub fn as_tiered(&self) -> Result<Vec<Tier>, PricingError> {
        match self {
            Self::Tiered { tiers, .. } => Ok(tiers
                .iter()
                .map(|t| Tier {
                    upper_limit: t.upper_limit,
                    unit_price: t.unit_price,
                })
                .collect()),
            Self::Scalar { .. } => Err(PricingError::NotFound {
                service: "expected tiered price but got scalar".into(),
                region: String::new(),
            }),
        }
    }
}

// ---- Legacy alias ---------------------------------------------------------
//
// The pre-ADR-0001 `PriceRecord` enum (Flat / Tiered) is renamed to
// `PricedValue` with an added currency tag. We keep an alias so unrelated
// in-flight refactors don't have to update naming in the same commit batch.

#[deprecated(note = "Use `PricedValue` (currency-tagged) instead. ADR-0001.")]
pub type PriceRecord = PricedValue;

/// Generic interface for pricing data lookups (dyn-friendly).
///
/// Implementations can be backed by flat files, an in-memory cache, a mock
/// (for testing), or any other source. The returned [`PricedValue`] carries
/// the declared currency at runtime.
pub trait PriceCatalog: Send + Sync {
    /// The AWS region (or equivalent) this catalog provides prices for.
    fn region(&self) -> &str;

    /// Look up a price record by SKU.
    fn lookup(&self, sku: &Sku) -> Result<PricedValue, PricingError>;

    /// Convenience method: look up a scalar (flat) price value.
    fn lookup_f64(&self, sku: &Sku) -> Result<f64, PricingError> {
        self.lookup(sku)?.as_scalar()
    }
}

// ---------------------------------------------------------------------------
// Typed (compile-time phantom currency) tier ---------------------------------
// ---------------------------------------------------------------------------

/// A single tier in a phantom-typed tiered record.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedTier<C: CurrencyCode> {
    pub upper_limit: Option<f64>,
    pub unit_price: Currency<f64, C>,
    pub _marker: PhantomData<fn() -> C>,
}

impl<C: CurrencyCode> TypedTier<C> {
    pub fn new(upper_limit: Option<f64>, unit_price: Currency<f64, C>) -> Self {
        Self {
            upper_limit,
            unit_price,
            _marker: PhantomData,
        }
    }
}

/// A phantom-typed price record. Used inside provider crates where the
/// currency is statically known.
#[derive(Debug, Clone, PartialEq)]
pub enum TypedPriceRecord<C: CurrencyCode> {
    Scalar(Currency<f64, C>),
    Tiered(Vec<TypedTier<C>>),
}

impl<C: CurrencyCode> TypedPriceRecord<C> {
    /// Erase the phantom currency into a runtime-tagged [`PricedValue`].
    pub fn erase(self) -> PricedValue {
        match self {
            Self::Scalar(amount) => PricedValue::Scalar {
                value: *amount.value(),
                currency: C::CODE.to_string(),
            },
            Self::Tiered(tiers) => {
                let tiers = tiers
                    .into_iter()
                    .map(|t| PricedTier {
                        upper_limit: t.upper_limit,
                        unit_price: *t.unit_price.value(),
                    })
                    .collect();
                PricedValue::Tiered {
                    tiers,
                    currency: C::CODE.to_string(),
                }
            }
        }
    }
}

/// Generic / dyn-unsafe pricing provider trait.
///
/// Lives at the **inside** of a provider crate: `AwsPricingRegistry` impls
/// `TypedPricingProvider<USD>` so internal pricing arithmetic stays
/// phantom-typed. The `PriceCatalog` impl on the same struct calls into this
/// trait and then `.erase()` the result.
pub trait TypedPricingProvider<C: CurrencyCode>: Send + Sync {
    fn lookup(&self, sku: &Sku) -> Result<TypedPriceRecord<C>, PricingError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use yevice_core::currency::{Currency, USD};

    #[test]
    fn typed_scalar_erases_to_priced_value_with_code() {
        let typed: TypedPriceRecord<USD> = TypedPriceRecord::Scalar(Currency::new(0.5));
        let erased = typed.erase();
        match erased {
            PricedValue::Scalar { value, currency } => {
                assert_eq!(value, 0.5);
                assert_eq!(currency, "USD");
            }
            PricedValue::Tiered { .. } => panic!("expected scalar"),
        }
    }

    #[test]
    fn typed_tiered_preserves_tier_count() {
        let typed: TypedPriceRecord<USD> = TypedPriceRecord::Tiered(vec![
            TypedTier::new(Some(1000.0), Currency::new(0.0)),
            TypedTier::new(None, Currency::new(0.0001)),
        ]);
        let erased = typed.erase();
        match erased {
            PricedValue::Tiered { tiers, currency } => {
                assert_eq!(tiers.len(), 2);
                assert_eq!(currency, "USD");
                assert_eq!(tiers[1].unit_price, 0.0001);
            }
            PricedValue::Scalar { .. } => panic!("expected tiered"),
        }
    }
}
