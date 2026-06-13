# ADR 0001: Currency and Time Dimensions in Cost Expressions

**Status:** Accepted (2026-06-13). Refs #36.

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

## Decision

**Option B 変形 — Phantom type at SKU→component, erase at component→architecture.**

`Expr` 自体は dimensionless のまま据え置き、評価結果に通貨/期間メタデータを
伴わせる方針 (Option B) を採るが、Layer によって表現を切り替える:

### Phantom type の適用範囲

- **Layer 1 (SKU lookup → CostComponent):** `Currency<f64, C: CurrencyCode>`
  を保持。`PricingMetadata.currency` を実際に読んで型パラメータとして引き
  回し、SKU の通貨ミスマッチをコンパイル時に検出する。
- **Layer 2 (CostComponent → ResourceCost):** 境界で
  `Money { value: f64, currency: &'static str, period: BillingPeriod }`
  に erase。複数 SKU を集約する時点で型レベルの通貨情報は失われるが、
  値レベルでは保持される。
- **Layer 3 (ResourceCost → Architecture):** `BTreeMap<&'static str, f64>`
  (`totals_by_currency`) に集約。`Architecture` 全体ではもはや通貨は
  単一とは限らないため、通貨別合計を辞書で持つ。

### 通貨型の表現

Marker struct + trait:

```rust
pub trait CurrencyCode { const CODE: &'static str; }
pub struct USD; impl CurrencyCode for USD { const CODE: &'static str = "USD"; }
pub struct EUR; impl CurrencyCode for EUR { const CODE: &'static str = "EUR"; }
pub struct JPY; impl CurrencyCode for JPY { const CODE: &'static str = "JPY"; }
```

### `BillingPeriod`

```rust
pub enum BillingPeriod { Monthly, Hourly, Yearly, /* reserved */ }
```

Phase 1 では `Monthly` のみ実装。他バリアントは型として予約し、cross-period
変換 (Hourly→Monthly 等) は別 Issue で対応する。

### CLI フラグ

`--display-currency <CODE>`:

| ケース | 挙動 |
|---|---|
| 未指定 + 全通貨同一 | そのまま表示 |
| 未指定 + 混在 | `tracing::warn!` + 通貨別内訳のみ表示 (synthetic total なし) |
| 指定あり + rate 完備 | 指定通貨に換算して表示 |
| 指定あり + rate 欠落 | hard error (`FxError::MissingRate`) |

### `ExchangeRates` trait

最初から日付パラメータを焼く:

```rust
pub trait ExchangeRates {
    fn rate(&self, from: &str, to: &str, at: RateDate) -> Result<Rate, FxError>;
}
pub enum RateDate { Monthly(YearMonth), Spot(DateTime<Utc>) }
```

monthly estimate (請求月平均) と spot rate (リアルタイム) を別概念として
シグネチャに表現する。Phase 1 の実装は `Monthly` のみで OK。

### Serialize/Deserialize

`Currency<f64, C>` の deserialize 時に通貨コードが不一致なら
`serde::de::Error::custom(...)` で Result 返却。**panic は禁止**。

### スコープ外

- **`f64` → `Decimal` 化**: 別 Issue (#TBD) として保留。誤差解析と
  HiGHS encoder への影響範囲が別問題のため。
- **Hourly/Yearly の cross-period 変換ロジック**: 別 Issue。

### 破壊変更

既存の `ResourceCost.total_monthly_cost: f64` は **完全置換**。互換 alias
は持たない。メジャーリリース扱いとして CHANGELOG に明記する。

### 主要な配線変更

- `PricingMetadata.currency` を Layer 1 の SKU lookup 側で実際に読み取り、
  `Currency<f64, C>` の型パラメータ決定に使う。現状は metadata に存在する
  が読まれていない。

## Consequences

- `yevice-pricing` に `Currency<T, C>` / `Money` / `BillingPeriod` /
  `ExchangeRates` を新設。
- `PriceCatalog` 系トレイトの返り値は `Currency<f64, C>` に変わる
  (provider crate 側で `C` を確定)。
- `Architecture` のシリアライズ JSON スキーマが破壊変更
  (`total_monthly_cost: f64` → `totals_by_currency: { "USD": ... }`).
- CLI に `--display-currency` フラグを追加、未指定混在時の警告を実装。
- f64 → Decimal、Hourly/Yearly 変換は本 ADR の射程外。
- Option A 全面適用 (Expr AST まで型化) は採らない。AST を dimensionless
  に保つことで MILP encoder (ADR-0002) との結合度を下げる効果を維持する。
