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

    /// File-read failure from the shared `yevice_core::io::read_iac_file`
    /// helper. Retained alongside `Io` so the existing public surface
    /// (`Io(std::io::Error)`) is not removed — non-breaking addition.
    #[error(transparent)]
    IoRead(#[from] yevice_core::io::IoReadError),

    #[error("YAML parse error")]
    Yaml(#[from] serde_yaml_ng::Error),
}
