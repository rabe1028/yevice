use thiserror::Error;

#[derive(Debug, Error)]
pub enum CfnError {
    #[error("failed to parse CloudFormation template: {0}")]
    ParseError(String),

    #[error("failed to resolve intrinsic function: {0}")]
    IntrinsicError(String),

    #[error("parameter not found: {0}")]
    ParameterNotFound(String),

    #[error("import value not found: {0}")]
    ImportValueNotFound(String),

    #[error("mapping not found: {map_name}.{first_key}.{second_key}")]
    MappingNotFound {
        map_name: String,
        first_key: String,
        second_key: String,
    },

    #[error("condition not found: {0}")]
    ConditionNotFound(String),

    #[error("Parameters: [{0}] must have values")]
    MissingParameters(String),

    #[error("unsupported resource type: {0}")]
    UnsupportedResourceType(String),

    #[error("missing required property {property} for {resource_type}")]
    MissingProperty {
        resource_type: String,
        property: String,
    },

    #[error("IO error")]
    Io(#[from] std::io::Error),

    #[error("YAML parse error")]
    Yaml(#[from] serde_yaml_ng::Error),
}

/// Funnel the shared IaC read error into the existing [`CfnError::Io`]
/// variant so that adding `read_iac_file` does **not** introduce a new
/// public enum variant. The full `IoReadError` `Display` (which includes
/// the offending path) is preserved by embedding it as the inner
/// `io::Error`'s message.
impl From<yevice_core::io::IoReadError> for CfnError {
    fn from(e: yevice_core::io::IoReadError) -> Self {
        let kind = e.source.kind();
        CfnError::Io(std::io::Error::new(kind, e.to_string()))
    }
}
