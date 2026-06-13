//! Foreign-exchange conversion primitives.
//!
//! Implements the `ExchangeRates` trait surface from ADR-0001. Phase 1 only
//! supports a single `RateDate` shape (a calendar day); spot vs. monthly
//! variants are deferred.
//!
//! ```
//! use std::collections::BTreeMap;
//! use chrono::NaiveDate;
//! use yevice_core::fx::{ExchangeRates, RateDate, StaticRates, convert_to};
//!
//! let mut rates = StaticRates::new();
//! rates.insert("JPY", "USD", 1.0 / 150.0);
//! let mut totals: BTreeMap<String, f64> = BTreeMap::new();
//! totals.insert("USD".into(), 100.0);
//! totals.insert("JPY".into(), 15_000.0);
//! let at = RateDate::new(NaiveDate::from_ymd_opt(2026, 6, 13).unwrap());
//! let display = convert_to(&totals, "USD", &rates, at).unwrap();
//! assert!((display.value - 200.0).abs() < 1e-9);
//! ```

use std::collections::BTreeMap;

use chrono::NaiveDate;
use thiserror::Error;

use crate::currency::{BillingPeriod, Money};

/// A calendar-day at which an FX rate is requested.
///
/// The ADR earmarks this for future expansion (e.g. `Spot(DateTime<Utc>)` vs.
/// `Monthly(YearMonth)`); Phase 1 only carries a single `NaiveDate`, but the
/// shape is baked into the trait so callers can adopt richer date semantics
/// without an API break.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RateDate(pub NaiveDate);

impl RateDate {
    pub const fn new(date: NaiveDate) -> Self {
        Self(date)
    }

    pub fn date(&self) -> NaiveDate {
        self.0
    }
}

/// A multiplicative exchange rate (target_per_source).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rate(pub f64);

impl Rate {
    pub const fn new(rate: f64) -> Self {
        Self(rate)
    }

    pub fn as_f64(self) -> f64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Error)]
pub enum FxError {
    #[error("missing exchange rate from {from} to {to} at {at:?}")]
    MissingRate {
        from: String,
        to: String,
        at: RateDate,
    },
}

/// Trait for looking up FX rates.
///
/// Identity (`from == to`) MUST resolve to `Rate(1.0)`; the default
/// implementation enforces this.
pub trait ExchangeRates {
    fn rate(&self, from: &str, to: &str, at: RateDate) -> Result<Rate, FxError>;
}

/// In-memory exchange rate table.
///
/// Maps `(from, to)` pairs to a constant rate — date is currently ignored.
#[derive(Debug, Clone, Default)]
pub struct StaticRates {
    rates: BTreeMap<(String, String), f64>,
}

impl StaticRates {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a directional rate (`to_per_from`).
    pub fn insert(&mut self, from: impl Into<String>, to: impl Into<String>, rate: f64) {
        self.rates.insert((from.into(), to.into()), rate);
    }

    /// Register a directional rate and return `self` for chaining.
    #[must_use]
    pub fn with(mut self, from: impl Into<String>, to: impl Into<String>, rate: f64) -> Self {
        self.insert(from, to, rate);
        self
    }
}

impl ExchangeRates for StaticRates {
    fn rate(&self, from: &str, to: &str, at: RateDate) -> Result<Rate, FxError> {
        if from == to {
            return Ok(Rate(1.0));
        }
        self.rates
            .get(&(from.to_string(), to.to_string()))
            .copied()
            .map(Rate)
            .ok_or_else(|| FxError::MissingRate {
                from: from.to_string(),
                to: to.to_string(),
                at,
            })
    }
}

/// Convert a `totals_by_currency` map into a single [`Money`] amount in the
/// target currency.
///
/// Fails fast if any source currency lacks a rate to `target`; partial
/// conversion would silently drop value.
pub fn convert_to(
    totals: &BTreeMap<String, f64>,
    target: &str,
    rates: &dyn ExchangeRates,
    at: RateDate,
) -> Result<Money, FxError> {
    let mut sum = 0.0;
    for (currency, amount) in totals {
        let rate = rates.rate(currency, target, at)?;
        sum += amount * rate.as_f64();
    }
    Ok(Money {
        value: sum,
        currency: target.to_string(),
        period: BillingPeriod::Monthly,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> RateDate {
        RateDate::new(NaiveDate::from_ymd_opt(y, m, day).unwrap())
    }

    #[test]
    fn identity_rate_is_one() {
        let rates = StaticRates::new();
        let r = rates.rate("USD", "USD", d(2026, 6, 13)).unwrap();
        assert_eq!(r.as_f64(), 1.0);
    }

    #[test]
    fn missing_rate_errors() {
        let rates = StaticRates::new();
        let err = rates.rate("USD", "JPY", d(2026, 6, 13)).unwrap_err();
        assert!(matches!(err, FxError::MissingRate { .. }));
    }

    #[test]
    fn convert_to_sums_across_currencies() {
        let rates = StaticRates::new()
            .with("JPY", "USD", 1.0 / 150.0)
            .with("EUR", "USD", 1.1);
        let mut totals = BTreeMap::new();
        totals.insert("USD".to_string(), 10.0);
        totals.insert("JPY".to_string(), 3_000.0);
        totals.insert("EUR".to_string(), 5.0);
        let display = convert_to(&totals, "USD", &rates, d(2026, 6, 13)).unwrap();
        assert_eq!(display.currency, "USD");
        assert_eq!(display.period, BillingPeriod::Monthly);
        // 10 + 3000/150 + 5*1.1 = 10 + 20 + 5.5
        assert!((display.value - 35.5).abs() < 1e-9);
    }

    #[test]
    fn convert_to_propagates_missing_rate() {
        let rates = StaticRates::new();
        let mut totals = BTreeMap::new();
        totals.insert("JPY".to_string(), 1000.0);
        let err = convert_to(&totals, "USD", &rates, d(2026, 6, 13)).unwrap_err();
        assert!(matches!(err, FxError::MissingRate { .. }));
    }
}
