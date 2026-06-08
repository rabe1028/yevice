//! Error types for the yevice solver.

use thiserror::Error;

/// Errors that can occur during optimization.
#[derive(Debug, Error)]
pub enum SolverError {
    /// The Cartesian product of all decision-variable domains exceeds the
    /// configured safety limit, preventing enumeration.
    #[error(
        "too many combinations to enumerate: {count} exceeds limit of {limit}. \
         Reduce domain sizes or switch to a non-enumerating backend."
    )]
    TooManyCombinations {
        /// Total number of combinations that would be enumerated.
        count: u64,
        /// The configured upper limit.
        limit: u64,
    },

    /// No assignment satisfies all constraints.
    #[error("problem is infeasible: no combination satisfies all constraints")]
    Infeasible,

    /// A core evaluation error propagated from the expression evaluator.
    #[error("expression evaluation error: {0}")]
    Eval(#[from] yevice_core::error::CoreError),
}
