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
