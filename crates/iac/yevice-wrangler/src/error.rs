use thiserror::Error;

#[derive(Debug, Error)]
pub enum WranglerError {
    #[error("failed to parse Wrangler config: {0}")]
    ParseError(String),
    #[error("IO error")]
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

/// Funnel the shared IaC read error into the existing [`WranglerError::Io`]
/// variant so that adopting `read_iac_file` does **not** introduce a new
/// public enum variant. The path-prefixed `IoReadError` message is
/// preserved as the inner `io::Error`'s message string.
impl From<yevice_core::io::IoReadError> for WranglerError {
    fn from(e: yevice_core::io::IoReadError) -> Self {
        let kind = e.source.kind();
        WranglerError::Io(std::io::Error::new(kind, e.to_string()))
    }
}
