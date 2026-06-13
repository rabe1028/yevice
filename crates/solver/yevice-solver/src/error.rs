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
    /// The [`EnumerationSolver`] expresses infeasibility as
    /// `Ok(Solution { feasible: false, .. })`. The HiGHS backend may also
    /// return `Ok(Solution { feasible: false, .. })` for infeasible problems;
    /// this variant is kept for backends that prefer to signal infeasibility
    /// as a typed error.
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

    /// The expression contains a non-linear shape that the MILP encoder
    /// cannot represent (e.g. `var * var`, `var / var`).
    ///
    /// Issued up-front by the HiGHS backend's pre-check
    /// (`expr_is_linearizable`).
    #[error(
        "expression is not MILP-linearizable: {expr}. \
         Use --solver enumeration or restructure the expression."
    )]
    Nonlinear {
        /// Debug-formatted snippet of the offending sub-expression.
        expr: String,
    },

    /// A `Ceil(expr)` node appears in a context where the lower-bound-only
    /// formulation (`expr <= y`, `y integer`) is not auto-tight.
    ///
    /// See ADR-0002 "Ceil 定式化の選択" for the full classification of
    /// allowed vs. rejected contexts. The HiGHS backend rejects the
    /// problem rather than encoding a relaxation that would silently
    /// drop the ceil semantics.
    #[error(
        "ceil expression in unsupported context: {expr}. {reason}. \
         Use --solver enumeration or restructure the expression."
    )]
    UnsupportedCeilContext {
        /// Debug-formatted snippet of the offending ceil expression
        /// (after bindings expansion).
        expr: String,
        /// Static description of the anti-tight direction.
        reason: &'static str,
    },

    /// A `Max`/`Min` big-M encoding could not pick a finite M because the
    /// inner expression has no finite bound under the current domain.
    ///
    /// Issued by the HiGHS encoder when `expr_bounds` returns `+/- inf`
    /// for a sub-expression that needs a big-M.
    #[error(
        "cannot derive a finite big-M for expression: {expr}. \
         Tighten the variable domains or use --solver enumeration."
    )]
    UnboundedExpression {
        /// Debug-formatted snippet of the unbounded sub-expression.
        expr: String,
    },

    /// The MILP backend itself signalled a failure (build error, numerical
    /// problem, time-limit exceeded without an incumbent, etc.).
    #[error("MILP backend error: {message}")]
    MilpBackend {
        /// Human-readable description from the backend.
        message: String,
    },
}
