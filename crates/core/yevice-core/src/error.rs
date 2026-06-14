use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("undefined variable: {0}")]
    UndefinedVariable(String),
    #[error("division by zero in cost expression")]
    DivisionByZero,
    #[error("resource {resource_id} has inconsistent component currencies: {currencies:?}")]
    ComponentCurrencyMismatch {
        resource_id: String,
        currencies: Vec<String>,
    },
}
