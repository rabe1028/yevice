# ADR 0002: LP/MIP Solver Backend

**Status:** Proposed

## Context

`EnumerationSolver` walks the Cartesian product of every decision variable's
discrete domain. It scales exponentially in the number of decision variables,
is capped at `MAX_COMBINATIONS = 1_000_000`, and even within that envelope
spends time enumerating points it could in principle skip. Issue #20 lands
domain pre-filtering for **single-variable** constraints, but multi-variable
constraints still drive the full product. With 50+ resources — each typically
contributing one or more decision variables — the product blows past the cap
in seconds, and pruning alone cannot rescue it.

A second pain point: `Expr::as_linear()` returns `None` for `Tiered`, `Ceil`,
`Max`, `Min`. These shapes are common (S3 / DynamoDB tiered pricing, Lambda
free-tier `Max(usage − allowance, 0)`, `Ceil` for billable units) so any
linear backend must accept a MILP encoding for them or refuse the problem.

## Motivation

- **Scale**: target ~50 resources × O(10) candidate values each.
- **Predictability**: a real optimizer terminates with a bound rather than
  silently truncating at `MAX_COMBINATIONS`.
- **Composability**: keep enumeration for small problems and tests; let
  larger problems opt into the heavier backend.

## Options

| Option | Pros | Cons |
|---|---|---|
| **HiGHS** (via `highs` crate or `good_lp`) | Best-in-class open LP/MIP, BSD-friendly, active upstream. | C++ dep; build complexity on Windows. |
| **CBC** (COIN-OR) | Mature, broad coverage. | Slow vs HiGHS; weaker MILP performance. |
| **`good_lp`** (Rust facade) | Pluggable backend (HiGHS / CBC / minilp / Cplex). | Limited expressiveness for MILP shaping. |
| **Hand-rolled MILP** | Zero external deps; full control. | Months of work; immature numerics. |

## Tiered / Ceil / Max / Min MILP formulations (sketch)

- `Tiered(tiers, var)` → piecewise-linear. Standard reformulation: introduce
  one continuous "fill" variable per tier `q_i ∈ [0, width_i]`, plus binary
  activators `z_i` with the SOS2-style chain `z_i ≥ z_{i+1}`,
  `q_i ≤ width_i · z_i`, `q_i ≥ width_i · z_{i+1}`, and the link
  `var = Σ q_i`. Objective contribution = `Σ price_i · q_i`.
- `Ceil(expr)` → integer variable `y` with `expr ≤ y ≤ expr + 1 − ε`.
- `Max(expr − k, 0)` (free-tier shape) → `m ≥ expr − k`, `m ≥ 0`,
  plus big-M to keep `m` tight: `m ≤ expr − k + M(1 − z)`, `m ≤ M · z`.
- `Min` is the dual; use the same trick with reversed signs.

These all rely on tight big-M values: the encoder must compute domain bounds
for each subexpression up-front so big-M doesn't bloat the relaxation.

## Recommendation

1. **Backend**: HiGHS via `good_lp` (`good_lp = { version = "*", features = ["highs"] }`).
   Best MILP performance with the least bespoke glue.
2. **Feature gate**: `--features lp` on `yevice-solver`. The default build
   stays dep-free (`EnumerationSolver` only).
3. **Factory wiring**: `solver_from_name("lp")` returns the new backend when
   the feature is on, otherwise `UnknownSolver { allowed: [...] }`.
4. **Encoder**: a new module `yevice_solver::milp` translating `Expr` →
   linear form, lowering `Tiered`/`Ceil`/`Max`/`Min` to auxiliary variables
   with the formulations above. Reject expressions it can't encode with an
   explicit error pointing at the offending sub-expression.

## Consequences

- Adds a non-trivial native dep; CI must cover Linux + macOS + Windows.
- `Expr::as_linear` keeps returning `None` for non-linear shapes; the MILP
  encoder grows its own dispatch. (Symmetric, no breaking change.)
- Enumeration stays the source of truth for golden tests — the MILP backend
  is validated against it on every problem small enough to enumerate.
- Future ADR will pick a strategy for **stochastic / non-convex** shapes
  (e.g. exchange-rate scenarios from ADR-0001) once the LP path is in.
