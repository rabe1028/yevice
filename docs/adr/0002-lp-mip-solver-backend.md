# ADR 0002: LP/MIP Solver Backend

**Status:** Accepted (2026-06-13). Refs #37.

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
| **A. HiGHS via `good_lp`** | Pluggable backend layer, well-trodden path. | `good_lp` の表現力が MILP shaping (big-M / SOS2 / 自前 introspection) で足を引っ張る。 |
| **B. HiGHS directly via `highs` crate, no facade** | Full control over encoding; one less abstraction layer. | 他バックエンド (CBC 等) を後で足す時に独自 facade を書く必要。 |
| **C. 自前 `MilpBackend` trait + `highs` crate 初期実装** | trait 境界で encoder / solver を分離。CBC や別ソルバを将来差し替え可能。expr_introspect を共通化できる。 | facade 設計の手間。最初の実装は HiGHS のみ。 |
| **D. Hand-rolled MILP** | Zero external deps. | Months of work; immature numerics. 採用しない。 |

## Tiered / Ceil / Max / Min MILP formulations (sketch)

- `Tiered(tiers, var)` → piecewise-linear. **Incremental (fill) formulation**:
  introduce one continuous "fill" variable per tier `q_i ∈ [0, width_i]`,
  plus binary activators `z_i` with the chain `z_i ≥ z_{i+1}`,
  `q_i ≤ width_i · z_i`, `q_i ≥ width_i · z_{i+1}`, and the link
  `var = Σ q_i`. Objective contribution = `Σ price_i · q_i`.
  SOS2 を直接利用せず線形不等式のみで表現する (CBC / 他バックエンド互換)。
- `Ceil(expr)` → 整数変数 `y` で `expr ≤ y`、`y is integer` (下限のみ + 整数性)。
  上限制約は置かず、目的関数の minimization と `y` の正係数によって自動的に
  `y* = ceil(expr)` を tight にする。詳細な選択理由と前提条件、撤回した代替案の
  経緯は下記「Ceil 定式化の選択」を参照。
- `Max(expr − k, 0)` (free-tier shape) → `m ≥ expr − k`, `m ≥ 0`,
  plus big-M to keep `m` tight: `m ≤ expr − k + M(1 − z)`, `m ≤ M · z`.
- `Min` is the dual; use the same trick with reversed signs.

big-M 値は `expr_introspect::expr_bounds(e, params, ranges)` で自動推定する。
無限値が出る場合は `SolverError::UnboundedExpression` で reject。

## Decision

**Option C — 自前 `MilpBackend` trait + `highs` クレートで初期実装。**

### Ceil 定式化の選択

**採用: 案 Z (Lower bound only + integer)** — `expr ≤ y`, `y ∈ ℤ` のみ。
上限制約は置かない。

#### 撤回した代替案

- **案 A (`expr ≤ y ≤ expr + 1`)**: `expr` が整数値のとき `y = expr` と
  `y = expr + 1` の双方が feasible になり、本来 `Ceil(expr) = expr` であるべき
  ケースで 1 単位過剰な値を許してしまう。**不採用**。
- **案 A' (`expr ≤ y ≤ expr + 1 - ε`, `ε = 1e-5`)**: 前 ADR 改訂で採用したが、
  `expr` の小数部が `ε` 未満のとき `ceil(expr)` が右端制約を満たさず infeasible
  になる退化を起こす。例: `expr = 1.000001`, `ε = 1e-5` のとき正しい `y = 2` が
  `y ≤ 1.99999` を破り reject される。整数近傍の解析的に正しい解を solver が
  誤って捨てるため **撤回**。
- **案 C (補助変数 `z` で `y = expr + z`, `0 ≤ z < 1`)**: 汎用的に正しいが、
  変数と制約が 1 本ずつ増え、`z < 1` の strict inequality を MILP で扱うため
  さらに `ε` が必要。複雑度に対するリターンが現状の使用パターンでは合わない。
  将来の汎用拡張時の選択肢として保留。

#### 採用案 (Z) の前提条件

`y* = ceil(expr)` が LP relaxation で自動的に tight になるのは、`y` がモデル中の
**すべての出現箇所で "tightness を強制する向き" に作用する** ときのみである。
`expr ≤ y` 単独では `y` の上限がないので、`y` を大きく取って制約を満たせる
"favorable" な向きで `y` が現れると `y = ceil(expr)` を選ぶ動機がなくなり、解が
不確定になる。具体的な許可文脈と reject 文脈を以下に列挙する。

##### 許可される使用文脈 (tight 方向)

- **Objective**: minimize 問題で `y` が **正係数** で目的関数に出現
- **`Le` 制約の LHS に正係数で出現**: `y + ... ≤ rhs`
  → `y` を増やすほど LHS が増えて違反方向に動くので、自動 tight
- **`Ge` 制約の LHS に負係数で出現**: `... - y ≥ rhs`
  → `y` を増やすほど LHS が減って違反方向に動くので、自動 tight
- **`expr` が定数項のみ (decision var を含まない)** は常に OK — `evaluate`
  段階で定数化され、MILP encoder には到達しない

##### Reject される使用文脈 (anti-tight 方向、`UnsupportedCeilContext`)

- maximize 問題かつ Ceil が objective に含まれる
- objective 内で `y` の係数が **負**
- 任意の `Eq` 制約に `y` が出現 (等式は tight 方向が両側で打ち消し合う)
- **`Ge` 制約の LHS に `y` が正係数で出現**: `y + ... ≥ rhs`
  → `y` を大きく取れば必ず満たせるので favorable、`y* = ceil(expr)` を選ぶ
  動機が消える。**指摘 6 で新規追加した reject 条件。**
- **`Le` 制約の LHS に `y` が負係数で出現**: `... - y ≤ rhs`
  → 同じく `y` を大きく取れば必ず満たせるので favorable、reject。

#### 実装時の前提検出義務

solve 冒頭で `expr_introspect::classify_ceil_context(model)` を走らせ、各 Ceil
について以下を判定する:

1. 出現箇所 (objective / constraint index)
2. 係数符号 (正 / 負)
3. 制約 sense (Le / Ge / Eq) と objective sense (Min / Max)
4. (出現箇所 × 係数符号 × sense) の組み合わせが上記「許可される使用文脈」に
   全件マッチするか

すべての出現が許可文脈なら通す。1 つでも reject 文脈にマッチすれば
`SolverError::UnsupportedCeilContext { expr: String, reason: &'static str }` を
返す。`reason` は具体的な anti-tight 方向を示す静的文字列 (例:
`"ceil appears with positive coefficient in Ge constraint LHS"`)。

#### 現状コードベースでの適合性

ベース調査時点 (`grep -rn "Expr::ceil" crates/`) で `Expr::ceil(...)` の出現箇所:

| 出現箇所 | コンテキスト | sense | LHS 係数 | tight 方向 |
|---|---|---|---|---|
| `bindings.rs:121` | `VariableBinding.expr` (user-defined) | — | — | binding は evaluate で前向き計算、MILP には RHS が直接渡らない |
| `connection_rules.rs:82` | `VariableBinding.expr` (auto, EventSource) | — | — | 同上 |
| `kinesis.rs:183` | `Constraint.required` (`shard_throughput`) | Le (`required ≤ limit`) | +1 (正) | **許可** (Le LHS, 正係数) |
| `kinesis.rs:195` | `Constraint.required` (`shard_record_rate`) | Le (`required ≤ limit`) | +1 (正) | **許可** (Le LHS, 正係数) |

`Constraint` の意味論は `capacity.rs:126` の `required > constraint.limit` (= Le
制約) で、Ceil は LHS に正係数で現れる auto-tight 文脈である。さらに `Expr::ceil`
の **内側の変数はすべて usage 変数** (`peak_ingestion_mb_per_sec`,
`peak_records_per_sec`, `source_var`) で decision variable を含まないため、
`evaluate` 段階で定数化される (MILP encoder は通らない)。`ResourceCost.expr` /
`CostComponent.expr` 内に Ceil は **ゼロ件**。

結論として現状コードベースでは:

1. すべての Ceil が許可文脈 (Le LHS 正係数 または decision var を含まない定数式)
2. `UnsupportedCeilContext` reject は本番影響ゼロ件
3. ガード自体は将来 Ceil が decision variable を含む文脈で誤用された場合の
   safety net として機能する

#### 将来の拡張

Ceil が decision variable を含み、かつ maximize / 負係数 / constraint 位置で
self-tight 不可能なケースが必要になった場合:

- 案 C (補助変数 `y = expr + z`, `0 ≤ z ≤ 1 - δ`) を別 Issue で実装
- または big-M ベースの両端定式化 (整数性とのギャップを別変数で吸収)

これらは追加の変数 / 制約 / tolerance パラメータを伴うため、本 ADR の射程外
とし、需要が顕在化した時点で別 ADR で評価する。

### Expr カバレッジ

- 線形化可能な shape のみサポート。
- `var * var` / `var / var` (decision var 同士の積/商) は
  `SolverError::Nonlinear { expr: String }` で reject。
- **本番リスク評価:** リポジトリ全体で `var * var` / `var / var` は
  実装ゼロ件 (`expr_introspect` で確認済み)。reject 戦略で十分。

### 検出タイミング

solve 冒頭で **プリチェック**。新設関数:

```rust
pub fn expr_is_linearizable(expr: &Expr, decision_vars: &[VarId]) -> Result<(), SolverError>;
```

`yevice-solver::expr_introspect` に追加。MILP 問題構築前に走らせて、
非線形 shape を含む式を早期 reject する。

### 離散 decision variable の扱い

各 candidate value に binary indicator `z_i` を割り当て、`Σz_i = 1` 制約で
正確に 1 つ選択。連続値の decision var は使わない (現状の domain モデルが
discrete のため)。

### Feature gate

- `Cargo.toml` の `default = []` (デフォルト無効)
- `--features highs` で有効化、`MilpBackend` impl が登場
- enumeration は常に有効

### CLI flags

- `--solver enumeration|highs` (デフォルト `enumeration`)
- HiGHS 指定時のみ有効化:
  - `--time-limit <seconds>`
  - `--mip-gap <ratio>`
  - `--threads <n>`

### enumeration へのフォールバック

**自動フォールバックなし。** `--solver highs` 指定下で reject が起きたら
エラーメッセージで `--solver enumeration` の利用を案内するに留める。
silent fallback はベンチマーク再現性と user surprise の観点で禁止。

### Property test

`proptest` で **domain size ≤ 50 / constraints ≤ 5** の小問題を生成し、
両ソルバーの最適値が次の許容範囲に収まることを確認:

```
|cost_enum - cost_milp| ≤ max(abs_tol, rel_tol * |cost_enum|)
```

具体値は `abs_tol = 1e-6`, `rel_tol = 1e-9` を初期値とする (HiGHS のデフォ
ルト feasibility tolerance に揃える)。

### CI

`highs-sys` の **vendored ビルド可能性を着手時に調査**:

- vendored build が全 OS で通る → CI 標準で `--features highs` 有効化
- vendored が macOS/Windows で通らない → Linux 限定 feature とし、
  非 Linux ビルドでは `MilpBackend` impl をコンパイル対象外にする

## Consequences

- `yevice-solver` に `MilpBackend` trait と `HighsBackend` impl を追加。
- `expr_introspect` に `expr_is_linearizable` / `expr_bounds` /
  `classify_ceil_context` を新設。
- `SolverError` に `UnsupportedCeilContext { expr, reason }` を追加し、
  Ceil 採用案 (Z) の前提条件を破る入力を早期 reject する。
- CLI に solver 選択フラグ + HiGHS チューニングフラグを追加。
- `proptest` ベースの cross-solver 検証スイート追加。
- Enumeration はテスト golden / 小問題の source of truth として残る。
- 将来の CBC / Cplex 等は同じ `MilpBackend` trait の別 impl として追加可能。
- 将来 ADR で stochastic / non-convex shapes (ADR-0001 の為替シナリオ等)
  を扱う。
