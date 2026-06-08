use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("undefined variable: {0}")]
    UndefinedVariable(String),
}
