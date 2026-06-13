//! HiGHS-backed implementation of [`MilpBackend`] and a `Solver` adapter.
//!
//! This module is only compiled with the `highs` Cargo feature. It wraps the
//! `highs` crate (vendored HiGHS C++ via `highs-sys`) and translates the
//! generic `MilpBackend` calls into HiGHS row/column operations.

use std::collections::{BTreeMap, HashMap};

use highs::{Col, HighsModelStatus, RowProblem, Sense as HighsSense};
use yevice_core::optimize::OptimizationProblem;
use yevice_core::types::VariableName;

use crate::error::SolverError;
use crate::expr_pre_checks::pre_check;
use crate::milp::{ConstraintSense, MilpBackend, MilpOptions, MilpSolution, Sense, VarId, VarType};
use crate::milp_encoder::encode;
use crate::{Solution, Solver};

/// HiGHS-backed MILP backend.
///
/// Implements [`MilpBackend`] by buffering all calls into a `RowProblem`
/// (variable-first matrix format, since we always know a variable's bounds
/// at `add_var` time). `solve` ships the problem to HiGHS, reads back the
/// solution, and returns it as a `MilpSolution`.
pub struct HighsBackend {
    problem: RowProblem,
    /// Backend-issued `Col` handles, indexed by `VarId` (= `Vec` position).
    cols: Vec<Col>,
    sense: HighsSense,
    options: MilpOptions,
}

impl HighsBackend {
    /// Construct a fresh backend with the given tuning options.
    #[must_use]
    pub fn new(options: MilpOptions) -> Self {
        Self {
            problem: RowProblem::default(),
            cols: Vec::new(),
            sense: HighsSense::Minimise,
            options,
        }
    }
}

impl MilpBackend for HighsBackend {
    fn add_var(
        &mut self,
        lower: f64,
        upper: f64,
        objective_coeff: f64,
        var_type: VarType,
    ) -> VarId {
        let is_integer = matches!(var_type, VarType::Integer | VarType::Binary);
        let col = if is_integer {
            self.problem
                .add_integer_column(objective_coeff, lower..=upper)
        } else {
            self.problem.add_column(objective_coeff, lower..=upper)
        };
        self.cols.push(col);
        (self.cols.len() - 1) as VarId
    }

    fn add_constraint(
        &mut self,
        var_terms: &[(VarId, f64)],
        sense: ConstraintSense,
        rhs: f64,
    ) -> u32 {
        let factors: Vec<(Col, f64)> = var_terms
            .iter()
            .map(|&(id, c)| (self.cols[id as usize], c))
            .collect();
        match sense {
            ConstraintSense::Le => self.problem.add_row(f64::NEG_INFINITY..=rhs, factors),
            ConstraintSense::Ge => self.problem.add_row(rhs..=f64::INFINITY, factors),
            ConstraintSense::Eq => self.problem.add_row(rhs..=rhs, factors),
        }
        0 // ConstraintIds are unused by the encoder.
    }

    fn set_sense(&mut self, sense: Sense) {
        self.sense = match sense {
            Sense::Minimize => HighsSense::Minimise,
            Sense::Maximize => HighsSense::Maximise,
        };
    }

    fn solve(self: Box<Self>) -> Result<MilpSolution, SolverError> {
        let Self {
            problem,
            cols: _,
            sense,
            options,
        } = *self;
        let mut model = problem.optimise(sense);

        // Forward tuning options. Misspelled keys silently fail (HiGHS C API).
        if let Some(t) = options.time_limit_sec {
            model.set_option("time_limit", t);
        }
        if let Some(g) = options.mip_gap {
            model.set_option("mip_rel_gap", g);
        }
        if let Some(n) = options.threads {
            model.set_option("threads", n);
        }
        // Quiet the HiGHS banner / iteration log.
        model.set_option("output_flag", false);

        let solved = model.solve();
        let status = solved.status();
        let feasible = matches!(
            status,
            HighsModelStatus::Optimal | HighsModelStatus::ReachedSolutionLimit
        );
        let definitely_infeasible = matches!(
            status,
            HighsModelStatus::Infeasible
                | HighsModelStatus::ModelEmpty
                | HighsModelStatus::UnboundedOrInfeasible
        );
        if !feasible && !definitely_infeasible {
            return Err(SolverError::MilpBackend {
                message: format!("HiGHS finished with status {status:?}"),
            });
        }

        let mut assignments: BTreeMap<VarId, f64> = BTreeMap::new();
        let objective_value = if feasible {
            let solution = solved.get_solution();
            for (idx, &v) in solution.columns().iter().enumerate() {
                assignments.insert(idx as VarId, v);
            }
            solved.objective_value()
        } else {
            f64::NAN
        };

        Ok(MilpSolution {
            assignments,
            objective_value,
            feasible,
            mip_gap: None,
        })
    }
}

// ---------------------------------------------------------------------------
// `HighsSolver`: implements the public `Solver` trait
// ---------------------------------------------------------------------------

/// A [`Solver`] that translates the problem to MILP and dispatches to HiGHS.
#[derive(Default)]
pub struct HighsSolver {
    pub options: MilpOptions,
}

impl Solver for HighsSolver {
    fn solve(&self, problem: &OptimizationProblem) -> Result<Solution, SolverError> {
        // Pre-checks (same set, in order): unbound vars → linearizability →
        // ceil context safety → unbounded big-M.
        crate::validate_bindings(problem)?;
        pre_check(problem)?;

        // Quick out: any empty domain ⇒ infeasible (mirrors EnumerationSolver).
        if problem
            .decision_variables
            .iter()
            .any(|dv| dv.domain.is_empty())
        {
            return Ok(infeasible_solution());
        }

        // Check that every variable referenced by a constraint LHS is bound
        // (decision variable, fixed parameter, or transitively-bound binding
        // target). Mirrors `EnumerationSolver`'s behaviour: `is_feasible`
        // returns `false` when constraint evaluation fails due to an unbound
        // variable, so no combination is ever feasible — the correct outcome
        // is an infeasible solution, not an `UnboundVariables` error.
        {
            let mut bound: std::collections::HashSet<VariableName> =
                problem.fixed_params.keys().cloned().collect();
            for dv in &problem.decision_variables {
                bound.insert(dv.name.clone());
            }
            loop {
                let mut progressed = false;
                for b in &problem.bindings {
                    if bound.contains(&b.target) {
                        continue;
                    }
                    if b.expr.variables().iter().all(|v| bound.contains(v)) {
                        bound.insert(b.target.clone());
                        progressed = true;
                    }
                }
                if !progressed {
                    break;
                }
            }
            let has_unbound_constraint_var = problem
                .constraints
                .iter()
                .any(|c| c.lhs.variables().iter().any(|v| !bound.contains(v)));
            if has_unbound_constraint_var {
                return Ok(infeasible_solution());
            }
        }

        let mut backend = Box::new(HighsBackend::new(self.options.clone()));
        let encoded = encode(backend.as_mut(), problem)?;
        let milp_sol = backend.solve()?;

        if !milp_sol.feasible {
            return Ok(infeasible_solution());
        }

        // Decode decision-variable assignments from indicator z's. Pick the
        // indicator with the highest z value (typically exactly one is 1.0).
        let mut assignments: HashMap<VariableName, f64> = HashMap::new();
        for dv in &problem.decision_variables {
            if let Some(indicators) = encoded.decision_indicators.get(&dv.name) {
                let mut best: Option<(f64, f64)> = None;
                for &(zid, v) in indicators {
                    let zv = milp_sol.assignments.get(&zid).copied().unwrap_or(0.0);
                    if best.is_none_or(|(b, _)| zv > b) {
                        best = Some((zv, v));
                    }
                }
                if let Some((_, v)) = best {
                    assignments.insert(dv.name.clone(), v);
                }
            }
        }

        Ok(Solution {
            assignments,
            objective_value: milp_sol.objective_value,
            feasible: true,
            evaluation_failures: 0,
            total_combinations: 0,
            first_evaluation_error: None,
        })
    }
}

fn infeasible_solution() -> Solution {
    Solution {
        assignments: HashMap::new(),
        objective_value: f64::NAN,
        feasible: false,
        evaluation_failures: 0,
        total_combinations: 0,
        first_evaluation_error: None,
    }
}
