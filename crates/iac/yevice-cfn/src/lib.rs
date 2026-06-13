pub mod convert;
pub mod error;
pub mod intrinsic;
pub mod parser;
pub mod resolved;

pub use convert::build_architecture;
pub use error::CfnError;
pub use parser::{
    CfnResource, CfnTemplate, ResolvedResource, ResolvedTemplate, parse_template,
    parse_template_str, resolve_template, resolve_template_with_policy,
};
pub use resolved::{Reference, ResolvedValue, StringPart};
