use thiserror::Error;
use yevice_pricing::error::PricingError;

#[derive(Debug, Error)]
pub enum CostError {
    #[error("pricing error: {0}")]
    Pricing(#[from] PricingError),

    #[error("failed to deserialize spec for service '{service_id}': {cause}")]
    SpecDeserialize { service_id: String, cause: String },

    #[error("spec type mismatch for service '{service_id}'")]
    SpecMismatch { service_id: String },

    #[error("unsupported resource: '{0}'")]
    UnsupportedResource(String),
}
