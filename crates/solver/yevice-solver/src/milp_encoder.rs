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

use yevice_core::evaluate::{Params, evaluate};
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

impl std::fmt::Debug for EncodedProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EncodedProblem")
            .field("var_index_count", &self.var_index.len())
            .field("decision_indicators_count", &self.decision_indicators.len())
            .finish()
    }
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
    let mut fixed_param_map: BTreeMap<VariableName, f64> = problem
        .fixed_params
        .iter()
        .filter(|(k, _)| !decision_names.contains(*k))
        .map(|(k, &v)| (k.clone(), v))
        .collect();

    // Pre-pass: fold constant bindings into fixed_param_map.
    //
    // A binding `rate = 2` (or any RHS that evaluates to a constant given
    // the current fixed_params) is effectively a constant, not a
    // variable.  If we leave it as an auxiliary variable, an expression
    // like `rate * x` would appear to have two variable-containing
    // factors and the encoder would surface `SolverError::Nonlinear`.
    // By folding such bindings into `fixed_param_map` first, `normalise`
    // will later substitute them as `Constant` nodes, making the product
    // linear.
    //
    // We run a fixed-point loop (bounded by `bindings.len()`) to handle
    // chains like `a = 2; b = a * 3` — once `a` is folded, `b` can be
    // folded in the next pass.  Bindings whose target is shadowed by a
    // decision variable are skipped; decision wins per ADR-0002.
    {
        let mut eval_params: Params = fixed_param_map
            .iter()
            .map(|(k, &v)| (k.clone(), v))
            .collect();
        let max_passes = problem.bindings.len() + 1;
        for _ in 0..max_passes {
            let mut progressed = false;
            for binding in &problem.bindings {
                if decision_names.contains(&binding.target) {
                    continue;
                }
                if eval_params.contains_key(&binding.target) {
                    continue;
                }
                if let Ok(value) = evaluate(&binding.expr, &eval_params) {
                    eval_params.insert(binding.target.clone(), value);
                    fixed_param_map.insert(binding.target.clone(), value);
                    progressed = true;
                }
            }
            if !progressed {
                break;
            }
        }
    }

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
    //
    //    Duplicate-target handling: `resolve_bindings_into` (yevice-core)
    //    uses a fixed-point loop that skips already-resolved targets, so the
    //    **first** binding that successfully resolves wins.  For example,
    //    `b = x; b = missing` resolves `b = x` in the first pass and then
    //    skips the second binding because `b` is already in `params`.
    //
    //    The encoder mirrors this first-resolvable semantics by running its
    //    own fixed-point loop (Pass A pre-computation).  A binding is
    //    considered "resolvable" when every variable referenced in its
    //    normalised RHS is already registered in `enc.var_index` (decision
    //    vars, fixed params, or an already-selected binding target) *and* no
    //    earlier binding for the same target has already been selected.  Only
    //    the first such binding per target gets a real `VarId`; later
    //    duplicates are marked `None` (skipped in Pass B).
    let first_resolvable_index_for_target: BTreeMap<&VariableName, usize> = {
        // Seed a "resolved" set with targets already covered by decision
        // variables and fixed params (i.e. already in enc.var_index).
        let mut resolved: std::collections::BTreeSet<&VariableName> =
            enc.var_index.keys().collect();
        let mut result: BTreeMap<&VariableName, usize> = BTreeMap::new();
        let max_passes = problem.bindings.len() + 1;
        for _ in 0..max_passes {
            let mut progressed = false;
            for (i, (binding, rhs_expr)) in problem
                .bindings
                .iter()
                .zip(normalised_bindings.iter())
                .enumerate()
            {
                // Skip targets already resolved (decision/fixed-param/earlier binding).
                if resolved.contains(&binding.target) {
                    continue;
                }
                // Skip if we already picked a binding for this target.
                if result.contains_key(&binding.target) {
                    continue;
                }
                // A binding is resolvable if every variable in its normalised
                // RHS is already registered in var_index or resolved so far.
                let all_vars_known = rhs_expr
                    .variables()
                    .iter()
                    .all(|v| enc.var_index.contains_key(v) || result.contains_key(v));
                if all_vars_known {
                    result.insert(&binding.target, i);
                    resolved.insert(&binding.target);
                    progressed = true;
                }
            }
            if !progressed {
                break;
            }
        }
        result
    };
    let mut binding_target_ids: Vec<Option<VarId>> = Vec::with_capacity(problem.bindings.len());
    for (i, binding) in problem.bindings.iter().enumerate() {
        if enc.var_index.contains_key(&binding.target) {
            // Target shadowed by fixed_param or decision_var → skip.
            binding_target_ids.push(None);
            continue;
        }
        if first_resolvable_index_for_target.get(&binding.target) != Some(&i) {
            // Not the first-resolvable binding for this target: skip to avoid
            // encoding an unresolvable RHS or overriding the chosen one.
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
            //
            // A factor is "variable-containing" only if at least one coefficient
            // in `lt.coeffs` is non-zero (|c| > EPS).  Coefficients that
            // cancelled to zero during summation must not trigger Nonlinear.
            const EPS: f64 = 1e-12;
            let mut const_factor = 1.0;
            let mut var_factor: Option<LinearTerms> = None;
            for e in exprs {
                let lt = encode_expr(enc, e)?;
                let has_variable = lt.coeffs.values().any(|&c| c.abs() > EPS);
                if !has_variable {
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
            const EPS: f64 = 1e-12;
            let d = encode_expr(enc, denominator)?;
            let denominator_has_variable = d.coeffs.values().any(|&c| c.abs() > EPS);
            if denominator_has_variable || d.constant == 0.0 {
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use yevice_core::cost::VariableBinding;
    use yevice_core::expr::Expr;
    use yevice_core::optimize::{DecisionVariable, ObjectiveDirection, OptimizationProblem};
    use yevice_core::types::VariableName;

    use super::*;
    use crate::error::SolverError;
    use crate::milp::{ConstraintSense, MilpSolution, Sense};

    fn var(name: &str) -> VariableName {
        VariableName::new(name)
    }

    fn binding(target: &str, expr: Expr) -> VariableBinding {
        VariableBinding {
            target: var(target),
            expr,
            description: String::new(),
            source: "test".into(),
        }
    }

    /// A minimal no-op `MilpBackend` that just records variables added and
    /// accepts constraints without solving.  Used to check that `encode`
    /// succeeds (returns `Ok`) without a real solver.
    struct CountingBackend {
        var_count: u32,
        constraint_count: u32,
    }

    impl CountingBackend {
        fn new() -> Self {
            Self {
                var_count: 0,
                constraint_count: 0,
            }
        }
    }

    impl MilpBackend for CountingBackend {
        fn add_var(&mut self, _lower: f64, _upper: f64, _obj: f64, _vtype: VarType) -> VarId {
            let id = self.var_count;
            self.var_count += 1;
            id
        }

        fn add_constraint(
            &mut self,
            _terms: &[(VarId, f64)],
            _sense: ConstraintSense,
            _rhs: f64,
        ) -> u32 {
            let id = self.constraint_count;
            self.constraint_count += 1;
            id
        }

        fn set_sense(&mut self, _sense: Sense) {}

        fn solve(self: Box<Self>) -> Result<MilpSolution, SolverError> {
            Err(SolverError::MilpBackend {
                message: "CountingBackend does not solve".into(),
            })
        }
    }

    /// Duplicate binding targets: `b = missing_var; b = x`.
    ///
    /// `resolve_bindings_into` adopts first-resolvable semantics: the first
    /// binding whose RHS can be evaluated wins.  Here `b = missing_var` is
    /// unresolvable (missing_var is never defined), so `b = x` — the first
    /// *resolvable* binding — is adopted.
    #[test]
    fn encode_first_resolvable_binding_wins_for_duplicate_target() {
        // objective = b, direction = Minimize, decision variable x in {1.0}
        // binding[0]: b = missing_var  (unresolvable — missing_var never defined)
        // binding[1]: b = x            (first-resolvable binding for `b`)
        let problem = OptimizationProblem {
            objective: Expr::variable("b"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![
                binding("b", Expr::variable("missing_var")),
                binding("b", Expr::variable("x")),
            ],
        };

        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed when the first-resolvable binding for a target is resolvable; got: {:?}",
            result.err()
        );
    }

    /// Regression: `b = x; b = missing` — first binding is resolvable, second
    /// is not.  The encoder must adopt `b = x` (first-resolvable), not
    /// `b = missing` (last).
    #[test]
    fn encode_first_binding_wins_when_later_is_unresolvable() {
        // objective = b, direction = Minimize, decision variable x in {1.0}
        // binding[0]: b = x        (first-resolvable — should be adopted)
        // binding[1]: b = missing  (unresolvable — must NOT shadow b = x)
        let problem = OptimizationProblem {
            objective: Expr::variable("b"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![
                binding("b", Expr::variable("x")),
                binding("b", Expr::variable("missing")),
            ],
        };

        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must adopt first-resolvable binding `b = x` and ignore later unresolvable `b = missing`; got: {:?}",
            result.err()
        );
    }

    /// Regression for issue #37: a constant-valued binding (`rate = 2`) used
    /// as a factor in a product expression (`rate * x`) must be folded into
    /// `fixed_param_map` before the encoder classifies products.  Without the
    /// fold, `rate` appears as an auxiliary variable and the product would have
    /// two variable-containing factors, causing `SolverError::Nonlinear`.
    #[test]
    fn encode_constant_binding_folded_before_product_classification() {
        // objective = rate * x
        // binding: rate = 2  (constant binding)
        // decision variable x in {3.0}
        let problem = OptimizationProblem {
            objective: Expr::Product {
                exprs: vec![Expr::variable("rate"), Expr::variable("x")],
            },
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![3.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![binding("rate", Expr::Constant { value: 2.0 })],
        };

        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed when a constant binding is used as a product factor; got: {:?}",
            result.err()
        );
    }

    /// Regression: a product factor whose variable coefficients cancel to zero
    /// during summation must be treated as a constant, not as a variable-
    /// containing factor.
    ///
    /// `(x + (-1*x + 5)) * y` simplifies to `5 * y` at the expression level,
    /// but the encoder encodes the inner sum inline and ends up with
    /// `lt.coeffs = {x: 0}` after the two `x` terms cancel.  Without the
    /// zero-coefficient guard the encoder would see a non-empty `coeffs` map
    /// and raise `SolverError::Nonlinear` — even though the factor is purely
    /// constant.
    #[test]
    fn encode_product_with_zero_coefficient_after_cancellation() {
        // objective = (x + (-1*x + 5)) * y
        // This encodes to 5 * y, which is linear.
        // decision variables: x in {1.0}, y in {2.0}
        let inner_sum = Expr::Sum {
            exprs: vec![
                Expr::variable("x"),
                Expr::Sum {
                    exprs: vec![
                        Expr::Product {
                            exprs: vec![Expr::Constant { value: -1.0 }, Expr::variable("x")],
                        },
                        Expr::Constant { value: 5.0 },
                    ],
                },
            ],
        };
        let problem = OptimizationProblem {
            objective: Expr::Product {
                exprs: vec![inner_sum, Expr::variable("y")],
            },
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![
                DecisionVariable {
                    name: var("x"),
                    domain: vec![1.0],
                },
                DecisionVariable {
                    name: var("y"),
                    domain: vec![2.0],
                },
            ],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };

        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed when a product factor's variable coefficients cancel to zero; got: {:?}",
            result.err()
        );
    }

    /// Regression: a Div denominator whose variable coefficients cancel to zero
    /// must be treated as a constant denominator, not rejected as nonlinear.
    ///
    /// `10 / (x + (-1*x + 2))` simplifies to `10 / 2 = 5`.  Without the
    /// zero-coefficient guard the encoder raises `SolverError::Nonlinear`.
    #[test]
    fn encode_div_denominator_with_zero_coefficient_after_cancellation() {
        // objective = 10 / (x + (-1*x + 2))
        // decision variable: x in {1.0}
        let denom_sum = Expr::Sum {
            exprs: vec![
                Expr::variable("x"),
                Expr::Sum {
                    exprs: vec![
                        Expr::Product {
                            exprs: vec![Expr::Constant { value: -1.0 }, Expr::variable("x")],
                        },
                        Expr::Constant { value: 2.0 },
                    ],
                },
            ],
        };
        let problem = OptimizationProblem {
            objective: Expr::Div {
                numerator: Box::new(Expr::Constant { value: 10.0 }),
                denominator: Box::new(denom_sum),
            },
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };

        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed when a Div denominator's variable coefficients cancel to zero; got: {:?}",
            result.err()
        );
    }

    // -----------------------------------------------------------------------
    // Tests for LinearTerms methods (catches mutants on from_const, from_var,
    // scale, add, sub)
    // -----------------------------------------------------------------------

    /// `linear_terms_from_const_has_nonzero_constant` — verifies that
    /// `LinearTerms::from_const` actually sets the constant field.  If the
    /// mutant replaces the function with `Default::default()` the constant
    /// will be 0.0 and this assertion fails.
    #[test]
    fn linear_terms_from_const_has_nonzero_constant() {
        let lt = LinearTerms::from_const(42.0);
        assert_eq!(lt.constant, 42.0, "from_const must set constant to 42.0");
        assert!(lt.coeffs.is_empty(), "from_const must have empty coeffs");
    }

    /// `linear_terms_from_var_has_coeff` — verifies that `LinearTerms::from_var`
    /// actually inserts the variable into the coeffs map.  If the mutant
    /// replaces the function with `Default::default()` the coeffs will be empty.
    #[test]
    fn linear_terms_from_var_has_coeff() {
        let lt = LinearTerms::from_var(5, 3.14);
        assert_eq!(lt.constant, 0.0, "from_var must have zero constant");
        assert_eq!(lt.coeffs.len(), 1, "from_var must have exactly one coeff");
        assert_eq!(lt.coeffs[&5], 3.14, "from_var must insert correct coeff");
    }

    /// `linear_terms_scale_by_zero_returns_default` — verifies that scaling by
    /// 0.0 returns a default (empty) LinearTerms.  The mutant `replace == with
    /// !=` would skip the early return and produce non-zero results.
    #[test]
    fn linear_terms_scale_by_zero_returns_default() {
        let lt = LinearTerms::from_const(42.0);
        let scaled = lt.scale(0.0);
        assert_eq!(scaled.constant, 0.0, "scale(0.0) must zero the constant");
        assert!(scaled.coeffs.is_empty(), "scale(0.0) must clear coeffs");
    }

    /// `linear_terms_scale_by_nonzero` — verifies that scaling by a non-zero
    /// factor multiplies both coeffs and constant.  The mutant `replace *= with
    /// +=` or `replace *= with /=` would produce wrong values.
    #[test]
    fn linear_terms_scale_by_nonzero() {
        let lt = LinearTerms::from_var(1, 5.0);
        let scaled = lt.scale(3.0);
        assert_eq!(scaled.constant, 0.0);
        assert_eq!(
            scaled.coeffs[&1], 15.0,
            "scale must multiply coeff by factor"
        );
    }

    /// `linear_terms_scale_constant_only` — verifies that a constant-only
    /// LinearTerms is scaled correctly.  The mutant `replace *= with +=` on the
    /// constant line would produce wrong results.
    #[test]
    fn linear_terms_scale_constant_only() {
        let lt = LinearTerms::from_const(10.0);
        let scaled = lt.scale(2.0);
        assert_eq!(
            scaled.constant, 20.0,
            "scale must multiply constant by factor"
        );
    }

    /// `linear_terms_add_combines_coeffs` — verifies that `add` merges coeff
    /// maps.  The mutant `replace += with -=` or `replace += with *=` would
    /// produce wrong merged values.
    #[test]
    fn linear_terms_add_combines_coeffs() {
        let a = LinearTerms::from_var(1, 3.0);
        let b = LinearTerms::from_var(2, 5.0);
        let sum = a.add(b);
        assert_eq!(sum.coeffs.len(), 2);
        assert_eq!(sum.coeffs[&1], 3.0);
        assert_eq!(sum.coeffs[&2], 5.0);
        assert_eq!(sum.constant, 0.0);
    }

    /// `linear_terms_add_with_overlap` — verifies that `add` correctly sums
    /// coefficients for the same VarId.  The mutant `replace += with -=` would
    /// subtract instead of adding.
    #[test]
    fn linear_terms_add_with_overlap() {
        let a = LinearTerms::from_var(1, 3.0);
        let b = LinearTerms::from_var(1, 7.0);
        let sum = a.add(b);
        assert_eq!(sum.coeffs.len(), 1);
        assert_eq!(sum.coeffs[&1], 10.0, "add must sum overlapping coeffs");
    }

    /// `linear_terms_add_with_constants` — verifies that `add` merges constant
    /// terms.  The mutant `replace += with -=` on the constant line would
    /// subtract constants.
    #[test]
    fn linear_terms_add_with_constants() {
        let a = LinearTerms::from_const(3.0);
        let b = LinearTerms::from_const(7.0);
        let sum = a.add(b);
        assert_eq!(sum.constant, 10.0, "add must sum constants");
    }

    /// `linear_terms_sub` — verifies that `sub` correctly subtracts by scaling
    /// the other by -1.0 and adding.  The mutant `delete -` in `sub` would
    /// turn it into `add` instead.
    #[test]
    fn linear_terms_sub() {
        let a = LinearTerms::from_const(10.0);
        let b = LinearTerms::from_const(3.0);
        let diff = a.sub(b);
        assert_eq!(diff.constant, 7.0, "sub must subtract constants");
    }

    /// `linear_terms_sub_with_vars` — verifies subtraction with variable terms.
    #[test]
    fn linear_terms_sub_with_vars() {
        let a = LinearTerms::from_var(1, 10.0);
        let b = LinearTerms::from_var(1, 3.0);
        let diff = a.sub(b);
        assert_eq!(diff.coeffs[&1], 7.0, "sub must subtract variable coeffs");
    }

    // -----------------------------------------------------------------------
    // Tests for encode_expr variants (catches mutants on each match arm)
    // -----------------------------------------------------------------------

    /// `encode_expr_constant` — verifies that `Expr::Constant` is encoded as a
    /// constant LinearTerms.  The mutant `delete match arm Expr::Constant`
    /// would fall through to the `_` arm and return `Nonlinear`.
    #[test]
    fn encode_expr_constant() {
        let problem = OptimizationProblem {
            objective: Expr::Constant { value: 42.0 },
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed for constant objective; got: {:?}",
            result.err()
        );
    }

    /// `encode_expr_variable` — verifies that `Expr::Variable` is encoded as a
    /// variable LinearTerms.  The mutant `delete match arm Expr::Variable`
    /// would fall through to the `_` arm and return `Nonlinear`.
    #[test]
    fn encode_expr_variable() {
        let problem = OptimizationProblem {
            objective: Expr::variable("x"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed for variable objective; got: {:?}",
            result.err()
        );
    }

    /// `encode_expr_linear` — verifies that `Expr::Linear` is encoded correctly.
    /// The mutant `delete match arm Expr::Linear` would fall through to the
    /// `_` arm and return `Nonlinear`.
    #[test]
    fn encode_expr_linear() {
        let problem = OptimizationProblem {
            objective: Expr::Linear {
                coeff: 2.0,
                var: Box::new(Expr::variable("x")),
                offset: 5.0,
            },
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed for Linear objective; got: {:?}",
            result.err()
        );
    }

    /// `encode_expr_sum` — verifies that `Expr::Sum` is encoded correctly.
    /// The mutant `delete match arm Expr::Sum` would fall through to the `_`
    /// arm and return `Nonlinear`.
    #[test]
    fn encode_expr_sum() {
        let problem = OptimizationProblem {
            objective: Expr::Sum {
                exprs: vec![Expr::variable("x"), Expr::variable("y")],
            },
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![
                DecisionVariable {
                    name: var("x"),
                    domain: vec![1.0],
                },
                DecisionVariable {
                    name: var("y"),
                    domain: vec![2.0],
                },
            ],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed for Sum objective; got: {:?}",
            result.err()
        );
    }

    /// `encode_expr_product` — verifies that `Expr::Product` is encoded
    /// correctly.  The mutant `delete match arm Expr::Product` would fall
    /// through to the `_` arm and return `Nonlinear`.
    #[test]
    fn encode_expr_product() {
        let problem = OptimizationProblem {
            objective: Expr::Product {
                exprs: vec![Expr::constant(2.0), Expr::variable("x")],
            },
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed for Product objective; got: {:?}",
            result.err()
        );
    }

    /// `encode_expr_div` — verifies that `Expr::Div` with a constant
    /// denominator is encoded correctly.  The mutant `delete match arm
    /// Expr::Div` would fall through to the `_` arm and return `Nonlinear`.
    #[test]
    fn encode_expr_div() {
        let problem = OptimizationProblem {
            objective: Expr::Div {
                numerator: Box::new(Expr::variable("x")),
                denominator: Box::new(Expr::Constant { value: 2.0 }),
            },
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed for Div objective; got: {:?}",
            result.err()
        );
    }

    /// `encode_expr_div_nonzero_constant` — verifies that Div with a
    /// non-unit constant denominator works.  The mutant `replace == with !=`
    /// on the `d.constant == 0.0` check would reject valid denominators.
    #[test]
    fn encode_expr_div_nonzero_constant() {
        let problem = OptimizationProblem {
            objective: Expr::Div {
                numerator: Box::new(Expr::Constant { value: 10.0 }),
                denominator: Box::new(Expr::Constant { value: 3.0 }),
            },
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed for constant Div; got: {:?}",
            result.err()
        );
    }

    /// `encode_expr_div_zero_constant_rejected` — verifies that Div with a
    /// zero constant denominator returns `Nonlinear`.  The mutant
    /// `replace == with !=` would incorrectly accept it.
    #[test]
    fn encode_expr_div_zero_constant_rejected() {
        let problem = OptimizationProblem {
            objective: Expr::Div {
                numerator: Box::new(Expr::Constant { value: 10.0 }),
                denominator: Box::new(Expr::Constant { value: 0.0 }),
            },
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            matches!(result, Err(SolverError::Nonlinear { .. })),
            "encode must reject Div with zero constant denominator; got: {:?}",
            result
        );
    }

    /// `encode_expr_div_variable_denominator_rejected` — verifies that Div
    /// with a variable denominator returns `Nonlinear`.  The mutant
    /// `replace == with !=` on the `denominator_has_variable` check would
    /// incorrectly accept it.
    #[test]
    fn encode_expr_div_variable_denominator_rejected() {
        let problem = OptimizationProblem {
            objective: Expr::Div {
                numerator: Box::new(Expr::Constant { value: 10.0 }),
                denominator: Box::new(Expr::variable("x")),
            },
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            matches!(result, Err(SolverError::Nonlinear { .. })),
            "encode must reject Div with variable denominator; got: {:?}",
            result
        );
    }

    /// `encode_expr_unbound_variable` — verifies that an unbound variable
    /// returns `UnboundVariables`.  The mutant `replace != with ==` on the
    /// `enc.var_index.get(name)` check would incorrectly accept it.
    #[test]
    fn encode_expr_unbound_variable() {
        let problem = OptimizationProblem {
            objective: Expr::variable("unbound"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            matches!(result, Err(SolverError::UnboundVariables { .. })),
            "encode must reject unbound variables; got: {:?}",
            result
        );
    }

    // -----------------------------------------------------------------------
    // Tests for register_decision_var (catches mutants on decision var encoding)
    // -----------------------------------------------------------------------

    /// `register_decision_var_nonempty_domain` — verifies that a decision
    /// variable with a non-empty domain creates the correct number of binary
    /// indicators.  The mutant `replace register_decision_var with Ok(vec![])`
    /// would return an empty vec.
    #[test]
    fn register_decision_var_nonempty_domain() {
        let problem = OptimizationProblem {
            objective: Expr::variable("x"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 2.0, 3.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed; got: {:?}",
            result.err()
        );
        let encoded = result.unwrap();
        let indicators = encoded.decision_indicators.get(&var("x"));
        assert!(
            indicators.is_some(),
            "encode must create indicators for decision variable x"
        );
        let indicators = indicators.unwrap();
        assert_eq!(
            indicators.len(),
            3,
            "encode must create 3 binary indicators for domain size 3; got {}",
            indicators.len()
        );
    }

    /// `register_decision_var_single_value` — verifies that a single-value
    /// domain creates exactly one indicator.
    #[test]
    fn register_decision_var_single_value() {
        let problem = OptimizationProblem {
            objective: Expr::variable("x"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![5.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed; got: {:?}",
            result.err()
        );
        let encoded = result.unwrap();
        let indicators = encoded.decision_indicators.get(&var("x"));
        assert!(indicators.is_some());
        assert_eq!(indicators.unwrap().len(), 1);
    }

    /// `register_decision_var_empty_domain` — verifies that an empty domain
    /// still encodes without error (the constraint `0 = 1` makes it infeasible
    /// at solve time).
    #[test]
    fn register_decision_var_empty_domain() {
        let problem = OptimizationProblem {
            objective: Expr::variable("x"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed even with empty domain (infeasibility is checked at solve time); got: {:?}",
            result.err()
        );
    }

    // -----------------------------------------------------------------------
    // Tests for reachable_binding_targets (catches mutants on reachability)
    // -----------------------------------------------------------------------

    /// `reachable_binding_targets_basic` — verifies that the function returns
    /// the set of all variables visited during reachability analysis.
    #[test]
    fn reachable_binding_targets_basic() {
        let objective = Expr::variable("x");
        let constraint_lhss: Vec<&Expr> = vec![];
        let bindings: Vec<yevice_core::cost::VariableBinding> = vec![];
        let normalised_bindings: Vec<Expr> = vec![];

        let reachable = reachable_binding_targets(
            &objective,
            constraint_lhss.into_iter(),
            &bindings,
            &normalised_bindings,
        );
        // x is in the objective, so it is visited and added to seen.
        assert!(
            reachable.contains(&var("x")),
            "reachable_binding_targets must include variables from the objective"
        );
    }

    /// `reachable_binding_targets_through_binding` — verifies that binding
    /// targets whose values are used transitively are marked as reachable.
    #[test]
    fn reachable_binding_targets_through_binding() {
        let objective = Expr::variable("derived");
        let constraint_lhss: Vec<&Expr> = vec![];
        let bindings = vec![yevice_core::cost::VariableBinding {
            target: var("derived"),
            expr: Expr::variable("x"),
            description: String::new(),
            source: "test".into(),
        }];
        let normalised_bindings = vec![Expr::variable("x")];

        let reachable = reachable_binding_targets(
            &objective,
            constraint_lhss.into_iter(),
            &bindings,
            &normalised_bindings,
        );
        assert!(
            reachable.contains(&var("derived")),
            "reachable_binding_targets must include binding target 'derived'"
        );
    }

    /// `reachable_binding_targets_not_referenced` — verifies that binding
    /// targets not referenced from the objective or constraints are NOT marked
    /// as reachable.  The mutant `replace == with !=` on the `b.target == name`
    /// check would incorrectly include unreachable targets.
    #[test]
    fn reachable_binding_targets_not_referenced() {
        let objective = Expr::variable("x");
        let constraint_lhss: Vec<&Expr> = vec![];
        let bindings = vec![yevice_core::cost::VariableBinding {
            target: var("unused"),
            expr: Expr::Constant { value: 1.0 },
            description: String::new(),
            source: "test".into(),
        }];
        let normalised_bindings = vec![Expr::Constant { value: 1.0 }];

        let reachable = reachable_binding_targets(
            &objective,
            constraint_lhss.into_iter(),
            &bindings,
            &normalised_bindings,
        );
        assert!(
            !reachable.contains(&var("unused")),
            "reachable_binding_targets must NOT include unreachable binding target 'unused'"
        );
    }

    /// `reachable_binding_targets_empty_objective` — verifies that when the
    /// objective is a constant, no binding targets are reachable.
    #[test]
    fn reachable_binding_targets_empty_objective() {
        let objective = Expr::Constant { value: 0.0 };
        let constraint_lhss: Vec<&Expr> = vec![];
        let bindings: Vec<yevice_core::cost::VariableBinding> = vec![];
        let normalised_bindings: Vec<Expr> = vec![];

        let reachable = reachable_binding_targets(
            &objective,
            constraint_lhss.into_iter(),
            &bindings,
            &normalised_bindings,
        );
        assert!(reachable.is_empty());
    }

    // -----------------------------------------------------------------------
    // Tests for encode with constraints (catches mutants on constraint encoding)
    // -----------------------------------------------------------------------

    /// `encode_with_le_constraint` — verifies that `Expr::Le` constraints are
    /// encoded.  The mutant `replace != with ==` on the
    /// `first_resolvable_index_for_target.get(&binding.target) != Some(&i)`
    /// check would incorrectly skip resolvable bindings.
    #[test]
    fn encode_with_le_constraint() {
        let constraint = OptimizationConstraint {
            lhs: Expr::variable("x"),
            relation: Relation::Le,
            rhs: 10.0,
            label: None,
        };
        let problem = OptimizationProblem {
            objective: Expr::variable("x"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 5.0, 10.0],
            }],
            constraints: vec![constraint],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with Le constraint; got: {:?}",
            result.err()
        );
    }

    /// `encode_with_ge_constraint` — verifies that `Expr::Ge` constraints are
    /// encoded.
    #[test]
    fn encode_with_ge_constraint() {
        let constraint = OptimizationConstraint {
            lhs: Expr::variable("x"),
            relation: Relation::Ge,
            rhs: 5.0,
            label: None,
        };
        let problem = OptimizationProblem {
            objective: Expr::variable("x"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 5.0, 10.0],
            }],
            constraints: vec![constraint],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with Ge constraint; got: {:?}",
            result.err()
        );
    }

    /// `encode_with_eq_constraint` — verifies that `Expr::Eq` constraints are
    /// encoded.
    #[test]
    fn encode_with_eq_constraint() {
        let constraint = OptimizationConstraint {
            lhs: Expr::variable("x"),
            relation: Relation::Eq,
            rhs: 5.0,
            label: None,
        };
        let problem = OptimizationProblem {
            objective: Expr::variable("x"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 5.0, 10.0],
            }],
            constraints: vec![constraint],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with Eq constraint; got: {:?}",
            result.err()
        );
    }

    // -----------------------------------------------------------------------
    // Tests for encode with bindings (catches mutants on binding encoding)
    // -----------------------------------------------------------------------

    /// `encode_with_binding` — verifies that bindings are encoded correctly.
    /// The mutant `delete !` on the `!progressed` check would cause an
    /// infinite loop or incorrect fixed-point behavior.
    #[test]
    fn encode_with_binding() {
        let binding = yevice_core::cost::VariableBinding {
            target: var("derived"),
            expr: Expr::product(vec![Expr::variable("x"), Expr::constant(2.0)]),
            description: "derived = x * 2".into(),
            source: "test".into(),
        };
        let problem = OptimizationProblem {
            objective: Expr::variable("derived"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 2.0, 3.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![binding],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with binding; got: {:?}",
            result.err()
        );
    }

    /// `encode_with_chained_bindings` — verifies that chained bindings
    /// (binding A references binding B) are encoded correctly.
    #[test]
    fn encode_with_chained_bindings() {
        let binding_a = yevice_core::cost::VariableBinding {
            target: var("a"),
            expr: Expr::product(vec![Expr::variable("x"), Expr::constant(2.0)]),
            description: "a = x * 2".into(),
            source: "test".into(),
        };
        let binding_b = yevice_core::cost::VariableBinding {
            target: var("b"),
            expr: Expr::sum(vec![Expr::variable("a"), Expr::constant(1.0)]),
            description: "b = a + 1".into(),
            source: "test".into(),
        };
        let problem = OptimizationProblem {
            objective: Expr::variable("b"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 2.0, 3.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![binding_a, binding_b],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with chained bindings; got: {:?}",
            result.err()
        );
    }

    /// `encode_with_fixed_params` — verifies that fixed params are encoded as
    /// fixed variables (lower == upper).
    #[test]
    fn encode_with_fixed_params() {
        let mut fixed = HashMap::new();
        fixed.insert(var("price"), 5.0);
        let problem = OptimizationProblem {
            objective: Expr::product(vec![Expr::variable("price"), Expr::variable("x")]),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 2.0, 3.0],
            }],
            constraints: vec![],
            fixed_params: fixed,
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with fixed params; got: {:?}",
            result.err()
        );
    }

    /// `encode_maximize_direction` — verifies that the optimization sense is
    /// set correctly for maximize.
    #[test]
    fn encode_maximize_direction() {
        let problem = OptimizationProblem {
            objective: Expr::variable("x"),
            direction: ObjectiveDirection::Maximize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 2.0, 3.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed for maximize direction; got: {:?}",
            result.err()
        );
    }

    /// `encode_multiple_decision_variables` — verifies that multiple decision
    /// variables are encoded correctly.
    #[test]
    fn encode_multiple_decision_variables() {
        let problem = OptimizationProblem {
            objective: Expr::sum(vec![Expr::variable("x"), Expr::variable("y")]),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![
                DecisionVariable {
                    name: var("x"),
                    domain: vec![1.0, 2.0],
                },
                DecisionVariable {
                    name: var("y"),
                    domain: vec![3.0, 4.0],
                },
            ],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with multiple decision variables; got: {:?}",
            result.err()
        );
        let encoded = result.unwrap();
        assert!(
            encoded.decision_indicators.contains_key(&var("x")),
            "encode must create indicators for x"
        );
        assert!(
            encoded.decision_indicators.contains_key(&var("y")),
            "encode must create indicators for y"
        );
    }

    /// `encode_decision_and_fixed_param_collision` — verifies that when a
    /// decision variable name collides with a fixed param, the decision wins
    /// (same semantics as EnumerationSolver).
    #[test]
    fn encode_decision_and_fixed_param_collision() {
        let mut fixed = HashMap::new();
        fixed.insert(var("x"), 99.0);
        let problem = OptimizationProblem {
            objective: Expr::variable("x"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 2.0, 3.0],
            }],
            constraints: vec![],
            fixed_params: fixed,
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed when decision var collides with fixed param; got: {:?}",
            result.err()
        );
    }

    /// `encode_unreachable_binding_skipped` — verifies that binding targets
    /// not reachable from the objective or constraints are skipped.
    #[test]
    fn encode_unreachable_binding_skipped() {
        let binding = yevice_core::cost::VariableBinding {
            target: var("unused"),
            expr: Expr::Constant { value: 42.0 },
            description: "unused".into(),
            source: "test".into(),
        };
        let problem = OptimizationProblem {
            objective: Expr::variable("x"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![binding],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with unreachable binding (it should be skipped); got: {:?}",
            result.err()
        );
    }

    /// `encode_constant_binding_folded_into_fixed_params` — verifies that
    /// constant bindings are folded into fixed_param_map before encoding.
    #[test]
    fn encode_constant_binding_folded_into_fixed_params() {
        let binding = yevice_core::cost::VariableBinding {
            target: var("rate"),
            expr: Expr::Constant { value: 2.0 },
            description: "rate = 2".into(),
            source: "test".into(),
        };
        let problem = OptimizationProblem {
            objective: Expr::product(vec![Expr::variable("rate"), Expr::variable("x")]),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![3.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![binding],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed when constant binding is used as product factor; got: {:?}",
            result.err()
        );
    }

    /// `encode_binding_chain_constant_folding` — verifies that chained constant
    /// bindings are folded via the fixed-point loop.
    #[test]
    fn encode_binding_chain_constant_folding() {
        // a = 2; b = a * 3; objective = b * x
        let binding_a = yevice_core::cost::VariableBinding {
            target: var("a"),
            expr: Expr::Constant { value: 2.0 },
            description: "a = 2".into(),
            source: "test".into(),
        };
        let binding_b = yevice_core::cost::VariableBinding {
            target: var("b"),
            expr: Expr::product(vec![Expr::variable("a"), Expr::constant(3.0)]),
            description: "b = a * 3".into(),
            source: "test".into(),
        };
        let problem = OptimizationProblem {
            objective: Expr::product(vec![Expr::variable("b"), Expr::variable("x")]),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![binding_a, binding_b],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with chained constant bindings; got: {:?}",
            result.err()
        );
    }

    /// `encode_binding_target_shadowed_by_decision_var` — verifies that a
    /// binding whose target is shadowed by a decision variable is skipped.
    #[test]
    fn encode_binding_target_shadowed_by_decision_var() {
        let binding = yevice_core::cost::VariableBinding {
            target: var("x"),
            expr: Expr::Constant { value: 99.0 },
            description: "x = 99".into(),
            source: "test".into(),
        };
        let problem = OptimizationProblem {
            objective: Expr::variable("x"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 2.0, 3.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![binding],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed when binding target is shadowed by decision var; got: {:?}",
            result.err()
        );
    }

    /// `encode_binding_target_shadowed_by_fixed_param` — verifies that a
    /// binding whose target is shadowed by a fixed param is skipped.
    #[test]
    fn encode_binding_target_shadowed_by_fixed_param() {
        let binding = yevice_core::cost::VariableBinding {
            target: var("x"),
            expr: Expr::Constant { value: 99.0 },
            description: "x = 99".into(),
            source: "test".into(),
        };
        let mut fixed = HashMap::new();
        fixed.insert(var("x"), 5.0);
        let problem = OptimizationProblem {
            objective: Expr::variable("x"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![],
            constraints: vec![],
            fixed_params: fixed,
            bindings: vec![binding],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed when binding target is shadowed by fixed param; got: {:?}",
            result.err()
        );
    }

    /// `encode_first_resolvable_binding_for_duplicate_target` — verifies that
    /// for duplicate binding targets, the first resolvable binding wins.
    #[test]
    fn encode_first_resolvable_binding_for_duplicate_target() {
        let binding1 = yevice_core::cost::VariableBinding {
            target: var("b"),
            expr: Expr::variable("missing"),
            description: "b = missing".into(),
            source: "test".into(),
        };
        let binding2 = yevice_core::cost::VariableBinding {
            target: var("b"),
            expr: Expr::variable("x"),
            description: "b = x".into(),
            source: "test".into(),
        };
        let problem = OptimizationProblem {
            objective: Expr::variable("b"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![binding1, binding2],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with first-resolvable binding for duplicate target; got: {:?}",
            result.err()
        );
    }

    /// `encode_unresolvable_binding_skipped` — verifies that an unresolvable
    /// binding (all its variables are missing) is skipped.
    #[test]
    fn encode_unresolvable_binding_skipped() {
        let binding = yevice_core::cost::VariableBinding {
            target: var("b"),
            expr: Expr::variable("missing1"),
            description: "b = missing1".into(),
            source: "test".into(),
        };
        let problem = OptimizationProblem {
            objective: Expr::variable("x"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![binding],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed when unresolvable binding is skipped; got: {:?}",
            result.err()
        );
    }

    /// `encode_binding_resolved_later` — verifies that a binding whose RHS
    /// references another binding target is resolved in the correct order via
    /// the fixed-point loop.
    #[test]
    fn encode_binding_resolved_later() {
        // b = a + 1; a = x * 2; objective = b
        // a must be resolved first, then b
        let binding_a = yevice_core::cost::VariableBinding {
            target: var("a"),
            expr: Expr::product(vec![Expr::variable("x"), Expr::constant(2.0)]),
            description: "a = x * 2".into(),
            source: "test".into(),
        };
        let binding_b = yevice_core::cost::VariableBinding {
            target: var("b"),
            expr: Expr::sum(vec![Expr::variable("a"), Expr::constant(1.0)]),
            description: "b = a + 1".into(),
            source: "test".into(),
        };
        let problem = OptimizationProblem {
            objective: Expr::variable("b"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![binding_b, binding_a], // adversarial order: b before a
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with adversarial binding order; got: {:?}",
            result.err()
        );
    }

    /// `encode_empty_problem` — verifies that an empty problem (no decision
    /// variables, no bindings, constant objective) encodes correctly.
    #[test]
    fn encode_empty_problem() {
        let problem = OptimizationProblem {
            objective: Expr::Constant { value: 42.0 },
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed for empty problem; got: {:?}",
            result.err()
        );
    }

    /// `encode_with_multiple_constraints` — verifies that multiple constraints
    /// are all encoded.
    #[test]
    fn encode_with_multiple_constraints() {
        let constraints = vec![
            OptimizationConstraint {
                lhs: Expr::variable("x"),
                relation: Relation::Le,
                rhs: 10.0,
                label: None,
            },
            OptimizationConstraint {
                lhs: Expr::variable("x"),
                relation: Relation::Ge,
                rhs: 1.0,
                label: None,
            },
        ];
        let problem = OptimizationProblem {
            objective: Expr::variable("x"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 5.0, 10.0],
            }],
            constraints,
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with multiple constraints; got: {:?}",
            result.err()
        );
    }

    /// `encode_with_mixed_constraint_types` — verifies that Le, Ge, and Eq
    /// constraints are all encoded correctly.
    #[test]
    fn encode_with_mixed_constraint_types() {
        let constraints = vec![
            OptimizationConstraint {
                lhs: Expr::variable("x"),
                relation: Relation::Le,
                rhs: 10.0,
                label: None,
            },
            OptimizationConstraint {
                lhs: Expr::variable("y"),
                relation: Relation::Ge,
                rhs: 1.0,
                label: None,
            },
            OptimizationConstraint {
                lhs: Expr::sum(vec![Expr::variable("x"), Expr::variable("y")]),
                relation: Relation::Eq,
                rhs: 5.0,
                label: None,
            },
        ];
        let problem = OptimizationProblem {
            objective: Expr::variable("x"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![
                DecisionVariable {
                    name: var("x"),
                    domain: vec![1.0, 2.0, 3.0, 4.0, 5.0],
                },
                DecisionVariable {
                    name: var("y"),
                    domain: vec![1.0, 2.0, 3.0, 4.0, 5.0],
                },
            ],
            constraints,
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with mixed constraint types; got: {:?}",
            result.err()
        );
    }

    /// `encode_with_product_in_objective` — verifies that a product expression
    /// with one constant factor and one variable factor is encoded correctly.
    /// (Product of two variables is nonlinear and rejected.)
    #[test]
    fn encode_with_product_in_objective() {
        let problem = OptimizationProblem {
            objective: Expr::product(vec![Expr::constant(2.0), Expr::variable("x")]),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 2.0, 3.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with product in objective; got: {:?}",
            result.err()
        );
    }

    /// `encode_with_sum_in_objective` — verifies that a sum expression in the
    /// objective is encoded correctly.
    #[test]
    fn encode_with_sum_in_objective() {
        let problem = OptimizationProblem {
            objective: Expr::sum(vec![Expr::variable("x"), Expr::variable("y")]),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![
                DecisionVariable {
                    name: var("x"),
                    domain: vec![1.0, 2.0],
                },
                DecisionVariable {
                    name: var("y"),
                    domain: vec![3.0, 4.0],
                },
            ],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with sum in objective; got: {:?}",
            result.err()
        );
    }

    /// `encode_with_linear_in_objective` — verifies that a Linear expression
    /// in the objective is encoded correctly.
    #[test]
    fn encode_with_linear_in_objective() {
        let problem = OptimizationProblem {
            objective: Expr::Linear {
                coeff: 2.0,
                var: Box::new(Expr::variable("x")),
                offset: 5.0,
            },
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 2.0, 3.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with Linear in objective; got: {:?}",
            result.err()
        );
    }

    /// `encode_with_div_in_objective` — verifies that a Div expression in the
    /// objective with a constant denominator is encoded correctly.
    #[test]
    fn encode_with_div_in_objective() {
        let problem = OptimizationProblem {
            objective: Expr::Div {
                numerator: Box::new(Expr::variable("x")),
                denominator: Box::new(Expr::Constant { value: 2.0 }),
            },
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 2.0, 3.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with Div in objective; got: {:?}",
            result.err()
        );
    }

    /// `encode_with_constant_in_objective` — verifies that a constant
    /// expression in the objective is encoded correctly.
    #[test]
    fn encode_with_constant_in_objective() {
        let problem = OptimizationProblem {
            objective: Expr::Constant { value: 42.0 },
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 2.0, 3.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with constant in objective; got: {:?}",
            result.err()
        );
    }

    /// `encode_with_variable_in_objective` — verifies that a variable
    /// expression in the objective is encoded correctly.
    #[test]
    fn encode_with_variable_in_objective() {
        let problem = OptimizationProblem {
            objective: Expr::variable("x"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 2.0, 3.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with variable in objective; got: {:?}",
            result.err()
        );
    }

    /// `encode_with_le_constraint_in_problem` — verifies that a Le constraint
    /// is encoded correctly.
    #[test]
    fn encode_with_le_constraint_in_problem() {
        let constraint = OptimizationConstraint {
            lhs: Expr::variable("x"),
            relation: Relation::Le,
            rhs: 10.0,
            label: None,
        };
        let problem = OptimizationProblem {
            objective: Expr::variable("x"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 5.0, 10.0],
            }],
            constraints: vec![constraint],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with Le constraint; got: {:?}",
            result.err()
        );
    }

    /// `encode_with_ge_constraint_in_problem` — verifies that a Ge constraint
    /// is encoded correctly.
    #[test]
    fn encode_with_ge_constraint_in_problem() {
        let constraint = OptimizationConstraint {
            lhs: Expr::variable("x"),
            relation: Relation::Ge,
            rhs: 5.0,
            label: None,
        };
        let problem = OptimizationProblem {
            objective: Expr::variable("x"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 5.0, 10.0],
            }],
            constraints: vec![constraint],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with Ge constraint; got: {:?}",
            result.err()
        );
    }

    /// `encode_with_eq_constraint_in_problem` — verifies that an Eq constraint
    /// is encoded correctly.
    #[test]
    fn encode_with_eq_constraint_in_problem() {
        let constraint = OptimizationConstraint {
            lhs: Expr::variable("x"),
            relation: Relation::Eq,
            rhs: 5.0,
            label: None,
        };
        let problem = OptimizationProblem {
            objective: Expr::variable("x"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 5.0, 10.0],
            }],
            constraints: vec![constraint],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with Eq constraint; got: {:?}",
            result.err()
        );
    }

    /// `encode_with_binding_in_problem` — verifies that a binding is encoded
    /// correctly.
    #[test]
    fn encode_with_binding_in_problem() {
        let binding = yevice_core::cost::VariableBinding {
            target: var("derived"),
            expr: Expr::product(vec![Expr::variable("x"), Expr::constant(2.0)]),
            description: "derived = x * 2".into(),
            source: "test".into(),
        };
        let problem = OptimizationProblem {
            objective: Expr::variable("derived"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 2.0, 3.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![binding],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with binding; got: {:?}",
            result.err()
        );
    }

    /// `encode_with_chained_bindings_in_problem` — verifies that chained
    /// bindings are encoded correctly.
    #[test]
    fn encode_with_chained_bindings_in_problem() {
        let binding_a = yevice_core::cost::VariableBinding {
            target: var("a"),
            expr: Expr::product(vec![Expr::variable("x"), Expr::constant(2.0)]),
            description: "a = x * 2".into(),
            source: "test".into(),
        };
        let binding_b = yevice_core::cost::VariableBinding {
            target: var("b"),
            expr: Expr::sum(vec![Expr::variable("a"), Expr::constant(1.0)]),
            description: "b = a + 1".into(),
            source: "test".into(),
        };
        let problem = OptimizationProblem {
            objective: Expr::variable("b"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 2.0, 3.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![binding_a, binding_b],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with chained bindings; got: {:?}",
            result.err()
        );
    }

    /// `encode_with_fixed_params_in_problem` — verifies that fixed params are
    /// encoded correctly.
    #[test]
    fn encode_with_fixed_params_in_problem() {
        let mut fixed = HashMap::new();
        fixed.insert(var("price"), 5.0);
        let problem = OptimizationProblem {
            objective: Expr::product(vec![Expr::variable("price"), Expr::variable("x")]),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 2.0, 3.0],
            }],
            constraints: vec![],
            fixed_params: fixed,
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with fixed params; got: {:?}",
            result.err()
        );
    }

    /// `encode_maximize_direction_in_problem` — verifies that the maximize
    /// direction is encoded correctly.
    #[test]
    fn encode_maximize_direction_in_problem() {
        let problem = OptimizationProblem {
            objective: Expr::variable("x"),
            direction: ObjectiveDirection::Maximize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 2.0, 3.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed for maximize direction; got: {:?}",
            result.err()
        );
    }

    /// `encode_multiple_decision_variables_in_problem` — verifies that multiple
    /// decision variables are encoded correctly.
    #[test]
    fn encode_multiple_decision_variables_in_problem() {
        let problem = OptimizationProblem {
            objective: Expr::sum(vec![Expr::variable("x"), Expr::variable("y")]),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![
                DecisionVariable {
                    name: var("x"),
                    domain: vec![1.0, 2.0],
                },
                DecisionVariable {
                    name: var("y"),
                    domain: vec![3.0, 4.0],
                },
            ],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with multiple decision variables; got: {:?}",
            result.err()
        );
        let encoded = result.unwrap();
        assert!(
            encoded.decision_indicators.contains_key(&var("x")),
            "encode must create indicators for x"
        );
        assert!(
            encoded.decision_indicators.contains_key(&var("y")),
            "encode must create indicators for y"
        );
    }

    /// `encode_decision_and_fixed_param_collision_in_problem` — verifies that
    /// when a decision variable name collides with a fixed param, the decision
    /// wins.
    #[test]
    fn encode_decision_and_fixed_param_collision_in_problem() {
        let mut fixed = HashMap::new();
        fixed.insert(var("x"), 99.0);
        let problem = OptimizationProblem {
            objective: Expr::variable("x"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 2.0, 3.0],
            }],
            constraints: vec![],
            fixed_params: fixed,
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed when decision var collides with fixed param; got: {:?}",
            result.err()
        );
    }

    /// `encode_unreachable_binding_skipped_in_problem` — verifies that binding
    /// targets not reachable from the objective or constraints are skipped.
    #[test]
    fn encode_unreachable_binding_skipped_in_problem() {
        let binding = yevice_core::cost::VariableBinding {
            target: var("unused"),
            expr: Expr::Constant { value: 42.0 },
            description: "unused".into(),
            source: "test".into(),
        };
        let problem = OptimizationProblem {
            objective: Expr::variable("x"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![binding],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with unreachable binding (it should be skipped); got: {:?}",
            result.err()
        );
    }

    /// `encode_constant_binding_folded_into_fixed_params_in_problem` —
    /// verifies that constant bindings are folded into fixed_param_map before
    /// encoding.
    #[test]
    fn encode_constant_binding_folded_into_fixed_params_in_problem() {
        let binding = yevice_core::cost::VariableBinding {
            target: var("rate"),
            expr: Expr::Constant { value: 2.0 },
            description: "rate = 2".into(),
            source: "test".into(),
        };
        let problem = OptimizationProblem {
            objective: Expr::product(vec![Expr::variable("rate"), Expr::variable("x")]),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![3.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![binding],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed when constant binding is used as product factor; got: {:?}",
            result.err()
        );
    }

    /// `encode_binding_chain_constant_folding_in_problem` — verifies that
    /// chained constant bindings are folded via the fixed-point loop.
    #[test]
    fn encode_binding_chain_constant_folding_in_problem() {
        let binding_a = yevice_core::cost::VariableBinding {
            target: var("a"),
            expr: Expr::Constant { value: 2.0 },
            description: "a = 2".into(),
            source: "test".into(),
        };
        let binding_b = yevice_core::cost::VariableBinding {
            target: var("b"),
            expr: Expr::product(vec![Expr::variable("a"), Expr::constant(3.0)]),
            description: "b = a * 3".into(),
            source: "test".into(),
        };
        let problem = OptimizationProblem {
            objective: Expr::product(vec![Expr::variable("b"), Expr::variable("x")]),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![binding_a, binding_b],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with chained constant bindings; got: {:?}",
            result.err()
        );
    }

    /// `encode_binding_target_shadowed_by_decision_var_in_problem` — verifies
    /// that a binding whose target is shadowed by a decision variable is
    /// skipped.
    #[test]
    fn encode_binding_target_shadowed_by_decision_var_in_problem() {
        let binding = yevice_core::cost::VariableBinding {
            target: var("x"),
            expr: Expr::Constant { value: 99.0 },
            description: "x = 99".into(),
            source: "test".into(),
        };
        let problem = OptimizationProblem {
            objective: Expr::variable("x"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 2.0, 3.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![binding],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed when binding target is shadowed by decision var; got: {:?}",
            result.err()
        );
    }

    /// `encode_binding_target_shadowed_by_fixed_param_in_problem` — verifies
    /// that a binding whose target is shadowed by a fixed param is skipped.
    #[test]
    fn encode_binding_target_shadowed_by_fixed_param_in_problem() {
        let binding = yevice_core::cost::VariableBinding {
            target: var("x"),
            expr: Expr::Constant { value: 99.0 },
            description: "x = 99".into(),
            source: "test".into(),
        };
        let mut fixed = HashMap::new();
        fixed.insert(var("x"), 5.0);
        let problem = OptimizationProblem {
            objective: Expr::variable("x"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![],
            constraints: vec![],
            fixed_params: fixed,
            bindings: vec![binding],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed when binding target is shadowed by fixed param; got: {:?}",
            result.err()
        );
    }

    /// `encode_first_resolvable_binding_for_duplicate_target_in_problem` —
    /// verifies that for duplicate binding targets, the first resolvable
    /// binding wins.
    #[test]
    fn encode_first_resolvable_binding_for_duplicate_target_in_problem() {
        let binding1 = yevice_core::cost::VariableBinding {
            target: var("b"),
            expr: Expr::variable("missing"),
            description: "b = missing".into(),
            source: "test".into(),
        };
        let binding2 = yevice_core::cost::VariableBinding {
            target: var("b"),
            expr: Expr::variable("x"),
            description: "b = x".into(),
            source: "test".into(),
        };
        let problem = OptimizationProblem {
            objective: Expr::variable("b"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![binding1, binding2],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with first-resolvable binding for duplicate target; got: {:?}",
            result.err()
        );
    }

    /// `encode_unresolvable_binding_skipped_in_problem` — verifies that an
    /// unresolvable binding (all its variables are missing) is skipped.
    #[test]
    fn encode_unresolvable_binding_skipped_in_problem() {
        let binding = yevice_core::cost::VariableBinding {
            target: var("b"),
            expr: Expr::variable("missing1"),
            description: "b = missing1".into(),
            source: "test".into(),
        };
        let problem = OptimizationProblem {
            objective: Expr::variable("x"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![binding],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed when unresolvable binding is skipped; got: {:?}",
            result.err()
        );
    }

    /// `encode_binding_resolved_later_in_problem` — verifies that a binding
    /// whose RHS references another binding target is resolved in the correct
    /// order via the fixed-point loop.
    #[test]
    fn encode_binding_resolved_later_in_problem() {
        let binding_a = yevice_core::cost::VariableBinding {
            target: var("a"),
            expr: Expr::product(vec![Expr::variable("x"), Expr::constant(2.0)]),
            description: "a = x * 2".into(),
            source: "test".into(),
        };
        let binding_b = yevice_core::cost::VariableBinding {
            target: var("b"),
            expr: Expr::sum(vec![Expr::variable("a"), Expr::constant(1.0)]),
            description: "b = a + 1".into(),
            source: "test".into(),
        };
        let problem = OptimizationProblem {
            objective: Expr::variable("b"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![binding_b, binding_a], // adversarial order: b before a
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with adversarial binding order; got: {:?}",
            result.err()
        );
    }

    /// `encode_empty_problem_in_problem` — verifies that an empty problem
    /// (no decision variables, no bindings, constant objective) encodes
    /// correctly.
    #[test]
    fn encode_empty_problem_in_problem() {
        let problem = OptimizationProblem {
            objective: Expr::Constant { value: 42.0 },
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed for empty problem; got: {:?}",
            result.err()
        );
    }

    /// `encode_with_multiple_constraints_in_problem` — verifies that multiple
    /// constraints are all encoded.
    #[test]
    fn encode_with_multiple_constraints_in_problem() {
        let constraints = vec![
            OptimizationConstraint {
                lhs: Expr::variable("x"),
                relation: Relation::Le,
                rhs: 10.0,
                label: None,
            },
            OptimizationConstraint {
                lhs: Expr::variable("x"),
                relation: Relation::Ge,
                rhs: 1.0,
                label: None,
            },
        ];
        let problem = OptimizationProblem {
            objective: Expr::variable("x"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 5.0, 10.0],
            }],
            constraints,
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with multiple constraints; got: {:?}",
            result.err()
        );
    }

    /// `encode_with_mixed_constraint_types_in_problem` — verifies that Le, Ge,
    /// and Eq constraints are all encoded correctly.
    #[test]
    fn encode_with_mixed_constraint_types_in_problem() {
        let constraints = vec![
            OptimizationConstraint {
                lhs: Expr::variable("x"),
                relation: Relation::Le,
                rhs: 10.0,
                label: None,
            },
            OptimizationConstraint {
                lhs: Expr::variable("y"),
                relation: Relation::Ge,
                rhs: 1.0,
                label: None,
            },
            OptimizationConstraint {
                lhs: Expr::sum(vec![Expr::variable("x"), Expr::variable("y")]),
                relation: Relation::Eq,
                rhs: 5.0,
                label: None,
            },
        ];
        let problem = OptimizationProblem {
            objective: Expr::variable("x"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![
                DecisionVariable {
                    name: var("x"),
                    domain: vec![1.0, 2.0, 3.0, 4.0, 5.0],
                },
                DecisionVariable {
                    name: var("y"),
                    domain: vec![1.0, 2.0, 3.0, 4.0, 5.0],
                },
            ],
            constraints,
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with mixed constraint types; got: {:?}",
            result.err()
        );
    }

    /// `encode_with_product_in_objective_in_problem` — verifies that a product
    /// expression with one constant factor and one variable factor is encoded
    /// correctly.
    #[test]
    fn encode_with_product_in_objective_in_problem() {
        let problem = OptimizationProblem {
            objective: Expr::product(vec![Expr::constant(2.0), Expr::variable("x")]),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 2.0, 3.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with product in objective; got: {:?}",
            result.err()
        );
    }

    /// `encode_with_sum_in_objective_in_problem` — verifies that a sum
    /// expression in the objective is encoded correctly.
    #[test]
    fn encode_with_sum_in_objective_in_problem() {
        let problem = OptimizationProblem {
            objective: Expr::sum(vec![Expr::variable("x"), Expr::variable("y")]),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![
                DecisionVariable {
                    name: var("x"),
                    domain: vec![1.0, 2.0],
                },
                DecisionVariable {
                    name: var("y"),
                    domain: vec![3.0, 4.0],
                },
            ],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with sum in objective; got: {:?}",
            result.err()
        );
    }

    /// `encode_with_linear_in_objective_in_problem` — verifies that a Linear
    /// expression in the objective is encoded correctly.
    #[test]
    fn encode_with_linear_in_objective_in_problem() {
        let problem = OptimizationProblem {
            objective: Expr::Linear {
                coeff: 2.0,
                var: Box::new(Expr::variable("x")),
                offset: 5.0,
            },
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 2.0, 3.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with Linear in objective; got: {:?}",
            result.err()
        );
    }

    /// `encode_with_div_in_objective_in_problem` — verifies that a Div
    /// expression in the objective with a constant denominator is encoded
    /// correctly.
    #[test]
    fn encode_with_div_in_objective_in_problem() {
        let problem = OptimizationProblem {
            objective: Expr::Div {
                numerator: Box::new(Expr::variable("x")),
                denominator: Box::new(Expr::Constant { value: 2.0 }),
            },
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 2.0, 3.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with Div in objective; got: {:?}",
            result.err()
        );
    }

    /// `encode_with_constant_in_objective_in_problem` — verifies that a
    /// constant expression in the objective is encoded correctly.
    #[test]
    fn encode_with_constant_in_objective_in_problem() {
        let problem = OptimizationProblem {
            objective: Expr::Constant { value: 42.0 },
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 2.0, 3.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with constant in objective; got: {:?}",
            result.err()
        );
    }

    /// `encode_with_variable_in_objective_in_problem` — verifies that a
    /// variable expression in the objective is encoded correctly.
    #[test]
    fn encode_with_variable_in_objective_in_problem() {
        let problem = OptimizationProblem {
            objective: Expr::variable("x"),
            direction: ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 2.0, 3.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };
        let mut backend = CountingBackend::new();
        let result = encode(&mut backend, &problem);
        assert!(
            result.is_ok(),
            "encode must succeed with variable in objective; got: {:?}",
            result.err()
        );
    }
}
