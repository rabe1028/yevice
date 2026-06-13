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
  `Money { value: f64, currency: String, period: BillingPeriod }`
  に erase。複数 SKU を集約する時点で型レベルの通貨情報は失われるが、
  値レベルでは保持される。erase は `Currency<f64, C>.erase()` 内で
  `C::CODE.to_string()` を呼び `&'static str` → `String` に変換する。
- **Layer 3 (ResourceCost → Architecture):** `BTreeMap<String, f64>`
  (`totals_by_currency`) に集約。`Architecture` 全体ではもはや通貨は
  単一とは限らないため、通貨別合計を辞書で持つ。

#### `&'static str` を使わない理由

phantom type の marker (`USD` / `EUR` / `JPY`) と `trait CurrencyCode { const
CODE: &'static str; }` は **コンパイル時定数のリテラル** なので `&'static str`
のままで問題ない (zero-cost)。一方で erase 後の `Money` / `ResourceCost` /
`CostComponent` / `ArchitectureResult` は **serde で cost_model.json から
deserialize される値** であり、JSON 中の任意文字列を `&'static str` に詰めることは
できない (`'static` 寿命を満たすには `Box::leak` や intern が必要で、
deserialize 経路に副作用が乗る)。したがって型レベルで通貨を持つ Layer 1 は
`&'static str`、値レベル (Layer 2 以降) は `String` で表現する非対称設計を採る。

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

評価結果側の `ArchitectureResult.total_monthly_cost: f64` は **完全置換** —
`totals_by_currency: BTreeMap<String, f64>` + `display_total: Option<Money>`
に置き換える。互換 alias は持たない。`ResourceResult.monthly_cost: f64` も同様に
`Money` に置き換える。メジャーリリース扱いとして CHANGELOG に明記する。

注: `ResourceCost` (cost_model.json の schema 型) には `total_monthly_cost`
フィールドは存在しない (実体は `expr: Expr` を保持するのみ)。したがって
「completion 置換」の対象は evaluation 結果型側であり、cost_model.json schema
側の通貨メタデータの持たせ方は別途以下で決定する。

### cost_model.json schema 側の通貨メタデータ

`cost_model.json` は `ArchitectureCost` をシリアライズしたもので、現状
`CostComponent { name, expr }` / `ResourceCost { logical_id, resource_type,
label, expr, components, required_variables }` を含む。Expr 自体は dimensionless
のまま据え置くため、通貨メタデータをどこに持たせるかを別途決める必要がある。
3 案を検討:

- **案 (a)**: `ResourceCost` / `CostComponent` には通貨フィールドを持たせず、
  `PricingMetadata` 経由で評価時に通貨を再ルックアップする。cost_model.json を
  別環境で評価する場合は PricingMetadata も同梱する必要がある。
  - Pros: schema 変更なし。
  - Cons: cost_model.json が self-contained でなくなる。SKU/region/effective-date
    の組から PricingMetadata を再構築する負担が評価側に残る。
- **案 (b)**: `CostComponent` のみに `currency: String` を追加。
  - Pros: SKU 粒度に最も近い。
  - Cons: `ResourceCost.components` が空 (`vec![]`) のサービスでは通貨情報が
    どこにも乗らない。`evaluate_architecture` の現実装は `components` が空または
    部分評価失敗時に `ResourceCost.expr` を直接評価する経路を持つため、現状
    AWS service 実装 57 件中 **9 件以上** が `components: vec![]` で完結しており、
    主要パスで通貨が失われる。
- **案 (b+, 採用)**: `ResourceCost` と `CostComponent` の **二段持ち** で
  リソース全体のデフォルト通貨 + コンポーネント単位のオーバーライドを表現。
  ```rust
  pub struct ResourceCost {
      // ... 既存フィールド ...
      pub currency: Option<String>,   // 追加: リソース全体のデフォルト
  }
  pub struct CostComponent {
      pub name: String,
      pub expr: Expr,
      pub currency: Option<String>,   // 追加: コンポーネント単位の override
  }
  ```
  serde で cost_model.json から deserialize されるため `&'static str` ではなく
  `String` を使用 (任意文字列を `'static` 寿命に詰められないため)。
  - Pros: `components` 空のサービスでも `ResourceCost.currency` で通貨を保持できる。
    コンポーネント単位で混在する場合 (例: AWS region SKU + サードパーティ SKU)
    は `CostComponent.currency` で override 可能。cost_model.json は依然
    self-contained。
  - Cons: フィールドが 2 段になり優先順位ルールが必要。
- **案 (c)**: `ResourceCost` 全体に 1 つの `currency` フィールド (混在不可)。
  - Pros: フィールド数最小。
  - Cons: リソース内 SKU 混在通貨を扱えない。

**採用: 案 (b+)**。理由: 案 (b) 単独では `evaluate_architecture` の
`components` 空経路で通貨情報が完全に失われる (実装調査で 9 件以上の AWS
service が該当)。`ResourceCost` レベルでもデフォルト通貨を持つことで全経路で
通貨を保持しつつ、コンポーネント単位の混在も `Option` の override で表現できる。

#### 優先順位ルール

`ArchitectureResult.totals_by_currency` への積み上げ時、評価する各値の通貨は
以下の順で解決する:

1. `CostComponent.currency` が `Some` ならそれを使用
2. それが `None` なら親 `ResourceCost.currency` を使用
3. それも `None` なら評価時に `PricingMetadata` 経由でルックアップ (fallback)
4. PricingMetadata でも解決できない場合は migration 期間の暫定処理として
   `"USD"` 扱い (warn ログを出して可視化)

`ArchitectureCost` レベルでは通貨を保持しない。複数リソースで混在し得るため、
集約は `BTreeMap<String, f64>` で個別管理する (Layer 3 の erase 仕様と整合)。

#### 後方互換 (migration period)

- 既存 cost_model.json は `currency` フィールドなしで serialize されていた。
  `#[serde(default)]` で `None` に deserialize し、評価時に上記 fallback
  (USD + warn) を適用する。
- 新規生成される cost_model.json は SKU lookup 時に `ResourceCost.currency`
  へ通貨を必ず焼き込む。`CostComponent.currency` は混在が起きるケースだけ
  `Some` で埋める。
- migration 期間終了 (全 cost_model.json 再生成後) に `Option` を外して
  `String` (必須) に絞り込むかは別 Issue で判断。

### 主要な配線変更

- `PricingMetadata.currency` を Layer 1 の SKU lookup 側で実際に読み取り、
  `Currency<f64, C>` の型パラメータ決定に使う。現状は metadata に存在する
  が読まれていない。

## Consequences

- `yevice-pricing` に `Currency<T, C>` / `Money` / `BillingPeriod` /
  `ExchangeRates` を新設。
- `PriceCatalog` 系トレイトの返り値は `Currency<f64, C>` に変わる
  (provider crate 側で `C` を確定)。
- 評価結果型の破壊変更: `ArchitectureResult.total_monthly_cost: f64` →
  `totals_by_currency: BTreeMap<String, f64>` + `display_total: Option<Money>`、
  `ResourceResult.monthly_cost: f64` → `Money`。
- cost_model.json schema の拡張: `ResourceCost` と `CostComponent` の双方に
  `currency: Option<String>` を追加 (案 b+)。`#[serde(default)]` により既存
  cost_model.json は再生成なしで deserialize 可能 (None → 評価時に USD
  fallback + warn)。優先順位は `CostComponent.currency` > `ResourceCost.currency`
  > `PricingMetadata` ルックアップ > USD fallback。
- 通貨型表現の非対称設計: phantom marker (`USD`/`EUR`/`JPY`) と
  `CurrencyCode::CODE` は `&'static str` のリテラル (zero-cost)、erase 後の
  `Money` / `*.currency` フィールド / `totals_by_currency` キーは `String`
  (serde で任意文字列を deserialize するため)。
- CLI に `--display-currency` フラグを追加、未指定混在時の警告を実装。
- f64 → Decimal、Hourly/Yearly 変換は本 ADR の射程外。
- Option A 全面適用 (Expr AST まで型化) は採らない。AST を dimensionless
  に保つことで MILP encoder (ADR-0002) との結合度を下げる効果を維持する。
