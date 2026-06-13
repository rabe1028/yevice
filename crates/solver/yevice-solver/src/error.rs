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

    /// One or more variables referenced by the objective are not bound by a
    /// fixed parameter, a decision variable, or a transitively-satisfiable
    /// binding. Detected up-front by [`validate_bindings`] before enumeration.
    ///
    /// [`validate_bindings`]: crate::validate_bindings
    #[error(
        "{} objective variable(s) are unbound: {}. Bind them via fixed parameters, \
         decision variables, or bindings.",
        variables.len(),
        variables.join(", ")
    )]
    UnboundVariables {
        /// Names of the unbound objective variables, in sorted order.
        variables: Vec<String>,
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
    #[error("expression evaluation error")]
    Eval(#[from] yevice_core::error::CoreError),

    /// The requested solver backend name is not recognized by the
    /// [`solver_from_name`] factory.
    ///
    /// [`solver_from_name`]: crate::solver_from_name
    #[error(
        "unknown solver backend '{requested}'. Allowed values: {}",
        allowed.join(", ")
    )]
    UnknownSolver {
        /// The name the caller asked for.
        requested: String,
        /// The list of backends that the current build understands.
        allowed: Vec<String>,
    },
}
