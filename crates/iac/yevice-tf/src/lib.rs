//! Terraform parsing and conversion for yevice.

pub mod convert;
pub mod error;
pub mod parser;
pub mod resolver;

pub use convert::build_architecture;
pub use error::{TfError, UnresolvedSymbolKind};
pub use parser::{TfConfig, parse_tf_dir, parse_tfvars};
pub use resolver::{ResolvedConfig, resolve_config, resolve_config_with_policy};
