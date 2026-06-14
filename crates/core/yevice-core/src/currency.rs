//! Currency and time-dimension types for the cost evaluation pipeline.
//!
//! Implements ADR-0001 (Option B 変形): phantom-typed currency at SKU lookup,
//! erased to a runtime tag (`Money`) at the resource-cost boundary.
//!
//! # Phantom vs. erased
//!
//! - [`Currency<T, C>`] is a zero-cost wrapper used inside provider crates to
//!   keep the compile-time currency parameter. Mismatched arithmetic between
//!   `Currency<f64, USD>` and `Currency<f64, JPY>` is a type error.
//! - [`Money`] is the runtime-tagged form: the currency lives as a `String`
//!   and the billing period as a [`BillingPeriod`] enum. It is what
//!   `cost_model.json` (de)serializes and what flows through architecture
//!   aggregation.
//!
//! See `docs/adr/0001-currency-and-time-dimensions.md` for the full design.

use std::marker::PhantomData;
use std::ops::{Add, Mul, Sub};

use serde::{Deserialize, Serialize};

/// A currency code carried at the type level.
///
/// Each implementor exposes the ISO 4217 code as a `&'static str` literal so
/// the marker can be erased to a runtime tag (`Money.currency: String`)
/// without runtime cost.
pub trait CurrencyCode {
    /// The canonical ISO 4217 code (e.g. `"USD"`).
    const CODE: &'static str;
}

/// US Dollar marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct USD;

impl CurrencyCode for USD {
    const CODE: &'static str = "USD";
}

/// Japanese Yen marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JPY;

impl CurrencyCode for JPY {
    const CODE: &'static str = "JPY";
}

/// Euro marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EUR;

impl CurrencyCode for EUR {
    const CODE: &'static str = "EUR";
}

/// A phantom-typed currency-tagged value.
///
/// `Currency<f64, USD>` and `Currency<f64, JPY>` are distinct types at compile
/// time. Same-currency arithmetic compiles; mixed-currency arithmetic does not.
///
/// Use [`Currency::erase`] to drop into the runtime-tagged [`Money`] when
/// crossing the SKU-lookup boundary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Currency<T, C: CurrencyCode> {
    value: T,
    _marker: PhantomData<fn() -> C>,
}

impl<T, C: CurrencyCode> Currency<T, C> {
    /// Wrap a raw scalar with a compile-time currency tag.
    pub const fn new(value: T) -> Self {
        Self {
            value,
            _marker: PhantomData,
        }
    }

    /// Borrow the inner scalar.
    pub fn value(&self) -> &T {
        &self.value
    }

    /// Consume and return the inner scalar (loses the currency tag).
    pub fn into_inner(self) -> T {
        self.value
    }

    /// The ISO 4217 code carried by the type parameter `C`.
    pub fn code() -> &'static str {
        C::CODE
    }
}

impl<C: CurrencyCode> Currency<f64, C> {
    /// Erase the phantom-typed currency into a runtime-tagged [`Money`] for a
    /// monthly billing period.
    pub fn erase(self) -> Money {
        Money {
            value: self.value,
            currency: C::CODE.to_string(),
            period: BillingPeriod::Monthly,
        }
    }

    /// Erase to [`Money`] with an explicit billing period.
    pub fn erase_with_period(self, period: BillingPeriod) -> Money {
        Money {
            value: self.value,
            currency: C::CODE.to_string(),
            period,
        }
    }
}

// ---- Arithmetic on Currency<f64, C> ----
//
// Only same-currency operations compile; cross-currency arithmetic is a type
// error, which is the entire point of the phantom parameter.

impl<C: CurrencyCode> Add for Currency<f64, C> {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self::new(self.value + rhs.value)
    }
}

impl<C: CurrencyCode> Sub for Currency<f64, C> {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self::new(self.value - rhs.value)
    }
}

impl<C: CurrencyCode> Mul<f64> for Currency<f64, C> {
    type Output = Self;

    fn mul(self, rhs: f64) -> Self::Output {
        Self::new(self.value * rhs)
    }
}

/// Billing period for a [`Money`] value.
///
/// Phase 1 only models `Monthly`. Other variants are placeholders so the type
/// surface is forward-compatible with reserved-instance / hourly pricing
/// (tracked in a follow-up issue).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum BillingPeriod {
    /// Per calendar month (the only variant implemented today).
    #[default]
    Monthly,
    // Future: Hourly, Yearly, etc. — kept off the surface until evaluation
    // logic is in place. Adding now would let callers construct values that
    // the evaluator silently treats as Monthly.
}

/// A runtime-tagged currency amount.
///
/// `Money` carries the currency as a `String` (so it can round-trip through
/// `cost_model.json`) and the billing period as a [`BillingPeriod`].
///
/// `Money` does **not** implement `Add` / `Sub`: cross-currency arithmetic
/// requires explicit reconciliation (see [`crate::fx::convert_to`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Money {
    pub value: f64,
    pub currency: String,
    #[serde(default)]
    pub period: BillingPeriod,
}

impl Money {
    /// Construct a monthly [`Money`] amount.
    pub fn monthly(value: f64, currency: impl Into<String>) -> Self {
        Self {
            value,
            currency: currency.into(),
            period: BillingPeriod::Monthly,
        }
    }

    /// Zero-valued amount in the given currency.
    pub fn zero(currency: impl Into<String>) -> Self {
        Self::monthly(0.0, currency)
    }
}

impl std::fmt::Display for Money {
    /// Renders as `${value:.2} {currency}` (e.g. `"$1.23 USD"`). This is a
    /// stable, currency-agnostic default — CLI rendering uses bespoke
    /// formatting that knows the currency-specific glyph.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:.2} {}", self.value, self.currency)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn currency_code_constants() {
        assert_eq!(<USD as CurrencyCode>::CODE, "USD");
        assert_eq!(<JPY as CurrencyCode>::CODE, "JPY");
        assert_eq!(<EUR as CurrencyCode>::CODE, "EUR");
    }

    #[test]
    fn currency_arithmetic_same_currency() {
        let a: Currency<f64, USD> = Currency::new(10.0);
        let b: Currency<f64, USD> = Currency::new(3.5);
        let sum = a + b;
        assert_eq!(*sum.value(), 13.5);
        let diff = b - a;
        assert_eq!(*diff.value(), -6.5);
        let scaled = a * 2.0;
        assert_eq!(*scaled.value(), 20.0);
    }

    // Compile-fail demonstration: uncommenting the following block must fail.
    //
    // ```compile_fail
    // use yevice_core::currency::{Currency, USD, JPY};
    // let a: Currency<f64, USD> = Currency::new(10.0);
    // let b: Currency<f64, JPY> = Currency::new(1000.0);
    // let _ = a + b; // Cross-currency add — type error.
    // ```

    #[test]
    fn erase_drops_into_money_with_default_period() {
        let amount: Currency<f64, USD> = Currency::new(42.0);
        let money = amount.erase();
        assert_eq!(money.value, 42.0);
        assert_eq!(money.currency, "USD");
        assert_eq!(money.period, BillingPeriod::Monthly);
    }

    #[test]
    fn money_serde_roundtrip() {
        let money = Money::monthly(7.5, "USD");
        let json = serde_json::to_string(&money).expect("serialize");
        let back: Money = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, money);
    }

    #[test]
    fn money_deserialize_without_period_defaults_to_monthly() {
        let json = r#"{"value":12.0,"currency":"JPY"}"#;
        let back: Money = serde_json::from_str(json).expect("deserialize");
        assert_eq!(back.period, BillingPeriod::Monthly);
    }
}
