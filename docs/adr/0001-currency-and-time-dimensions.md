# ADR 0001: Currency and Time Dimensions in Cost Expressions

**Status:** Proposed

## Context

`Expr` evaluates to a bare `f64`.  All cost values implicitly represent
monthly USD amounts.  There is no unit attached to the result, no currency
code, and no time-axis concept.  This works today because every service
implementation uses the same convention, but it creates hidden coupling.

### Current state

- All `f64` values are USD, monthly granularity — by convention only.
- `Expr::Linear`, `Expr::Tiered`, etc. carry no unit metadata.
- Reserved-Instance / Savings-Plan pricing (1-year / 3-year) is not modelled;
  callers manually convert to an equivalent monthly rate before building the
  expression.
- Time-of-day or day-of-week pricing (e.g. Spot, Fargate Spot) is not
  representable.

### Anticipated future requirements

1. **Multi-currency** — GCP costs are in USD but billed in local currency; some
   enterprise customers need EUR/JPY reporting.
2. **RI/SP annual discount** — expressing "1-year RI saves 30% vs on-demand"
   requires a time axis that spans months.
3. **Time-varying pricing** — Spot / Fargate Spot prices fluctuate; a
   worst-case or expected-value model would need time-bucket weights.

## Options

### Option A — Unit type inside `Expr`

Add `currency: CurrencyCode` and `period: BillingPeriod` fields to each
`Expr` variant (or wrap the whole tree in a typed envelope).

**Pros:** Units are checked at construction time; mismatches surface early.

**Cons:** Breaks every existing `Expr` builder; `#[non_exhaustive]` alone
does not prevent churn — all callers must be updated.  Combinatorial
explosion of unit variants as more dimensions are added.

### Option B — Metadata on the evaluation result

Keep `Expr` as a dimensionless `f64` tree.  Return `EvaluationResult`
instead of `f64`, carrying `{ value: f64, currency: CurrencyCode,
period: BillingPeriod }`.

**Pros:** Zero change to `Expr` AST or existing builders.  Units live
next to the value that needs them.

**Cons:** Metadata must be threaded through `evaluate()` and all callers;
arithmetic on two `EvaluationResult` values must reconcile units.

### Option C — Separate conversion layer

Keep `Expr` dimensionless.  Add a `CostNormalizer` that accepts an
`EvaluationResult` together with a `CostContext` (base currency, reporting
period, exchange rates, RI-discount table) and converts to the target unit.

**Pros:** `Expr` stays simple; conversion logic is testable in isolation;
existing callers are unaffected until they opt in.

**Cons:** Two-step evaluation; risk of callers skipping the normalizer and
reporting raw values.

## Recommendation

**Option C** is the recommended starting point.

Rationale:
- It requires no changes to `Expr` or the existing service implementations,
  keeping backward compatibility while `Expr` gains stability under
  `#[non_exhaustive]`.
- Unit conversion is a cross-cutting concern that belongs outside the
  expression engine; it can be introduced incrementally and adopted per
  service.
- If strong compile-time enforcement becomes necessary, Option A can be
  layered on top later — but only after the conversion layer has stabilised
  the required unit vocabulary.

## Consequences

- A `CostContext` struct and `CostNormalizer` trait will be introduced in a
  future PR; existing `evaluate()` signature is unchanged.
- Service implementations may continue returning monthly-USD `f64` values
  until they explicitly opt in to the normalizer.
- Exchange-rate data sourcing is deferred; an initial implementation may
  assume a fixed rate table or a no-op (USD identity) normalizer.
