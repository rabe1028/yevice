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

    /// Reserved for future solver backends that signal infeasibility as an
    /// error rather than through a `Solution` value.
    ///
    /// The current [`EnumerationSolver`] expresses infeasibility as
    /// `Ok(Solution { feasible: false, .. })` and never constructs this variant.
    /// It is kept in the public API so that future backends (e.g., LP/MIP
    /// solvers) can use it without a breaking change.
    ///
    /// [`EnumerationSolver`]: crate::EnumerationSolver
    #[error("problem is infeasible: no combination satisfies all constraints")]
    Infeasible,

    /// A core evaluation error propagated from the expression evaluator.
    #[error("expression evaluation error: {0}")]
    Eval(#[from] yevice_core::error::CoreError),
}
