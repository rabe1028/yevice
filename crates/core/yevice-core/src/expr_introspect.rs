//! Introspection layer for the `Expr` AST.
//!
//! Provides methods to extract variable sets and affine (linear) forms from a
//! cost expression. This is the foundation for the MILP solver backend
//! (`yevice-solver`), which needs to query the structure of an expression
//! without evaluating it: linearizability, big-M bounds, ceil-context safety.

use std::collections::{BTreeMap, BTreeSet};

use crate::cost::VariableBinding;
use crate::expr::Expr;
use crate::optimize::{ObjectiveDirection, OptimizationProblem, Relation};
use crate::types::VariableName;

/// Lower / upper bound pair for an expression value.
///
/// `f64::NEG_INFINITY` / `f64::INFINITY` are used when a side cannot be
/// derived from the supplied parameters and variable ranges.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub lower: f64,
    pub upper: f64,
}

impl Bounds {
    /// Single point bound (e.g. for a constant).
    fn point(value: f64) -> Self {
        Self {
            lower: value,
            upper: value,
        }
    }
    /// Unbounded on both sides.
    fn unbounded() -> Self {
        Self {
            lower: f64::NEG_INFINITY,
            upper: f64::INFINITY,
        }
    }
    /// True iff both bounds are finite.
    pub fn is_finite(&self) -> bool {
        self.lower.is_finite() && self.upper.is_finite()
    }
}

/// Known ranges for the variables appearing in an expression.
///
/// `decision_var_ranges` maps each decision variable to its `(min, max)`
/// domain extremes. Fixed parameters are passed via `fixed_params`.
#[derive(Debug, Clone, Default)]
pub struct VarRanges {
    pub decision_var_ranges: BTreeMap<VariableName, (f64, f64)>,
    pub fixed_params: BTreeMap<VariableName, f64>,
}

/// Affine form of an expression: `sum(coeff_i * var_i) + constant`.
#[derive(Debug, Clone, PartialEq)]
pub struct LinearForm {
    pub coefficients: BTreeMap<VariableName, f64>,
    pub constant: f64,
}

impl LinearForm {
    /// Scale every coefficient and the constant term by `factor`.
    fn scale(mut self, factor: f64) -> Self {
        for v in self.coefficients.values_mut() {
            *v *= factor;
        }
        self.constant *= factor;
        self
    }

    /// Add another `LinearForm` into `self` (merge coefficients and constants).
    fn add_assign(&mut self, other: Self) {
        for (var, coeff) in other.coefficients {
            *self.coefficients.entry(var).or_insert(0.0) += coeff;
        }
        self.constant += other.constant;
    }
}

impl Expr {
    /// All distinct variable names referenced anywhere in the expression.
    pub fn variables(&self) -> BTreeSet<VariableName> {
        let mut set = BTreeSet::new();
        collect_variables(self, &mut set);
        set
    }

    /// True iff the expression is affine in its variables (LP-expressible
    /// without auxiliary variables). Equivalent to `as_linear().is_some()`.
    pub fn is_linear(&self) -> bool {
        self.as_linear().is_some()
    }

    /// Extract the affine form, or `None` if non-linear.
    pub fn as_linear(&self) -> Option<LinearForm> {
        match self {
            Expr::Constant { value } => Some(LinearForm {
                coefficients: BTreeMap::new(),
                constant: *value,
            }),

            Expr::Variable { name } => {
                let mut coefficients = BTreeMap::new();
                coefficients.insert(name.clone(), 1.0);
                Some(LinearForm {
                    coefficients,
                    constant: 0.0,
                })
            }

            Expr::Linear { coeff, var, offset } => {
                let inner = var.as_linear()?;
                let mut result = inner.scale(*coeff);
                result.constant += offset;
                Some(result)
            }

            Expr::Sum { exprs } => {
                let mut acc = LinearForm {
                    coefficients: BTreeMap::new(),
                    constant: 0.0,
                };
                for e in exprs {
                    let lf = e.as_linear()?;
                    acc.add_assign(lf);
                }
                Some(acc)
            }

            Expr::Product { exprs } => {
                // Classify each factor as constant-only or variable-containing.
                //
                // A factor is "constant" when it has no non-zero coefficients —
                // this covers pure `Constant` nodes *and* degenerate cases such as
                // `0 * x` (which has coefficient x→0.0, effectively zero).
                // Only factors with at least one non-zero coefficient are treated
                // as "variable-containing".
                let mut constant_product = 1.0;
                let mut variable_factor: Option<LinearForm> = None;

                for e in exprs {
                    let lf = e.as_linear()?;
                    let has_nonzero_coeff = lf.coefficients.values().any(|&c| c != 0.0);
                    if has_nonzero_coeff {
                        // Variable-containing factor.
                        if variable_factor.is_some() {
                            // Two variable factors → non-linear product.
                            return None;
                        }
                        variable_factor = Some(lf);
                    } else {
                        // Effectively a constant factor (all coefficients are 0).
                        // The "value" of this factor is its constant term.
                        constant_product *= lf.constant;
                    }
                }

                Some(match variable_factor {
                    None => LinearForm {
                        coefficients: BTreeMap::new(),
                        constant: constant_product,
                    },
                    Some(lf) => lf.scale(constant_product),
                })
            }

            Expr::Div {
                numerator,
                denominator,
            } => {
                let d = denominator.as_linear()?;
                // Division by a variable-containing expression is non-linear.
                // A "variable-containing" denominator has at least one non-zero
                // coefficient.  A map like {y: 0.0} has all-zero coefficients
                // and is effectively a constant — same logic as Product.
                let has_nonzero = d.coefficients.values().any(|&c| c != 0.0);
                if has_nonzero {
                    return None;
                }
                // Division by zero is non-linear (undefined).
                if d.constant == 0.0 {
                    return None;
                }
                let n = numerator.as_linear()?;
                Some(n.scale(1.0 / d.constant))
            }

            // Non-linear or non-affine variants.
            Expr::Tiered { .. } | Expr::Max { .. } | Expr::Min { .. } | Expr::Ceil { .. } => None,
        }
    }
}

// ---------------------------------------------------------------------------
// expr_is_linearizable / linearizable shape check
// ---------------------------------------------------------------------------

/// True iff every sub-expression is a shape the MILP encoder knows how to
/// linearize (possibly by introducing auxiliary variables and constraints).
///
/// Supported shapes (recursively):
/// - `Constant`, `Variable`, `Linear`
/// - `Sum`, `Product` with at most one variable-containing factor
/// - `Div` whose denominator is constant
/// - `Tiered`, `Ceil` over linearizable inner expressions
/// - `Max` / `Min` over linearizable inner expressions
///
/// Rejected: `var * var`, `var / var`, and any nesting of these.
///
/// `decision_vars` is the set of decision-variable names; it is not used by
/// the structural check itself (linearizability is a syntactic property of
/// `Expr`), but is accepted for forward-compatibility (e.g. if a future
/// extension wants to treat fixed-only sub-expressions as "linearizable
/// constants").
#[must_use]
#[allow(clippy::only_used_in_recursion)]
pub fn expr_is_linearizable(expr: &Expr, decision_vars: &BTreeSet<VariableName>) -> bool {
    match expr {
        Expr::Constant { .. } | Expr::Variable { .. } => true,
        Expr::Linear { var, .. } => expr_is_linearizable(var, decision_vars),
        Expr::Sum { exprs } => exprs.iter().all(|e| expr_is_linearizable(e, decision_vars)),
        Expr::Product { exprs } => {
            // At most one factor may contain variables; every factor itself
            // must also be linearizable.
            if !exprs.iter().all(|e| expr_is_linearizable(e, decision_vars)) {
                return false;
            }
            let var_count = exprs
                .iter()
                .filter(|e| match e.as_linear() {
                    Some(lf) => lf.coefficients.values().any(|&c| c != 0.0),
                    None => true, // non-linear factor — but if it's a Tiered/Ceil/Max/Min
                                  // we still want to allow `constant * Ceil(x)` etc.
                                  // The encoder evaluates each factor individually:
                                  // any factor that is non-affine yields an aux var,
                                  // and the surrounding Product needs at most one
                                  // such "variable-containing" factor.
                })
                .count();
            var_count <= 1
        }
        Expr::Div {
            numerator,
            denominator,
        } => {
            if !expr_is_linearizable(numerator, decision_vars) {
                return false;
            }
            // Denominator must be a pure constant expression.
            match denominator.as_linear() {
                Some(lf) => {
                    let has_var = lf.coefficients.values().any(|&c| c != 0.0);
                    !has_var && lf.constant != 0.0
                }
                None => false,
            }
        }
        Expr::Tiered { var, .. } | Expr::Ceil { expr: var } => {
            expr_is_linearizable(var, decision_vars)
        }
        Expr::Max { expr, .. } | Expr::Min { expr, .. } => {
            expr_is_linearizable(expr, decision_vars)
        }
    }
}

// ---------------------------------------------------------------------------
// expr_bounds: interval-arithmetic style big-M derivation
// ---------------------------------------------------------------------------

/// Compute conservative `(lower, upper)` bounds for the value of an expression
/// under the given variable ranges and fixed parameters.
///
/// The bounds are derived by interval arithmetic on the AST. Unknown variables
/// (not in `ranges.decision_var_ranges` and not in `ranges.fixed_params`)
/// yield `(-inf, +inf)`. Division by an interval that contains zero yields
/// `(-inf, +inf)`.
///
/// The result is conservative (a true upper bound on the value range), not
/// necessarily tight. Callers use it to pick a finite big-M; a non-finite
/// result indicates the encoder must reject the problem
/// (`SolverError::UnboundedExpression`).
#[must_use]
pub fn expr_bounds(expr: &Expr, ranges: &VarRanges) -> Bounds {
    match expr {
        Expr::Constant { value } => Bounds::point(*value),

        Expr::Variable { name } => {
            // Decision variables win over fixed params on a name collision,
            // matching the documented last-write-wins semantics of
            // `EnumerationSolver` (a decision-variable slot's value is
            // written into `scratch` after fixed_params, and the MILP
            // encoder also skips a colliding fixed param). Using the fixed
            // value here would produce a bogus point bound that ignores
            // the decision domain.
            if let Some(&(lo, hi)) = ranges.decision_var_ranges.get(name) {
                return Bounds {
                    lower: lo,
                    upper: hi,
                };
            }
            if let Some(&v) = ranges.fixed_params.get(name) {
                return Bounds::point(v);
            }
            Bounds::unbounded()
        }

        Expr::Linear { coeff, var, offset } => {
            let b = expr_bounds(var, ranges);
            scale_then_shift(b, *coeff, *offset)
        }

        Expr::Sum { exprs } => {
            exprs
                .iter()
                .map(|e| expr_bounds(e, ranges))
                .fold(Bounds::point(0.0), |a, b| Bounds {
                    lower: a.lower + b.lower,
                    upper: a.upper + b.upper,
                })
        }

        Expr::Product { exprs } => exprs
            .iter()
            .map(|e| expr_bounds(e, ranges))
            .fold(Bounds::point(1.0), interval_mul),

        Expr::Div {
            numerator,
            denominator,
        } => {
            let n = expr_bounds(numerator, ranges);
            let d = expr_bounds(denominator, ranges);
            interval_div(n, d)
        }

        Expr::Max { expr, floor } => {
            let b = expr_bounds(expr, ranges);
            Bounds {
                lower: b.lower.max(*floor),
                upper: b.upper.max(*floor),
            }
        }

        Expr::Min { expr, ceiling } => {
            let b = expr_bounds(expr, ranges);
            Bounds {
                lower: b.lower.min(*ceiling),
                upper: b.upper.min(*ceiling),
            }
        }

        Expr::Ceil { expr } => {
            let b = expr_bounds(expr, ranges);
            // ceil(x) ∈ [ceil(lo), ceil(hi)] for finite bounds; preserve infinities.
            Bounds {
                lower: if b.lower.is_finite() {
                    b.lower.ceil()
                } else {
                    b.lower
                },
                upper: if b.upper.is_finite() {
                    b.upper.ceil()
                } else {
                    b.upper
                },
            }
        }

        Expr::Tiered { tiers, var } => {
            // Tiered cost over a variable with bounds [lo, hi] yields a cost
            // bound of [tiered_eval(max(0, lo)), tiered_eval(max(0, hi))].
            // We compute the eval explicitly using the tiered formula.
            let b = expr_bounds(var, ranges);
            let lo_val = if b.lower.is_finite() {
                Some(tiered_eval(tiers, b.lower.max(0.0)))
            } else {
                None
            };
            let hi_val = if b.upper.is_finite() {
                Some(tiered_eval(tiers, b.upper.max(0.0)))
            } else {
                None
            };
            Bounds {
                lower: lo_val.unwrap_or(f64::NEG_INFINITY),
                upper: hi_val.unwrap_or(f64::INFINITY),
            }
        }
    }
}

fn scale_then_shift(b: Bounds, coeff: f64, offset: f64) -> Bounds {
    let (lo, hi) = if coeff >= 0.0 {
        (b.lower * coeff, b.upper * coeff)
    } else {
        (b.upper * coeff, b.lower * coeff)
    };
    Bounds {
        lower: lo + offset,
        upper: hi + offset,
    }
}

fn interval_mul(a: Bounds, b: Bounds) -> Bounds {
    let candidates = [
        a.lower * b.lower,
        a.lower * b.upper,
        a.upper * b.lower,
        a.upper * b.upper,
    ];
    let lower = candidates.iter().copied().fold(f64::INFINITY, f64::min);
    let upper = candidates.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    Bounds { lower, upper }
}

fn interval_div(n: Bounds, d: Bounds) -> Bounds {
    // If the denominator interval straddles zero, the result is unbounded.
    if d.lower <= 0.0 && d.upper >= 0.0 {
        return Bounds::unbounded();
    }
    let candidates = [
        n.lower / d.lower,
        n.lower / d.upper,
        n.upper / d.lower,
        n.upper / d.upper,
    ];
    let lower = candidates.iter().copied().fold(f64::INFINITY, f64::min);
    let upper = candidates.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    Bounds { lower, upper }
}

fn tiered_eval(tiers: &[crate::expr::Tier], usage: f64) -> f64 {
    let mut total = 0.0;
    let mut remaining = usage;
    let mut prev_limit = 0.0;
    for tier in tiers {
        if remaining <= 0.0 {
            break;
        }
        let width = match tier.upper_limit {
            Some(limit) => limit - prev_limit,
            None => remaining,
        };
        let consumed = remaining.min(width);
        total += consumed * tier.unit_price;
        remaining -= consumed;
        if let Some(limit) = tier.upper_limit {
            prev_limit = limit;
        }
    }
    total
}

// ---------------------------------------------------------------------------
// substitute_fixed_params: inline known constant values into an expression
// ---------------------------------------------------------------------------

/// Replace every `Variable { name }` whose name appears in `fixed_params`
/// with the corresponding `Constant`. Used by the MILP encoder so that
/// expressions like `price * x` — where `price` is a CLI-supplied fixed
/// parameter and `x` is the only true decision variable — pass the
/// linearizability check.
#[must_use]
pub fn substitute_fixed_params(expr: &Expr, fixed_params: &BTreeMap<VariableName, f64>) -> Expr {
    match expr {
        Expr::Constant { .. } => expr.clone(),
        Expr::Variable { name } => {
            if let Some(&v) = fixed_params.get(name) {
                Expr::Constant { value: v }
            } else {
                expr.clone()
            }
        }
        Expr::Linear { coeff, var, offset } => Expr::Linear {
            coeff: *coeff,
            var: Box::new(substitute_fixed_params(var, fixed_params)),
            offset: *offset,
        },
        Expr::Tiered { tiers, var } => Expr::Tiered {
            tiers: tiers.clone(),
            var: Box::new(substitute_fixed_params(var, fixed_params)),
        },
        Expr::Sum { exprs } => Expr::Sum {
            exprs: exprs
                .iter()
                .map(|e| substitute_fixed_params(e, fixed_params))
                .collect(),
        },
        Expr::Product { exprs } => Expr::Product {
            exprs: exprs
                .iter()
                .map(|e| substitute_fixed_params(e, fixed_params))
                .collect(),
        },
        Expr::Max { expr, floor } => Expr::Max {
            expr: Box::new(substitute_fixed_params(expr, fixed_params)),
            floor: *floor,
        },
        Expr::Min { expr, ceiling } => Expr::Min {
            expr: Box::new(substitute_fixed_params(expr, fixed_params)),
            ceiling: *ceiling,
        },
        Expr::Ceil { expr } => Expr::Ceil {
            expr: Box::new(substitute_fixed_params(expr, fixed_params)),
        },
        Expr::Div {
            numerator,
            denominator,
        } => Expr::Div {
            numerator: Box::new(substitute_fixed_params(numerator, fixed_params)),
            denominator: Box::new(substitute_fixed_params(denominator, fixed_params)),
        },
    }
}

// ---------------------------------------------------------------------------
// substitute_bindings: inline bindings into an expression
// ---------------------------------------------------------------------------

/// Recursively inline every binding-target variable reference with the
/// binding's expression, until no further substitution is possible.
///
/// Assumes the binding graph is acyclic (validated up-front by
/// `validate_bindings`). A safety counter caps recursion at
/// `MAX_SUBSTITUTION_PASSES` passes to guarantee termination even on
/// adversarial input.
const MAX_SUBSTITUTION_PASSES: usize = 64;

#[must_use]
pub fn substitute_bindings(expr: &Expr, bindings: &[VariableBinding]) -> Expr {
    let mut current = expr.clone();
    for _ in 0..MAX_SUBSTITUTION_PASSES {
        let next = substitute_once(&current, bindings);
        if next == current {
            return current;
        }
        current = next;
    }
    current
}

fn substitute_once(expr: &Expr, bindings: &[VariableBinding]) -> Expr {
    match expr {
        Expr::Constant { .. } => expr.clone(),
        Expr::Variable { name } => {
            for b in bindings {
                if &b.target == name {
                    return b.expr.clone();
                }
            }
            expr.clone()
        }
        Expr::Linear { coeff, var, offset } => Expr::Linear {
            coeff: *coeff,
            var: Box::new(substitute_once(var, bindings)),
            offset: *offset,
        },
        Expr::Tiered { tiers, var } => Expr::Tiered {
            tiers: tiers.clone(),
            var: Box::new(substitute_once(var, bindings)),
        },
        Expr::Sum { exprs } => Expr::Sum {
            exprs: exprs.iter().map(|e| substitute_once(e, bindings)).collect(),
        },
        Expr::Product { exprs } => Expr::Product {
            exprs: exprs.iter().map(|e| substitute_once(e, bindings)).collect(),
        },
        Expr::Max { expr, floor } => Expr::Max {
            expr: Box::new(substitute_once(expr, bindings)),
            floor: *floor,
        },
        Expr::Min { expr, ceiling } => Expr::Min {
            expr: Box::new(substitute_once(expr, bindings)),
            ceiling: *ceiling,
        },
        Expr::Ceil { expr } => Expr::Ceil {
            expr: Box::new(substitute_once(expr, bindings)),
        },
        Expr::Div {
            numerator,
            denominator,
        } => Expr::Div {
            numerator: Box::new(substitute_once(numerator, bindings)),
            denominator: Box::new(substitute_once(denominator, bindings)),
        },
    }
}

// ---------------------------------------------------------------------------
// first_resolvable_bindings: first-resolvable semantics binding filter
// ---------------------------------------------------------------------------

/// Filter `bindings` to retain only the **first resolvable** binding for each
/// target, mirroring the semantics used by `EnumerationSolver`'s
/// `resolve_bindings_into` and the MILP encoder.
///
/// A binding is *resolvable* when every variable referenced in its RHS is
/// either already in `known_names` (decision variables, fixed parameters) or
/// has already been selected as the first-resolvable binding for an earlier
/// target.  The function runs a fixed-point loop so that a binding whose RHS
/// references another binding target (a chain) resolves correctly regardless
/// of declaration order.
///
/// Bindings whose target is already in `known_names` (shadowed by a decision
/// variable or fixed parameter) are dropped entirely, consistent with the
/// encoder and enumerator "decision wins" rule.
///
/// The returned `Vec` preserves the relative order of the selected bindings
/// within the original slice.
#[must_use]
pub fn first_resolvable_bindings(
    bindings: &[VariableBinding],
    known_names: &BTreeSet<VariableName>,
) -> Vec<VariableBinding> {
    // `resolved` tracks all names we consider "defined": starts with the
    // caller-supplied set (decision vars + fixed params), grows as we select
    // each binding.
    let mut resolved: BTreeSet<VariableName> = known_names.clone();
    // `selected_index` records the index into `bindings` of the chosen
    // binding for each target.  BTreeMap gives us stable key ordering, but
    // we re-sort by value (original index) before collecting.
    let mut selected_index: BTreeMap<&VariableName, usize> = BTreeMap::new();
    // Targets that are shadowed by known_names (decision vars / fixed params)
    // must never be selected; track them explicitly so we can skip duplicates.
    let shadowed: BTreeSet<&VariableName> = bindings
        .iter()
        .map(|b| &b.target)
        .filter(|t| known_names.contains(*t))
        .collect();

    let max_passes = bindings.len() + 1;
    for _ in 0..max_passes {
        let mut progressed = false;
        for (i, binding) in bindings.iter().enumerate() {
            // Skip targets shadowed by decision vars / fixed params.
            if shadowed.contains(&binding.target) {
                continue;
            }
            // Skip if we already picked a binding for this target.
            if selected_index.contains_key(&binding.target) {
                continue;
            }
            // A binding is resolvable if every variable in its RHS is known.
            let all_known = binding
                .expr
                .variables()
                .iter()
                .all(|v| resolved.contains(v));
            if all_known {
                selected_index.insert(&binding.target, i);
                resolved.insert(binding.target.clone());
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }

    // Collect selected bindings in original slice order.
    let mut result: Vec<(usize, &VariableBinding)> = selected_index
        .values()
        .map(|&i| (i, &bindings[i]))
        .collect();
    result.sort_by_key(|(i, _)| *i);
    result.into_iter().map(|(_, b)| b.clone()).collect()
}

// ---------------------------------------------------------------------------
// classify_ceil_context: ADR-0002 Ceil safety classifier
// ---------------------------------------------------------------------------

/// Result of classifying a ceil expression's context.
#[derive(Debug, Clone)]
pub enum CeilContextResult {
    /// All ceil occurrences appear in auto-tight contexts.
    Ok,
    /// At least one ceil occurrence appears in an anti-tight context.
    /// Carries the offending expression snippet and a static reason.
    Reject {
        expr_repr: String,
        reason: &'static str,
    },
}

/// Classify every `Ceil(...)` occurrence in the problem (after bindings
/// expansion) and decide whether the lower-bound-only formulation
/// (`expr <= y`, `y integer`) is safe.
///
/// See ADR-0002 "Ceil 定式化の選択" for the full classification:
///
/// - **Allowed**: minimization objective with positive coefficient, Le-LHS
///   with positive coefficient, Ge-LHS with negative coefficient, or
///   appearance in a constant-only sub-expression (handled by `evaluate`
///   before reaching the MILP encoder).
/// - **Rejected**: maximize objective containing ceil, negative coefficient
///   in objective, any appearance in `Eq` constraint LHS, positive coefficient
///   in Ge-LHS, negative coefficient in Le-LHS.
///
/// The classifier walks the expanded objective and each expanded constraint
/// LHS, finding ceil nodes and probing their coefficient in the surrounding
/// affine context.
#[must_use]
pub fn classify_ceil_context(problem: &OptimizationProblem) -> CeilContextResult {
    // Decision-variable names override fixed_params on collision (matches the
    // encoder's and `pre_check`'s behaviour).
    let decision_names: BTreeSet<VariableName> = problem
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
    // Normalise: expand bindings first (using first-resolvable semantics to
    // match EnumerationSolver and the MILP encoder), then inline fixed params
    // as constants.  Using `substitute_bindings` directly would pick the first
    // *textual* binding for each target, which can expand an unresolvable RHS
    // (`b = missing * other`) before a valid later binding (`b = x`) is seen.
    let known_names: BTreeSet<VariableName> = decision_names
        .iter()
        .cloned()
        .chain(fixed_param_map.keys().cloned())
        .collect();
    let resolvable = first_resolvable_bindings(&problem.bindings, &known_names);
    let normalise = |e: &Expr| -> Expr {
        let after_bindings = substitute_bindings(e, &resolvable);
        substitute_fixed_params(&after_bindings, &fixed_param_map)
    };

    let objective = normalise(&problem.objective);
    let direction = problem.direction;

    // Objective check.
    let obj_ceils = find_ceils_with_coeff_sign(&objective);
    for (expr_snippet, sign) in obj_ceils {
        match (direction, sign) {
            (ObjectiveDirection::Minimize, CoeffSign::Positive) => {} // ok
            // Zero coefficient: the ceil is effectively absent; skip.
            (_, CoeffSign::Zero) => {}
            (ObjectiveDirection::Minimize, CoeffSign::Negative) => {
                return CeilContextResult::Reject {
                    expr_repr: expr_snippet,
                    reason: "ceil appears with negative coefficient in minimization objective",
                };
            }
            (ObjectiveDirection::Maximize, _) => {
                return CeilContextResult::Reject {
                    expr_repr: expr_snippet,
                    reason: "ceil cannot appear in a maximization objective",
                };
            }
            (_, CoeffSign::Unknown) => {
                return CeilContextResult::Reject {
                    expr_repr: expr_snippet,
                    reason: "ceil coefficient sign in objective is undetermined",
                };
            }
        }
    }

    // Constraint checks.
    for c in &problem.constraints {
        let lhs = normalise(&c.lhs);
        let ceils = find_ceils_with_coeff_sign(&lhs);
        for (expr_snippet, sign) in ceils {
            let allowed = match (c.relation, sign) {
                (Relation::Le, CoeffSign::Positive) => true,
                (Relation::Ge, CoeffSign::Negative) => true,
                // Zero coefficient: ceil is effectively absent; always allowed.
                (_, CoeffSign::Zero) => true,
                (Relation::Eq, _) => false,
                (Relation::Le, CoeffSign::Negative) => false,
                (Relation::Ge, CoeffSign::Positive) => false,
                (_, CoeffSign::Unknown) => false,
            };
            if !allowed {
                let reason: &'static str = match (c.relation, sign) {
                    (Relation::Eq, _) => "ceil cannot appear in an Eq constraint",
                    (Relation::Le, CoeffSign::Negative) => {
                        "ceil appears with negative coefficient in Le constraint LHS"
                    }
                    (Relation::Ge, CoeffSign::Positive) => {
                        "ceil appears with positive coefficient in Ge constraint LHS"
                    }
                    _ => "ceil coefficient sign in constraint is undetermined",
                };
                return CeilContextResult::Reject {
                    expr_repr: expr_snippet,
                    reason,
                };
            }
        }
    }

    CeilContextResult::Ok
}

#[derive(Debug, Clone, Copy)]
enum CoeffSign {
    Positive,
    Negative,
    Unknown,
    /// The effective coefficient is (approximately) zero; the ceil is
    /// multiplied away and has no impact on the objective / constraint.
    Zero,
}

/// Find every `Ceil(...)` node in `expr` and report the sign of its effective
/// coefficient in the surrounding affine context.
///
/// We walk the expression tree once, tracking a multiplicative "outer
/// coefficient sign" as we descend through `Sum` / `Linear` / `Product`
/// (by constant factor) / `Div` (by constant denominator). When we hit a
/// ceil, we record the sign at that point.
///
/// Sums propagate the parent sign unchanged; `Linear(coeff, var, _)`
/// multiplies the sign by `sign(coeff)`; `Product` multiplies the sign by
/// the product of all sibling constant factors (variable-containing sibling
/// factors yield `Unknown`). `Min`/`Max`/`Tiered`/inner `Ceil` are not
/// classified — their inner ceils would themselves need analysis, but the
/// current ADR scope only reaches ceils through linear contexts.
fn find_ceils_with_coeff_sign(expr: &Expr) -> Vec<(String, CoeffSign)> {
    let mut out = Vec::new();
    walk(expr, CoeffSign::Positive, &mut out);
    out
}

/// Threshold below which a coefficient is treated as zero (same constant used
/// in the MILP encoder).
const COEFF_EPS: f64 = 1e-12;

fn multiply_sign(a: CoeffSign, factor: f64) -> CoeffSign {
    if factor.abs() < COEFF_EPS {
        // A zero (or near-zero) coefficient makes the ceil term vanish.
        // Return Zero so callers can skip this occurrence entirely rather than
        // incorrectly treating it as an undetermined (Unknown) context.
        return CoeffSign::Zero;
    }
    let f_sign = if factor > 0.0 {
        CoeffSign::Positive
    } else {
        CoeffSign::Negative
    };
    match (a, f_sign) {
        (CoeffSign::Zero, _) | (_, CoeffSign::Zero) => CoeffSign::Zero,
        (CoeffSign::Positive, CoeffSign::Positive) => CoeffSign::Positive,
        (CoeffSign::Negative, CoeffSign::Negative) => CoeffSign::Positive,
        (CoeffSign::Positive, CoeffSign::Negative) | (CoeffSign::Negative, CoeffSign::Positive) => {
            CoeffSign::Negative
        }
        _ => CoeffSign::Unknown,
    }
}

fn walk(expr: &Expr, outer: CoeffSign, out: &mut Vec<(String, CoeffSign)>) {
    match expr {
        Expr::Constant { .. } | Expr::Variable { .. } => {}
        Expr::Linear {
            coeff,
            var,
            offset: _,
        } => {
            walk(var, multiply_sign(outer, *coeff), out);
        }
        Expr::Sum { exprs } => {
            for e in exprs {
                walk(e, outer, out);
            }
        }
        Expr::Product { exprs } => {
            // Compute the product of all constant factors; if any factor is
            // non-constant, we cannot determine the sign for any ceil inside
            // a sibling factor, but the ceil itself may not appear under it.
            // Conservative pass: walk each child with sign = outer * (product
            // of sibling constants); when a sibling is non-constant, mark
            // Unknown.
            // Compute the product of all constant-only factors plus a count
            // of variable-containing (non-constant) factors. Linearizable
            // expressions have at most one such non-constant factor; if more
            // than one appears, we cannot determine the sign of a ceil that
            // hides inside any single factor.
            let mut all_const_product = 1.0;
            let mut non_constant_count = 0usize;
            for e in exprs {
                match e.as_linear() {
                    Some(lf) if lf.coefficients.values().all(|&c| c == 0.0) => {
                        all_const_product *= lf.constant;
                    }
                    _ => {
                        non_constant_count += 1;
                    }
                }
            }
            for e in exprs {
                // Constant-only factors contain no ceil to classify.
                let is_const = matches!(
                    e.as_linear(),
                    Some(ref lf) if lf.coefficients.values().all(|&c| c == 0.0)
                );
                if is_const {
                    continue;
                }
                // Walking child `e` itself — siblings are the *other* children.
                // For a linearizable Product (non_constant_count <= 1) every
                // sibling is constant, so sibling product = all_const_product.
                // Only when there are TWO+ non-constant factors do we lose
                // sign information.
                let child_outer = if non_constant_count > 1 {
                    CoeffSign::Unknown
                } else {
                    multiply_sign(outer, all_const_product)
                };
                walk(e, child_outer, out);
            }
        }
        Expr::Div {
            numerator,
            denominator,
        } => {
            // Denominator must be constant for linearizability; the sign is
            // the sign of 1 / denom_const.
            match denominator.as_linear() {
                Some(lf) if lf.coefficients.values().all(|&c| c == 0.0) && lf.constant != 0.0 => {
                    let sign = multiply_sign(outer, 1.0 / lf.constant);
                    walk(numerator, sign, out);
                }
                _ => {
                    walk(numerator, CoeffSign::Unknown, out);
                }
            }
        }
        Expr::Ceil { expr: inner } => {
            // A zero effective coefficient means the ceil term is multiplied
            // away; it has no impact on the objective/constraint value, so
            // skip recording it (treating it as absent prevents spurious
            // Unknown-context rejections for free/zero-priced components).
            if !matches!(outer, CoeffSign::Zero) {
                out.push((format!("ceil({:?})", flatten_for_debug(inner)), outer));
            }
            // Walk inside for nested ceils, with sign Unknown.
            walk(inner, CoeffSign::Unknown, out);
        }
        Expr::Max { expr, .. } | Expr::Min { expr, .. } => {
            // Ceils inside Max/Min defy the simple sign analysis; mark Unknown.
            walk(expr, CoeffSign::Unknown, out);
        }
        Expr::Tiered { var, .. } => {
            walk(var, CoeffSign::Unknown, out);
        }
    }
}

/// Minimal debug-style rendering for `expr_repr` strings (avoids leaking the
/// full Debug output of large subtrees while still naming the relevant variable).
fn flatten_for_debug(expr: &Expr) -> String {
    match expr {
        Expr::Constant { value } => format!("{value}"),
        Expr::Variable { name } => name.to_string(),
        Expr::Linear { coeff, var, offset } => {
            format!("{coeff}*{var} + {offset}", var = flatten_for_debug(var))
        }
        _ => format!("{expr:?}"),
    }
}

/// Recursively collect all `Variable` names from an expression into `set`.
fn collect_variables(expr: &Expr, set: &mut BTreeSet<VariableName>) {
    match expr {
        Expr::Constant { .. } => {}
        Expr::Variable { name } => {
            set.insert(name.clone());
        }
        Expr::Linear { var, .. } => collect_variables(var, set),
        Expr::Tiered { tiers: _, var } => collect_variables(var, set),
        Expr::Sum { exprs } => {
            for e in exprs {
                collect_variables(e, set);
            }
        }
        Expr::Product { exprs } => {
            for e in exprs {
                collect_variables(e, set);
            }
        }
        Expr::Max { expr, .. } => collect_variables(expr, set),
        Expr::Min { expr, .. } => collect_variables(expr, set),
        Expr::Ceil { expr } => collect_variables(expr, set),
        Expr::Div {
            numerator,
            denominator,
        } => {
            collect_variables(numerator, set);
            collect_variables(denominator, set);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::{Expr, Tier};

    fn var(name: &str) -> VariableName {
        VariableName::new(name)
    }

    // -------------------------------------------------------------------------
    // variables()
    // -------------------------------------------------------------------------

    #[test]
    fn variables_collects_all_nested() {
        // Sum( Linear(x), Div(y, z), Product(x, Constant(2)) )
        let expr = Expr::sum(vec![
            Expr::linear(3.0, Expr::variable("x"), 0.0),
            Expr::div(Expr::variable("y"), Expr::variable("z")),
            Expr::product(vec![Expr::variable("x"), Expr::constant(2.0)]),
        ]);
        let vars = expr.variables();
        assert!(vars.contains(&var("x")));
        assert!(vars.contains(&var("y")));
        assert!(vars.contains(&var("z")));
        assert_eq!(vars.len(), 3);
    }

    // -------------------------------------------------------------------------
    // as_linear() — basic cases
    // -------------------------------------------------------------------------

    #[test]
    fn as_linear_constant_variable_linear() {
        // Constant
        let c = Expr::constant(7.0).as_linear().unwrap();
        assert_eq!(c.coefficients.len(), 0);
        assert_eq!(c.constant, 7.0);

        // Variable
        let v = Expr::variable("x").as_linear().unwrap();
        assert_eq!(v.coefficients[&var("x")], 1.0);
        assert_eq!(v.constant, 0.0);

        // Linear(2.0 * x + 5.0)
        let l = Expr::linear(2.0, Expr::variable("x"), 5.0)
            .as_linear()
            .unwrap();
        assert_eq!(l.coefficients[&var("x")], 2.0);
        assert_eq!(l.constant, 5.0);
    }

    // -------------------------------------------------------------------------
    // as_linear() — Sum merges coefficients
    // -------------------------------------------------------------------------

    #[test]
    fn as_linear_sum_merges_coefficients() {
        // 3x + 2x + 1  →  5x + 1
        let expr = Expr::sum(vec![
            Expr::linear(3.0, Expr::variable("x"), 0.0),
            Expr::linear(2.0, Expr::variable("x"), 1.0),
        ]);
        let lf = expr.as_linear().unwrap();
        assert_eq!(lf.coefficients[&var("x")], 5.0);
        assert_eq!(lf.constant, 1.0);
    }

    // -------------------------------------------------------------------------
    // as_linear() — Product
    // -------------------------------------------------------------------------

    #[test]
    fn as_linear_product_constant_times_linear() {
        // Constant(3) * Variable(x)  →  3x
        let expr = Expr::product(vec![Expr::constant(3.0), Expr::variable("x")]);
        let lf = expr.as_linear().unwrap();
        assert_eq!(lf.coefficients[&var("x")], 3.0);
        assert_eq!(lf.constant, 0.0);

        // Variable(x) * Variable(y)  →  None
        let non_linear = Expr::product(vec![Expr::variable("x"), Expr::variable("y")]);
        assert!(non_linear.as_linear().is_none());
    }

    // -------------------------------------------------------------------------
    // as_linear() — Div
    // -------------------------------------------------------------------------

    #[test]
    fn as_linear_div_by_constant_ok_div_by_variable_none() {
        // (2x + 4) / 2  →  x + 2
        let expr = Expr::div(
            Expr::linear(2.0, Expr::variable("x"), 4.0),
            Expr::constant(2.0),
        );
        let lf = expr.as_linear().unwrap();
        assert_eq!(lf.coefficients[&var("x")], 1.0);
        assert_eq!(lf.constant, 2.0);

        // x / y  →  None (variable denominator)
        let div_by_var = Expr::div(Expr::variable("x"), Expr::variable("y"));
        assert!(div_by_var.as_linear().is_none());

        // x / 0  →  None (division by zero)
        let div_by_zero = Expr::div(Expr::variable("x"), Expr::constant(0.0));
        assert!(div_by_zero.as_linear().is_none());
    }

    // -------------------------------------------------------------------------
    // as_linear() — Non-linear variants
    // -------------------------------------------------------------------------

    #[test]
    fn as_linear_tiered_max_min_ceil_are_none() {
        let tiered = Expr::tiered(
            vec![Tier {
                upper_limit: Some(100.0),
                unit_price: 0.01,
            }],
            Expr::variable("x"),
        );
        assert!(tiered.as_linear().is_none());

        let max_expr = Expr::Max {
            expr: Box::new(Expr::variable("x")),
            floor: 0.0,
        };
        assert!(max_expr.as_linear().is_none());

        let min_expr = Expr::Min {
            expr: Box::new(Expr::variable("x")),
            ceiling: 100.0,
        };
        assert!(min_expr.as_linear().is_none());

        let ceil_expr = Expr::ceil(Expr::variable("x"));
        assert!(ceil_expr.as_linear().is_none());
    }

    // -------------------------------------------------------------------------
    // as_linear() — zero-coefficient Product (#8)
    // -------------------------------------------------------------------------

    /// `0 * x` has a coefficient for `x` of 0.0.  The product must be treated
    /// as the constant 0 (empty coefficients map, constant = 0.0).
    #[test]
    fn as_linear_product_zero_coeff_factor_is_constant_zero() {
        // Linear(0.0, x, 0.0) → coefficients: {x: 0.0}, constant: 0.0
        let zero_x = Expr::linear(0.0, Expr::variable("x"), 0.0);
        let lf = zero_x.as_linear().unwrap();
        assert!(
            lf.coefficients.values().all(|&c| c == 0.0),
            "0*x should have zero coefficient: {lf:?}"
        );

        // Product(Linear(0, x), Variable(y)) → effectively 0 * y → constant 0
        let product = Expr::product(vec![
            Expr::linear(0.0, Expr::variable("x"), 0.0),
            Expr::variable("y"),
        ]);
        let lf_prod = product.as_linear().unwrap();
        // The result should be: {y: 0.0} (or empty map), constant 0.
        // The important property: no non-zero y coefficient.
        let y_coeff = lf_prod.coefficients.get(&var("y")).copied().unwrap_or(0.0);
        assert_eq!(
            y_coeff, 0.0,
            "0*x * y should yield coefficient 0 for y, got {y_coeff}"
        );
        assert_eq!(lf_prod.constant, 0.0);
    }

    /// `(0*x + 5) * y` — the `0*x + 5` factor has coefficient 0 for x and
    /// constant 5.  It should be treated as constant 5, giving `{y: 5}`.
    #[test]
    fn as_linear_product_zero_coeff_plus_constant_times_var() {
        // Sum(Linear(0,x,0), Constant(5)) → constant-only factor with value 5
        let factor_lhs = Expr::sum(vec![
            Expr::linear(0.0, Expr::variable("x"), 0.0),
            Expr::constant(5.0),
        ]);
        let product = Expr::product(vec![factor_lhs, Expr::variable("y")]);
        let lf = product.as_linear().unwrap();
        let y_coeff = lf.coefficients.get(&var("y")).copied().unwrap_or(0.0);
        assert_eq!(y_coeff, 5.0, "(0*x + 5) * y should give y coefficient 5");
        assert_eq!(lf.constant, 0.0);
    }

    // -------------------------------------------------------------------------
    // as_linear() — Div with zero-coefficient denominator (#4 fix)
    // -------------------------------------------------------------------------

    /// `(2x+4) / (0*y + 5)` — the denominator `0*y + 5` has coefficient y=0.0
    /// and constant 5.  It must be treated as the constant 5 (not non-linear),
    /// giving LinearForm {x: 0.4, constant: 0.8}.
    #[test]
    fn as_linear_div_by_zero_coeff_plus_constant_is_linear() {
        // denominator: 0*y + 5
        let denom = Expr::sum(vec![
            Expr::linear(0.0, Expr::variable("y"), 0.0),
            Expr::constant(5.0),
        ]);
        // numerator: 2x + 4
        let numerator = Expr::linear(2.0, Expr::variable("x"), 4.0);
        let expr = Expr::div(numerator, denom);
        let lf = expr
            .as_linear()
            .expect("(2x+4)/(0*y+5) must be linear: denominator is effectively constant 5");
        assert!(
            (lf.coefficients[&var("x")] - 0.4).abs() < 1e-12,
            "x coefficient should be 0.4, got {:?}",
            lf.coefficients[&var("x")]
        );
        assert!(
            (lf.constant - 0.8).abs() < 1e-12,
            "constant should be 0.8, got {}",
            lf.constant
        );
    }

    // -------------------------------------------------------------------------
    // is_linear() matches as_linear().is_some()
    // -------------------------------------------------------------------------

    #[test]
    fn is_linear_matches_as_linear_some() {
        let linear = Expr::linear(2.0, Expr::variable("x"), 1.0);
        assert!(linear.is_linear());
        assert_eq!(linear.is_linear(), linear.as_linear().is_some());

        let non_linear = Expr::ceil(Expr::variable("x"));
        assert!(!non_linear.is_linear());
        assert_eq!(non_linear.is_linear(), non_linear.as_linear().is_some());
    }

    // -------------------------------------------------------------------------
    // classify_ceil_context — fixed_params folding (#37)
    // -------------------------------------------------------------------------

    /// `price * ceil(x)` where `price` is a fixed param and `x` is a decision
    /// variable must be classified as `Ok` (positive coefficient, Minimize).
    /// Before the fix, `price` was not inlined and the product counted as
    /// having an unknown coefficient.
    #[test]
    fn classify_ceil_context_folds_fixed_params_before_classifying() {
        use crate::optimize::{DecisionVariable, OptimizationProblem};
        use std::collections::HashMap;

        // objective = price * ceil(x),  direction = Minimize
        // price is a fixed param (2.0),  x is a decision variable
        let objective = Expr::product(vec![
            Expr::variable("price"),
            Expr::ceil(Expr::variable("x")),
        ]);
        let problem = OptimizationProblem {
            objective,
            direction: crate::optimize::ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 2.0, 3.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::from([(var("price"), 2.0)]),
            bindings: vec![],
        };

        assert!(
            matches!(classify_ceil_context(&problem), CeilContextResult::Ok),
            "price * ceil(x) with fixed_params[price]=2.0 must be Ok for Minimize"
        );
    }

    /// Negative fixed param coefficient must still be rejected.
    #[test]
    fn classify_ceil_context_negative_fixed_param_rejects() {
        use crate::optimize::{DecisionVariable, OptimizationProblem};
        use std::collections::HashMap;

        // objective = (-1.0) * ceil(x) via fixed param neg_price = -1.0
        let objective = Expr::product(vec![
            Expr::variable("neg_price"),
            Expr::ceil(Expr::variable("x")),
        ]);
        let problem = OptimizationProblem {
            objective,
            direction: crate::optimize::ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 2.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::from([(var("neg_price"), -1.0)]),
            bindings: vec![],
        };

        assert!(
            matches!(
                classify_ceil_context(&problem),
                CeilContextResult::Reject { .. }
            ),
            "neg_price * ceil(x) with fixed_params[neg_price]=-1.0 must be Reject for Minimize"
        );
    }

    // -------------------------------------------------------------------------
    // classify_ceil_context — zero-coefficient / free billing component (#37)
    // -------------------------------------------------------------------------

    /// `0 * ceil(x)` (literal zero coefficient) must be accepted: the ceil
    /// term is effectively absent from the objective.
    #[test]
    fn classify_ceil_context_zero_literal_coeff_accepted() {
        use crate::optimize::{DecisionVariable, OptimizationProblem};
        use std::collections::HashMap;

        // objective = 0.0 * ceil(x), direction = Minimize
        let objective = Expr::product(vec![Expr::constant(0.0), Expr::ceil(Expr::variable("x"))]);
        let problem = OptimizationProblem {
            objective,
            direction: crate::optimize::ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 2.0, 3.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::new(),
            bindings: vec![],
        };

        assert!(
            matches!(classify_ceil_context(&problem), CeilContextResult::Ok),
            "0 * ceil(x) must be Ok (zero-coefficient ceil is effectively absent)"
        );
    }

    /// `price * ceil(x)` where `price = 0` (fixed param) must be accepted.
    /// This models a "free" billing component that contributes nothing to cost.
    #[test]
    fn classify_ceil_context_zero_fixed_param_accepted() {
        use crate::optimize::{DecisionVariable, OptimizationProblem};
        use std::collections::HashMap;

        // objective = price * ceil(x), direction = Minimize
        // price is a fixed param set to 0.0 (free component)
        let objective = Expr::product(vec![
            Expr::variable("price"),
            Expr::ceil(Expr::variable("x")),
        ]);
        let problem = OptimizationProblem {
            objective,
            direction: crate::optimize::ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 2.0, 3.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::from([(var("price"), 0.0)]),
            bindings: vec![],
        };

        assert!(
            matches!(classify_ceil_context(&problem), CeilContextResult::Ok),
            "price * ceil(x) with fixed_params[price]=0.0 must be Ok (zero-priced free component)"
        );
    }

    /// `price * ceil(x)` where `price = 2.0` (non-zero fixed param) must still
    /// be accepted as before (positive coefficient, Minimize).
    #[test]
    fn classify_ceil_context_nonzero_fixed_param_still_accepted() {
        use crate::optimize::{DecisionVariable, OptimizationProblem};
        use std::collections::HashMap;

        let objective = Expr::product(vec![
            Expr::variable("price"),
            Expr::ceil(Expr::variable("x")),
        ]);
        let problem = OptimizationProblem {
            objective,
            direction: crate::optimize::ObjectiveDirection::Minimize,
            decision_variables: vec![DecisionVariable {
                name: var("x"),
                domain: vec![1.0, 2.0, 3.0],
            }],
            constraints: vec![],
            fixed_params: HashMap::from([(var("price"), 2.0)]),
            bindings: vec![],
        };

        assert!(
            matches!(classify_ceil_context(&problem), CeilContextResult::Ok),
            "price * ceil(x) with fixed_params[price]=2.0 must still be Ok for Minimize"
        );
    }
}
