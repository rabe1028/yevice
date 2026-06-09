//! Introspection layer for the `Expr` AST.
//!
//! Provides methods to extract variable sets and affine (linear) forms from a
//! cost expression. This is the foundation for future LP/MILP solvers that need
//! to query the structure of an expression without evaluating it.

use std::collections::{BTreeMap, BTreeSet};

use crate::expr::Expr;
use crate::types::VariableName;

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
}
