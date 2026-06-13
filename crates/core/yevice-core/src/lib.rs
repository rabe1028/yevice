pub mod bindings;
pub mod capacity;
pub mod cost;
pub mod error;
pub mod evaluate;
pub mod expr;
pub mod expr_introspect;
pub mod expr_parser;
pub mod io;
pub mod optimize;
pub mod parse_policy;
pub mod resource;
pub mod schema;
pub mod simulate;
pub mod topology;
pub mod types;

pub use expr_introspect::LinearForm;
pub use optimize::{
    DecisionVariable, ObjectiveDirection, OptimizationConstraint, OptimizationProblem, Relation,
};
pub use parse_policy::{
    DiagnosticSource, IacParseDiagnostic, ParseOutcome, ParsePolicy, Severity, SourceLocation,
};
pub use topology::{Topology, TopologyNode};

/// Approximate number of hours in a calendar month (365 * 24 / 12).
///
/// Used by service implementations that price resources in hourly rates
/// and need to convert to a monthly cost.
pub const HOURS_PER_MONTH: f64 = 730.0;
