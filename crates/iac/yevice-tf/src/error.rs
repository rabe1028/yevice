use thiserror::Error;

#[derive(Debug, Error)]
pub enum TfError {
    #[error("failed to parse Terraform config: {0}")]
    ParseError(String),
    #[error("IO error")]
    Io(#[from] std::io::Error),
    #[error("missing required attribute {attr} for resource {resource}")]
    MissingAttribute { resource: String, attr: String },
}

impl From<hcl::Error> for TfError {
    fn from(error: hcl::Error) -> Self {
        Self::ParseError(error.to_string())
    }
}

/// Funnel the shared IaC read error into the existing [`TfError::Io`]
/// variant so that adopting `read_iac_file` does **not** introduce a new
/// public enum variant. The path-prefixed `IoReadError` message is
/// preserved as the inner `io::Error`'s message string.
impl From<yevice_core::io::IoReadError> for TfError {
    fn from(e: yevice_core::io::IoReadError) -> Self {
        let kind = e.source.kind();
        TfError::Io(std::io::Error::new(kind, e.to_string()))
    }
}
