use thiserror::Error;

#[derive(Debug, Error)]
pub enum PricingError {
    #[error("pricing data not found for {service} in {region}")]
    NotFound { service: String, region: String },

    #[error("pricing data file not found: {0}")]
    FileNotFound(String),

    #[error("failed to parse pricing data: {0}")]
    ParseError(String),
}
