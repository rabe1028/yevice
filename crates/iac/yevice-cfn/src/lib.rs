pub mod convert;
pub mod error;
pub mod intrinsic;
pub mod parser;
pub(crate) mod sentinel;

pub use convert::build_architecture;
pub use error::CfnError;
pub use parser::{CfnResource, CfnTemplate, parse_template, parse_template_str, resolve_template};
