use thiserror::Error;

#[derive(Debug, Error)]
pub enum WranglerError {
    #[error("failed to parse Wrangler config: {0}")]
    ParseError(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<toml::de::Error> for WranglerError {
    fn from(e: toml::de::Error) -> Self {
        WranglerError::ParseError(e.to_string())
    }
}

impl From<serde_json::Error> for WranglerError {
    fn from(e: serde_json::Error) -> Self {
        WranglerError::ParseError(e.to_string())
    }
}
