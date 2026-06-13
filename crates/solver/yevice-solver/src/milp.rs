//! Self-hosted MILP backend abstraction.
//!
//! Defines the minimal `MilpBackend` trait that `yevice-solver` builds against,
//! plus the `MilpSolution` value returned by every backend. The default
//! HiGHS-backed implementation lives in [`crate::highs_backend`] behind the
//! `highs` Cargo feature; future backends (CBC, GLOP, etc.) plug in by
//! implementing the same trait.
//!
//! See ADR-0002 "LP/MIP Solver Backend" for the rationale (Option C).

use std::collections::BTreeMap;

use crate::error::SolverError;

/// Variable type the solver may assign.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarType {
    /// Continuous variable.
    Continuous,
    /// Integer variable.
    Integer,
    /// Binary variable (0 / 1).
    Binary,
}

/// Optimization sense for the MILP backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sense {
    Minimize,
    Maximize,
}

/// Constraint sense for the MILP backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstraintSense {
    /// `lhs <= rhs`
    Le,
    /// `lhs >= rhs`
    Ge,
    /// `lhs == rhs`
    Eq,
}

/// Tuning options forwarded to the backend.
///
/// Each backend is free to ignore fields it does not support; the HiGHS
/// adapter forwards every field to the underlying HiGHS options.
#[derive(Debug, Clone)]
pub struct MilpOptions {
    /// Wall-clock time limit in seconds. `None` = backend default.
    pub time_limit_sec: Option<f64>,
    /// Relative MIP optimality gap. `None` = backend default.
    pub mip_gap: Option<f64>,
    /// Thread count (0 = backend auto). `None` = backend default.
    pub threads: Option<i32>,
}

impl Default for MilpOptions {
    fn default() -> Self {
        Self {
            time_limit_sec: Some(300.0),
            mip_gap: Some(1e-4),
            threads: Some(0),
        }
    }
}

/// Outcome of a MILP solve.
#[derive(Debug, Clone)]
pub struct MilpSolution {
    /// Per-variable assignment, keyed by the `VarId` returned from `add_var`.
    pub assignments: BTreeMap<VarId, f64>,
    /// Objective value at the returned assignment. `f64::NAN` when infeasible.
    pub objective_value: f64,
    /// True iff the backend returned a feasible point (optimal or otherwise).
    pub feasible: bool,
    /// Optional final MIP gap reported by the backend.
    pub mip_gap: Option<f64>,
}

/// Opaque variable handle issued by the backend on `add_var`.
///
/// Using a single concrete `u32` type (rather than a `MilpBackend::VarId`
/// associated type) keeps the encoder code generic-free; the trait itself
/// still allows different backends to choose how they map this to their
/// internal column ids.
pub type VarId = u32;

/// Opaque constraint handle issued by the backend on `add_constraint`.
pub type ConstraintId = u32;

/// Minimum API every MILP backend implements.
///
/// The trait deliberately stays small: variable, constraint, objective, solve.
/// Tuning is passed once via `MilpOptions` at construction time. Backends do
/// not need to implement column generation, callbacks, warm starts, etc.
pub trait MilpBackend {
    /// Add a variable with the given bounds, objective coefficient, and type.
    /// Returns its handle.
    fn add_var(&mut self, lower: f64, upper: f64, objective_coeff: f64, var_type: VarType)
    -> VarId;

    /// Add a linear constraint `sum(coeffs[i] * vars[i]) <relation> rhs`.
    /// Returns its handle. The `var_terms` slice gives `(var_id, coefficient)`
    /// pairs.
    fn add_constraint(
        &mut self,
        var_terms: &[(VarId, f64)],
        sense: ConstraintSense,
        rhs: f64,
    ) -> ConstraintId;

    /// Set the optimization sense.
    fn set_sense(&mut self, sense: Sense);

    /// Run the solver and return its solution.
    ///
    /// Consumes `self` to make the API symmetric with backends that move the
    /// problem into a solve handle.
    fn solve(self: Box<Self>) -> Result<MilpSolution, SolverError>;
}
