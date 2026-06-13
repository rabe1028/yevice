use thiserror::Error;

#[derive(Debug, Error)]
pub enum WranglerError {
    #[error("failed to parse Wrangler config: {0}")]
    ParseError(String),
    #[error("IO error")]
    Io(#[from] std::io::Error),
    /// File-read failure from the shared `yevice_core::io::read_iac_file`
    /// helper. Retained alongside `Io` so the existing public surface
    /// (`Io(std::io::Error)`) is not removed — non-breaking addition.
    #[error(transparent)]
    IoRead(#[from] yevice_core::io::IoReadError),
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
