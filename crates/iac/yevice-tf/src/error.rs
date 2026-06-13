use thiserror::Error;

#[derive(Debug, Error)]
pub enum TfError {
    #[error("failed to parse Terraform config: {0}")]
    ParseError(String),
    #[error("IO error")]
    Io(#[from] std::io::Error),
    /// File-read failure from the shared `yevice_core::io::read_iac_file`
    /// helper. Retained alongside `Io` so the existing public surface
    /// (`Io(std::io::Error)`) is not removed — non-breaking addition.
    #[error(transparent)]
    IoRead(#[from] yevice_core::io::IoReadError),
    #[error("missing required attribute {attr} for resource {resource}")]
    MissingAttribute { resource: String, attr: String },
}

impl From<hcl::Error> for TfError {
    fn from(error: hcl::Error) -> Self {
        Self::ParseError(error.to_string())
    }
}
