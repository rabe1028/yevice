use thiserror::Error;

use crate::catalog::Sku;

#[derive(Debug, Error)]
pub enum PricingError {
    #[error("pricing data not found for {service} in {region}")]
    NotFound { service: String, region: String },

    #[error("pricing data file not found: {0}")]
    FileNotFound(String),

    #[error("failed to parse pricing data: {0}")]
    ParseError(String),

    /// Bulk API metadata declared a currency that does not match the
    /// provider's static `CurrencyCode::CODE`. Introduced by ADR-0001 to
    /// surface silent mislabeling (e.g. CNY data loaded into a USD provider).
    #[error(
        "pricing currency mismatch for {sku}: provider expects {expected}, file reported {actual}"
    )]
    CurrencyMismatch {
        expected: String,
        actual: String,
        sku: Sku,
    },
}
