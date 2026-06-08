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

    #[error("unsupported resource type: {0}")]
    UnsupportedResourceType(String),

    #[error("missing required property {property} for {resource_type}")]
    MissingProperty {
        resource_type: String,
        property: String,
    },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("YAML parse error: {0}")]
    Yaml(#[from] serde_yaml_ng::Error),
}
