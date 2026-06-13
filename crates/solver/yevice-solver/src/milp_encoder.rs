//! Translate an `OptimizationProblem` into a stream of `MilpBackend` calls.
//!
//! This module is backend-agnostic: it takes any `&mut dyn MilpBackend` and
//! emits `add_var` / `add_constraint` / `set_sense` calls. The HiGHS adapter
//! plugs in via [`crate::highs_backend`]. Other backends (CBC, GLOP, ...)
//! can reuse this encoder unchanged.
//!
//! ## Encoded shapes
//!
//! - **Decision variables**: each discrete decision variable becomes a set of
//!   binary indicators with a `Σ z_i = 1` constraint; the variable's value is
//!   the inner product `Σ value_i · z_i`. The shadow continuous variable lets
//!   us re-use the same `VarId` everywhere `Variable { name }` is referenced.
//! - **Linear (`as_linear()` returns `Some`)**: inlined directly.
//! - **`Tiered`**: Incremental (fill) formulation with binary activators.
//! - **`Max(expr, k)`**: big-M with `m ≥ expr`, `m ≥ k`, `m ≤ expr + M(1-z)`,
//!   `m ≤ k + M z`.
//! - **`Min(expr, k)`**: dual of Max (reverse signs).
//! - **`Ceil(expr)`**: integer aux var `y` with `expr ≤ y`. ADR-0002 case (Z)
//!   — only safe under contexts checked by `classify_ceil_context` (called by
//!   `crate::highs_backend::solve` before encoding).
//! - **`Product`**: at most one variable-containing factor; the factor's
//!   linearization is scaled by the constant product of the other factors.
//! - **`Div`**: denominator must be constant.

use std::collections::BTreeMap;

use yevice_core::expr::Expr;
use yevice_core::expr_introspect::{VarRanges, expr_bounds, substitute_fixed_params};
use yevice_core::optimize::{
    DecisionVariable, ObjectiveDirection, OptimizationConstraint, OptimizationProblem, Relation,
};
use yevice_core::types::VariableName;

use crate::error::SolverError;
use crate::milp::{ConstraintSense, MilpBackend, Sense, VarId, VarType};

/// Encoder state: maps every `VariableName` referenced anywhere in the
/// problem (after bindings expansion) to a backend `VarId`. Aux variables
/// (binary indicators, Max/Min/Ceil shadows, tiered fill vars) are tracked
/// here too — they get synthetic names like `__aux_max_3` to keep the
/// mapping uniform.
struct Encoder<'a> {
    backend: &'a mut dyn MilpBackend,
    /// VariableName → backend handle.
    var_index: BTreeMap<VariableName, VarId>,
    /// Ranges used for big-M derivation.
    ranges: VarRanges,
    /// Counter for synthetic aux variable names.
    aux_counter: u32,
}

/// Linear combination over backend `VarId`s plus a constant.
#[derive(Debug, Clone, Default)]
struct LinearTerms {
    coeffs: BTreeMap<VarId, f64>,
    constant: f64,
}

impl LinearTerms {
    fn from_const(value: f64) -> Self {
        Self {
            coeffs: BTreeMap::new(),
            constant: value,
        }
    }
    fn from_var(id: VarId, coeff: f64) -> Self {
        let mut coeffs = BTreeMap::new();
        coeffs.insert(id, coeff);
        Self {
            coeffs,
            constant: 0.0,
        }
    }
    fn scale(mut self, factor: f64) -> Self {
        if factor == 0.0 {
            return Self::default();
        }
        for v in self.coeffs.values_mut() {
            *v *= factor;
        }
        self.constant *= factor;
        self
    }
    fn add(mut self, other: Self) -> Self {
        for (id, c) in other.coeffs {
            *self.coeffs.entry(id).or_insert(0.0) += c;
        }
        self.constant += other.constant;
        self
    }
    fn sub(self, other: Self) -> Self {
        self.add(other.scale(-1.0))
    }
}

impl<'a> Encoder<'a> {
    fn new(backend: &'a mut dyn MilpBackend, ranges: VarRanges) -> Self {
        Self {
            backend,
            var_index: BTreeMap::new(),
            ranges,
            aux_counter: 0,
        }
    }

    /// Register a "real" variable name and return its handle.
    /// `lower` / `upper` describe the variable's natural domain.
    fn register_var(
        &mut self,
        name: VariableName,
        lower: f64,
        upper: f64,
        var_type: VarType,
    ) -> VarId {
        if let Some(&id) = self.var_index.get(&name) {
            return id;
        }
        let id = self.backend.add_var(lower, upper, 0.0, var_type);
        self.var_index.insert(name, id);
        id
    }

    /// Allocate an anonymous auxiliary variable.
    fn alloc_aux(&mut self, lower: f64, upper: f64, var_type: VarType) -> VarId {
        self.aux_counter += 1;
        self.backend.add_var(lower, upper, 0.0, var_type)
    }
}

/// Encode the whole problem onto `backend`, then run `solve`.
///
/// The caller is responsible for:
/// - `validate_bindings(problem)` before encoding (we expect every var
///   referenced in the objective / constraints to be resolvable).
/// - `expr_is_linearizable` pre-check on every expression.
/// - `classify_ceil_context(problem)` pre-check.
///
/// Returns the mapping `VariableName → VarId` together with the backend's
/// solution, so callers can read out decision-variable values.
pub(crate) struct EncodedProblem {
    /// Reverse mapping `VariableName → VarId`. Kept for diagnostics; not
    /// consumed by the default `HighsSolver` decoding path.
    #[allow(dead_code)]
    pub var_index: BTreeMap<VariableName, VarId>,
    pub decision_indicators: BTreeMap<VariableName, Vec<(VarId, f64)>>,
}

pub(crate) fn encode(
    backend: &mut dyn MilpBackend,
    problem: &OptimizationProblem,
) -> Result<EncodedProblem, SolverError> {
    let mut enc = Encoder::new(backend, build_ranges(problem));

    // Inline fixed parameters as constants into every expression we will
    // encode below. This unblocks shapes like `price * x` where `price` is
    // a fixed param and `x` is the decision variable — the surrounding
    // Product would otherwise count both as variable-containing and the
    // encoder would surface `SolverError::Nonlinear`. The `as_linear` /
    // big-M code paths all benefit from the same simplification.
    // Exclude any fixed_param whose name collides with a decision variable
    // — decision wins per ADR-0002 / EnumerationSolver semantics, so we
    // must NOT inline the fixed value at those names.
    let decision_names: std::collections::BTreeSet<VariableName> = problem
        .decision_variables
        .iter()
        .map(|dv| dv.name.clone())
        .collect();
    let fixed_param_map: BTreeMap<VariableName, f64> = problem
        .fixed_params
        .iter()
        .filter(|(k, _)| !decision_names.contains(*k))
        .map(|(k, &v)| (k.clone(), v))
        .collect();
    let normalise = |e: &Expr| -> Expr { substitute_fixed_params(e, &fixed_param_map) };
    let normalised_objective = normalise(&problem.objective);
    let normalised_constraints: Vec<(Expr, &OptimizationConstraint)> = problem
        .constraints
        .iter()
        .map(|c| (normalise(&c.lhs), c))
        .collect();
    let normalised_bindings: Vec<Expr> = problem
        .bindings
        .iter()
        .map(|b| normalise(&b.expr))
        .collect();

    // 1) Register decision variables as binary-indicator combinations.
    //
    // Duplicate names: `EnumerationSolver` documents "last-write-wins"
    // semantics (a later slot overwrites earlier values in the scratch
    // map). The MILP encoder honours the same rule by keeping only the
    // LAST slot per name; encoding multiple indicator sets with the same
    // value variable would intersect domains and break feasibility for
    // problems the enumerator accepts.
    let mut last_slot_by_name: BTreeMap<&VariableName, &DecisionVariable> = BTreeMap::new();
    for dv in &problem.decision_variables {
        last_slot_by_name.insert(&dv.name, dv);
    }
    let mut decision_indicators: BTreeMap<VariableName, Vec<(VarId, f64)>> = BTreeMap::new();
    for dv in last_slot_by_name.values() {
        let indicators = register_decision_var(&mut enc, dv)?;
        decision_indicators.insert(dv.name.clone(), indicators);
    }

    // 2) Register fixed parameters as fixed (lower == upper) continuous vars.
    //    This keeps the encoder uniform: every Expr::Variable lookup goes
    //    through `var_index`.
    for (name, &val) in &problem.fixed_params {
        if enc.var_index.contains_key(name) {
            // Decision-variable / fixed-param collision: decision wins per
            // EnumerationSolver's documented semantics (see bug-1 regression
            // test). The encoder follows the same rule.
            continue;
        }
        enc.register_var(name.clone(), val, val, VarType::Continuous);
    }

    // 3) Encode bindings in two passes so the order of `problem.bindings`
    //    does not matter. Pass A allocates a `VarId` for every binding
    //    target (with broad default bounds — the RHS may reference targets
    //    not yet seen). Pass B encodes each RHS and emits the linking
    //    equality `target == linearize(expr)`. This matches the fixed-point
    //    semantics enforced by `validate_bindings` and `EnumerationSolver`.
    let mut binding_target_ids: Vec<Option<VarId>> = Vec::with_capacity(problem.bindings.len());
    for binding in &problem.bindings {
        if enc.var_index.contains_key(&binding.target) {
            // Target shadowed by fixed_param or decision_var → skip.
            binding_target_ids.push(None);
            continue;
        }
        // Default to a very wide interval; bindings whose RHS we can already
        // bound tightly will see tighter ranges after `expr_bounds` is
        // applied in pass B (we cannot do it here because the RHS may
        // reference other not-yet-registered targets).
        let target_id = enc.backend.add_var(-1e30, 1e30, 0.0, VarType::Continuous);
        enc.var_index.insert(binding.target.clone(), target_id);
        binding_target_ids.push(Some(target_id));
    }
    // Pass B(.0): propagate ranges in a fixed-point loop so a binding
    //   whose RHS depends on a *later* binding still picks up a finite
    //   interval before pass B(.1) encodes RHS expressions (which use the
    //   ranges for big-M derivation in any nested Max / Min / Tiered).
    //   `validate_bindings` has already proven the graph is acyclic, so
    //   the loop terminates within `bindings.len()` passes.
    let max_passes = problem.bindings.len() + 1;
    for _ in 0..max_passes {
        let mut progressed = false;
        for ((binding, target_opt), rhs_expr) in problem
            .bindings
            .iter()
            .zip(binding_target_ids.iter())
            .zip(normalised_bindings.iter())
        {
            if target_opt.is_none() {
                continue;
            }
            let already_known = enc.ranges.decision_var_ranges.get(&binding.target).copied();
            let bounds = expr_bounds(rhs_expr, &enc.ranges);
            if !bounds.is_finite() {
                continue;
            }
            let new_range = (bounds.lower, bounds.upper);
            if already_known != Some(new_range) {
                enc.ranges
                    .decision_var_ranges
                    .insert(binding.target.clone(), new_range);
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }
    // Pass B(.1): encode each RHS once ranges are stable. Bindings whose
    // target is not transitively referenced from objective / constraint
    // expressions are skipped — the enumerator silently ignores unused
    // bindings, and we match that behaviour to avoid spurious Nonlinear /
    // UnboundVariables errors on dead RHS expressions.
    let reachable_targets = reachable_binding_targets(
        &normalised_objective,
        normalised_constraints.iter().map(|(lhs, _)| lhs),
        &problem.bindings,
        &normalised_bindings,
    );
    for ((binding, target_opt), rhs_expr) in problem
        .bindings
        .iter()
        .zip(binding_target_ids.iter())
        .zip(normalised_bindings.iter())
    {
        let Some(target_id) = *target_opt else {
            continue;
        };
        if !reachable_targets.contains(&binding.target) {
            continue;
        }
        let lhs = encode_expr(&mut enc, rhs_expr)?;
        let combined = LinearTerms::from_var(target_id, 1.0).sub(lhs);
        emit_linear_eq(enc.backend, &combined, 0.0);
    }

    // 4) Encode objective.
    let obj_terms = encode_expr(&mut enc, &normalised_objective)?;
    set_objective(enc.backend, &obj_terms);

    // 5) Encode constraints: `lhs <relation> rhs`.
    for (lhs_expr, c) in &normalised_constraints {
        let lhs = encode_expr(&mut enc, lhs_expr)?;
        emit_constraint(enc.backend, &lhs, c);
    }

    // 6) Sense.
    enc.backend.set_sense(match problem.direction {
        ObjectiveDirection::Minimize => Sense::Minimize,
        ObjectiveDirection::Maximize => Sense::Maximize,
    });

    Ok(EncodedProblem {
        var_index: enc.var_index,
        decision_indicators,
    })
}

/// Compute the set of binding-target names that the encoder actually needs
/// to materialise, transitively through bindings.
///
/// The walk starts from variable references in the (normalised) objective
/// and every constraint LHS. Whenever we hit a binding target, we expand
/// it by walking the binding's RHS variables. Bindings whose target never
/// surfaces in this closure can be dropped — they would otherwise force
/// the encoder to emit a linking equality for an RHS whose value the
/// solution never depends on.
fn reachable_binding_targets<'a>(
    objective: &Expr,
    constraint_lhss: impl Iterator<Item = &'a Expr>,
    bindings: &[yevice_core::cost::VariableBinding],
    normalised_bindings: &[Expr],
) -> std::collections::HashSet<VariableName> {
    use std::collections::HashSet;
    let mut seen: HashSet<VariableName> = HashSet::new();
    let mut frontier: Vec<VariableName> = objective.variables().into_iter().collect();
    for lhs in constraint_lhss {
        frontier.extend(lhs.variables());
    }
    while let Some(name) = frontier.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        for (b, rhs) in bindings.iter().zip(normalised_bindings.iter()) {
            if b.target == name {
                for v in rhs.variables() {
                    if !seen.contains(&v) {
                        frontier.push(v);
                    }
                }
            }
        }
    }
    seen
}

/// Build the `VarRanges` used for big-M derivation.
fn build_ranges(problem: &OptimizationProblem) -> VarRanges {
    let mut ranges = VarRanges::default();
    for dv in &problem.decision_variables {
        if dv.domain.is_empty() {
            continue;
        }
        let lo = dv.domain.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = dv.domain.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        ranges.decision_var_ranges.insert(dv.name.clone(), (lo, hi));
    }
    for (name, &v) in &problem.fixed_params {
        ranges.fixed_params.insert(name.clone(), v);
    }
    ranges
}

/// Encode a single decision variable as a 1-of-N binary indicator set.
///
/// Adds:
/// - one continuous "value" variable (lower = min domain, upper = max domain)
/// - one binary indicator per domain value
/// - a `Σ z_i = 1` constraint
/// - a `value = Σ v_i z_i` linking constraint
///
/// Returns the indicator handles paired with their domain values so the
/// caller can decode the solution.
fn register_decision_var(
    enc: &mut Encoder<'_>,
    dv: &DecisionVariable,
) -> Result<Vec<(VarId, f64)>, SolverError> {
    if dv.domain.is_empty() {
        // Treat as infeasible — caller handles this by checking the solver
        // result. We still add a single 0 variable to keep the encoder
        // moving; the constraint `0 = 1` from the indicator sum below will
        // make the problem infeasible at solve time.
    }
    let lo = dv.domain.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = dv.domain.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let (lo, hi) = if dv.domain.is_empty() {
        (0.0, 0.0)
    } else {
        (lo, hi)
    };
    let value_id = enc.register_var(dv.name.clone(), lo, hi, VarType::Continuous);

    let mut indicators: Vec<(VarId, f64)> = Vec::with_capacity(dv.domain.len());
    for &v in &dv.domain {
        let z = enc.backend.add_var(0.0, 1.0, 0.0, VarType::Binary);
        indicators.push((z, v));
    }

    // Σ z_i = 1
    let terms: Vec<(VarId, f64)> = indicators.iter().map(|&(z, _)| (z, 1.0)).collect();
    enc.backend.add_constraint(&terms, ConstraintSense::Eq, 1.0);

    // value - Σ v_i z_i = 0
    let mut link: Vec<(VarId, f64)> = vec![(value_id, 1.0)];
    for &(z, v) in &indicators {
        link.push((z, -v));
    }
    enc.backend.add_constraint(&link, ConstraintSense::Eq, 0.0);

    Ok(indicators)
}

/// Lower an `Expr` into a `LinearTerms` over backend `VarId`s, allocating
/// auxiliary variables as needed for Tiered / Ceil / Max / Min.
fn encode_expr(enc: &mut Encoder<'_>, expr: &Expr) -> Result<LinearTerms, SolverError> {
    match expr {
        Expr::Constant { value } => Ok(LinearTerms::from_const(*value)),

        Expr::Variable { name } => match enc.var_index.get(name) {
            Some(&id) => Ok(LinearTerms::from_var(id, 1.0)),
            None => Err(SolverError::UnboundVariables {
                variables: vec![name.to_string()],
            }),
        },

        Expr::Linear { coeff, var, offset } => {
            let inner = encode_expr(enc, var)?;
            let mut out = inner.scale(*coeff);
            out.constant += offset;
            Ok(out)
        }

        Expr::Sum { exprs } => {
            let mut acc = LinearTerms::default();
            for e in exprs {
                acc = acc.add(encode_expr(enc, e)?);
            }
            Ok(acc)
        }

        Expr::Product { exprs } => {
            // At most one variable-containing factor; everything else is
            // collapsed into a constant multiplier.
            let mut const_factor = 1.0;
            let mut var_factor: Option<LinearTerms> = None;
            for e in exprs {
                let lt = encode_expr(enc, e)?;
                if lt.coeffs.is_empty() {
                    const_factor *= lt.constant;
                } else if var_factor.is_some() {
                    return Err(SolverError::Nonlinear {
                        expr: format!("{expr:?}"),
                    });
                } else {
                    var_factor = Some(lt);
                }
            }
            Ok(match var_factor {
                None => LinearTerms::from_const(const_factor),
                Some(lt) => lt.scale(const_factor),
            })
        }

        Expr::Div {
            numerator,
            denominator,
        } => {
            let d = encode_expr(enc, denominator)?;
            if !d.coeffs.is_empty() || d.constant == 0.0 {
                return Err(SolverError::Nonlinear {
                    expr: format!("{expr:?}"),
                });
            }
            let n = encode_expr(enc, numerator)?;
            Ok(n.scale(1.0 / d.constant))
        }

        Expr::Ceil { expr: inner } => encode_ceil(enc, inner),

        Expr::Max { expr: inner, floor } => encode_max(enc, inner, *floor),

        Expr::Min {
            expr: inner,
            ceiling,
        } => encode_min(enc, inner, *ceiling),

        Expr::Tiered { tiers, var } => encode_tiered(enc, tiers, var),

        // Expr is #[non_exhaustive]; reject any future variant explicitly.
        _ => Err(SolverError::Nonlinear {
            expr: format!("{expr:?}"),
        }),
    }
}

/// Ceil: introduce integer aux `y`, constrain `expr <= y`. ADR-0002 case (Z).
/// Caller has already invoked `classify_ceil_context` so this is safe.
fn encode_ceil(enc: &mut Encoder<'_>, inner: &Expr) -> Result<LinearTerms, SolverError> {
    let inner_terms = encode_expr(enc, inner)?;
    // Bound y by the inner expression's interval (rounded up on the high side).
    let b = expr_bounds(inner, &enc.ranges);
    let lo = if b.lower.is_finite() {
        b.lower.ceil()
    } else {
        -1e30
    };
    let hi = if b.upper.is_finite() {
        b.upper.ceil()
    } else {
        1e30
    };
    let y = enc.alloc_aux(lo, hi, VarType::Integer);
    // inner - y <= 0   (i.e. inner <= y)
    let constraint_terms = inner_terms.sub(LinearTerms::from_var(y, 1.0));
    emit_linear_le(enc.backend, &constraint_terms, 0.0);
    Ok(LinearTerms::from_var(y, 1.0))
}

/// Max(expr, floor): big-M with binary selector z.
/// We add `m`, the auxiliary, with `m ≥ expr`, `m ≥ floor`,
/// `m ≤ expr + M·(1 - z)`, `m ≤ floor + M·z`.
fn encode_max(enc: &mut Encoder<'_>, inner: &Expr, floor: f64) -> Result<LinearTerms, SolverError> {
    let inner_terms = encode_expr(enc, inner)?;
    let b = expr_bounds(inner, &enc.ranges);
    if !b.is_finite() {
        return Err(SolverError::UnboundedExpression {
            expr: format!("{inner:?}"),
        });
    }
    let m_lo = b.lower.max(floor);
    let m_hi = b.upper.max(floor);
    let m = enc.alloc_aux(m_lo, m_hi, VarType::Continuous);
    let z = enc.alloc_aux(0.0, 1.0, VarType::Binary);

    // big-M must dominate the slack of the *disabled* side of every
    // inequality. For z=0 (selecting `expr`):
    //   - `m ≤ floor + M·z` collapses to `m ≤ floor + 0`. The active side
    //     `m = expr` can reach `b.upper`, so M ≥ b.upper - floor.
    // For z=1 (selecting `floor`):
    //   - `m ≤ expr + M·(1 - z)` collapses to `m ≤ expr`. The active side
    //     `m = floor` must not be cut off when `expr` can dip down to
    //     `b.lower`, so M ≥ floor - b.lower.
    // Take the max of both legs (and the interval width, for headroom).
    let big_m = (b.upper - floor)
        .abs()
        .max((floor - b.lower).abs())
        .max((b.upper - b.lower).abs())
        .max(1.0)
        + 1.0;

    // m - inner >= 0
    let t1 = LinearTerms::from_var(m, 1.0).sub(inner_terms.clone());
    emit_linear_ge(enc.backend, &t1, 0.0);
    // m >= floor
    emit_linear_ge(enc.backend, &LinearTerms::from_var(m, 1.0), floor);

    // m - inner - M*(1 - z) <= 0   ⇔   m - inner + M z <= M
    let t3 = LinearTerms::from_var(m, 1.0)
        .sub(inner_terms.clone())
        .add(LinearTerms::from_var(z, big_m));
    emit_linear_le(enc.backend, &t3, big_m);
    // m - floor - M z <= 0
    let t4 = LinearTerms::from_var(m, 1.0).add(LinearTerms::from_var(z, -big_m));
    emit_linear_le(enc.backend, &t4, floor);

    Ok(LinearTerms::from_var(m, 1.0))
}

/// Min(expr, ceiling): dual of Max with reversed signs.
fn encode_min(
    enc: &mut Encoder<'_>,
    inner: &Expr,
    ceiling: f64,
) -> Result<LinearTerms, SolverError> {
    let inner_terms = encode_expr(enc, inner)?;
    let b = expr_bounds(inner, &enc.ranges);
    if !b.is_finite() {
        return Err(SolverError::UnboundedExpression {
            expr: format!("{inner:?}"),
        });
    }
    let m_lo = b.lower.min(ceiling);
    let m_hi = b.upper.min(ceiling);
    let m = enc.alloc_aux(m_lo, m_hi, VarType::Continuous);
    let z = enc.alloc_aux(0.0, 1.0, VarType::Binary);

    // Same two-leg derivation as encode_max, dual sign.
    let big_m = (b.upper - ceiling)
        .abs()
        .max((ceiling - b.lower).abs())
        .max((b.upper - b.lower).abs())
        .max((ceiling - b.lower).abs())
        .max(1.0)
        + 1.0;

    // m - inner <= 0
    let t1 = LinearTerms::from_var(m, 1.0).sub(inner_terms.clone());
    emit_linear_le(enc.backend, &t1, 0.0);
    // m - ceiling <= 0
    emit_linear_le(enc.backend, &LinearTerms::from_var(m, 1.0), ceiling);

    // m - inner + M(1 - z) >= 0  ⇔  m - inner - M z >= -M
    let t3 = LinearTerms::from_var(m, 1.0)
        .sub(inner_terms.clone())
        .add(LinearTerms::from_var(z, -big_m));
    emit_linear_ge(enc.backend, &t3, -big_m);
    // m - ceiling + M z >= 0
    let t4 = LinearTerms::from_var(m, 1.0).add(LinearTerms::from_var(z, big_m));
    emit_linear_ge(enc.backend, &t4, ceiling);

    Ok(LinearTerms::from_var(m, 1.0))
}

/// Tiered: Incremental (fill) formulation.
///
/// For an input expression `u` (already a `LinearTerms` after encoding) and
/// tiers `t_1, …, t_n`, we introduce:
/// - `q_i ∈ [0, width_i]` continuous fill per tier (width_i = upper_i - upper_{i-1};
///   for the unbounded tail tier we use a generous big-M-style upper bound).
/// - `z_i ∈ {0,1}` activator per tier with the chain `z_i ≥ z_{i+1}`,
///   `q_i ≤ width_i · z_i`, `q_i ≥ width_i · z_{i+1}` (the second forces a
///   lower tier to be "full" before the next can be entered).
/// - link: `u = Σ q_i` (encoded as `inner_terms - Σ q_i = 0`).
///
/// The encoded value (Tiered's cost) is `Σ price_i · q_i`.
fn encode_tiered(
    enc: &mut Encoder<'_>,
    tiers: &[yevice_core::expr::Tier],
    inner: &Expr,
) -> Result<LinearTerms, SolverError> {
    // The evaluator returns 0 for an empty tier list without constraining
    // the input expression. Mirror that here so `tiered([], x)` does not
    // silently force `x = 0` via the `Σ q_i = u` link below.
    if tiers.is_empty() {
        return Ok(LinearTerms::from_const(0.0));
    }
    let raw_inner_terms = encode_expr(enc, inner)?;
    let b = expr_bounds(inner, &enc.ranges);
    let usage_upper = if b.upper.is_finite() {
        b.upper.max(0.0)
    } else {
        return Err(SolverError::UnboundedExpression {
            expr: format!("{inner:?}"),
        });
    };

    // The evaluator clamps negative usage to 0 (it exits the tier loop as
    // soon as `remaining <= 0`). The MILP equality `Σ q_i = inner` would
    // make a negative inner value infeasible since `q_i ≥ 0`. Whenever the
    // inner's interval admits negative values, route through an auxiliary
    // `usage = max(inner, 0)` so the link uses the clamped value instead.
    let inner_terms = if b.lower < 0.0 && b.lower.is_finite() {
        // Reuse the Max encoder by emitting it inline with the existing
        // LinearTerms. Aux var `u ∈ [0, usage_upper]`, plus the same
        // big-M selector pattern as `encode_max`.
        let m_lo = 0.0f64;
        let m_hi = usage_upper;
        let u = enc.alloc_aux(m_lo, m_hi, VarType::Continuous);
        let z = enc.alloc_aux(0.0, 1.0, VarType::Binary);
        let big_m = (b.upper - 0.0)
            .abs()
            .max((0.0 - b.lower).abs())
            .max((b.upper - b.lower).abs())
            .max(1.0)
            + 1.0;
        // u >= inner
        let t1 = LinearTerms::from_var(u, 1.0).sub(raw_inner_terms.clone());
        emit_linear_ge(enc.backend, &t1, 0.0);
        // u >= 0  (already ensured by lower bound).
        // u <= inner + M(1 - z)
        let t3 = LinearTerms::from_var(u, 1.0)
            .sub(raw_inner_terms.clone())
            .add(LinearTerms::from_var(z, big_m));
        emit_linear_le(enc.backend, &t3, big_m);
        // u <= M z
        let t4 = LinearTerms::from_var(u, 1.0).add(LinearTerms::from_var(z, -big_m));
        emit_linear_le(enc.backend, &t4, 0.0);
        LinearTerms::from_var(u, 1.0)
    } else {
        raw_inner_terms
    };

    // Determine each tier's width.
    let mut prev_limit = 0.0;
    let mut widths: Vec<f64> = Vec::with_capacity(tiers.len());
    for tier in tiers {
        let width = match tier.upper_limit {
            Some(limit) => (limit - prev_limit).max(0.0),
            None => (usage_upper - prev_limit).max(0.0),
        };
        widths.push(width);
        if let Some(limit) = tier.upper_limit {
            prev_limit = limit;
        }
    }

    // Allocate q_i continuous, z_i binary.
    let mut qs: Vec<VarId> = Vec::with_capacity(tiers.len());
    let mut zs: Vec<VarId> = Vec::with_capacity(tiers.len());
    for &w in &widths {
        let q = enc.alloc_aux(0.0, w.max(0.0), VarType::Continuous);
        let z = enc.alloc_aux(0.0, 1.0, VarType::Binary);
        qs.push(q);
        zs.push(z);
    }

    // q_i ≤ width_i · z_i   ⇔   q_i - width_i z_i ≤ 0
    // q_i ≥ width_i · z_{i+1}  ⇔  q_i - width_i z_{i+1} ≥ 0
    // z_i ≥ z_{i+1}  ⇔  z_i - z_{i+1} ≥ 0
    for i in 0..tiers.len() {
        let q_i = qs[i];
        let z_i = zs[i];
        let w_i = widths[i];
        let terms_upper: Vec<(VarId, f64)> = vec![(q_i, 1.0), (z_i, -w_i)];
        enc.backend
            .add_constraint(&terms_upper, ConstraintSense::Le, 0.0);
        if i + 1 < tiers.len() {
            let z_next = zs[i + 1];
            let terms_lower: Vec<(VarId, f64)> = vec![(q_i, 1.0), (z_next, -w_i)];
            enc.backend
                .add_constraint(&terms_lower, ConstraintSense::Ge, 0.0);
            let terms_chain: Vec<(VarId, f64)> = vec![(z_i, 1.0), (z_next, -1.0)];
            enc.backend
                .add_constraint(&terms_chain, ConstraintSense::Ge, 0.0);
        }
    }

    // u = Σ q_i  ⇔  inner_terms - Σ q_i = 0
    let mut sum_q = LinearTerms::default();
    for &q in &qs {
        sum_q = sum_q.add(LinearTerms::from_var(q, 1.0));
    }
    let link = inner_terms.sub(sum_q);
    emit_linear_eq(enc.backend, &link, 0.0);

    // Returned linear combination: Σ price_i · q_i.
    let mut cost = LinearTerms::default();
    for (i, tier) in tiers.iter().enumerate() {
        cost = cost.add(LinearTerms::from_var(qs[i], tier.unit_price));
    }
    Ok(cost)
}

// ---------------------------------------------------------------------------
// Constraint / objective emission
// ---------------------------------------------------------------------------

fn set_objective(backend: &mut dyn MilpBackend, lt: &LinearTerms) {
    // The MilpBackend trait sets the objective by passing per-variable
    // coefficients on `add_var`. Since the encoder allocates vars before it
    // knows the final objective coefficients, we model the objective as a
    // free continuous variable `obj_value` with coefficient 1, plus an
    // equality constraint `obj_value = Σ c_i x_i + constant`.
    //
    // This costs one extra variable + one extra constraint but keeps
    // `add_var(.., objective_coeff, ..)` honest. The objective constant is
    // absorbed into the bound of `obj_value`.
    let obj_var = backend.add_var(-1e30, 1e30, 1.0, VarType::Continuous);
    let mut terms: Vec<(VarId, f64)> = vec![(obj_var, 1.0)];
    for (&id, &c) in &lt.coeffs {
        terms.push((id, -c));
    }
    backend.add_constraint(&terms, ConstraintSense::Eq, lt.constant);
}

fn emit_linear_le(backend: &mut dyn MilpBackend, lt: &LinearTerms, rhs_addition: f64) {
    let terms: Vec<(VarId, f64)> = lt.coeffs.iter().map(|(&id, &c)| (id, c)).collect();
    backend.add_constraint(&terms, ConstraintSense::Le, rhs_addition - lt.constant);
}

fn emit_linear_ge(backend: &mut dyn MilpBackend, lt: &LinearTerms, rhs_addition: f64) {
    let terms: Vec<(VarId, f64)> = lt.coeffs.iter().map(|(&id, &c)| (id, c)).collect();
    backend.add_constraint(&terms, ConstraintSense::Ge, rhs_addition - lt.constant);
}

fn emit_linear_eq(backend: &mut dyn MilpBackend, lt: &LinearTerms, rhs_addition: f64) {
    let terms: Vec<(VarId, f64)> = lt.coeffs.iter().map(|(&id, &c)| (id, c)).collect();
    backend.add_constraint(&terms, ConstraintSense::Eq, rhs_addition - lt.constant);
}

fn emit_constraint(backend: &mut dyn MilpBackend, lhs: &LinearTerms, c: &OptimizationConstraint) {
    let sense = match c.relation {
        Relation::Le => ConstraintSense::Le,
        Relation::Ge => ConstraintSense::Ge,
        Relation::Eq => ConstraintSense::Eq,
    };
    let terms: Vec<(VarId, f64)> = lhs.coeffs.iter().map(|(&id, &cc)| (id, cc)).collect();
    backend.add_constraint(&terms, sense, c.rhs - lhs.constant);
}
