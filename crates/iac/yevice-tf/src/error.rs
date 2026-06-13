use thiserror::Error;
use yevice_core::parse_policy::SourceLocation;

#[derive(Debug, Error)]
pub enum TfError {
    #[error("failed to parse Terraform config: {0}")]
    ParseError(String),
    #[error("IO error")]
    Io(#[from] std::io::Error),
    #[error("missing required attribute {attr} for resource {resource}")]
    MissingAttribute { resource: String, attr: String },
    /// A `var.*` or `local.*` reference could not be resolved.
    ///
    /// Raised under [`yevice_core::parse_policy::ParsePolicy::Strict`]; the
    /// Lenient default funnels these through
    /// [`yevice_core::parse_policy::IacParseDiagnostic`] instead.
    /// Per ADR-0003, this covers only undefined variable/local symbols;
    /// `TfValue::ResourceRef` (`resource.<type>.<name>.<attr>`) is policy-neutral
    /// because the resolver treats it as a valid pass-through.
    #[error("unresolved {kind} reference: {name}")]
    UnresolvedSymbol {
        kind: UnresolvedSymbolKind,
        name: String,
        location: Option<SourceLocation>,
    },
}

/// Kind of unresolved Terraform symbol carried by [`TfError::UnresolvedSymbol`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnresolvedSymbolKind {
    /// `var.*` reference whose variable has no default and was not supplied
    /// via tfvars.
    Variable,
    /// `local.*` reference whose value could not be resolved (cycle or
    /// missing dependency).
    Local,
}

impl std::fmt::Display for UnresolvedSymbolKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Variable => "var",
            Self::Local => "local",
        };
        f.write_str(s)
    }
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
