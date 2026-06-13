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
- `Ceil(expr)` → 整数変数 `y` で `expr ≤ y ≤ expr + 1 - ε`。
  `ε = 1e-5` を採用 (HiGHS のデフォルト integer feasibility tolerance
  `1e-6` より一桁緩く取り、丸め誤差で右端が誤って feasible になるのを防ぐ)。
  ε を引かない `expr ≤ y ≤ expr + 1` の素朴な定式化だと、`expr` が整数値の
  ときに `y = expr` と `y = expr + 1` の双方が feasible になり、`Ceil(expr) = expr`
  であるべきケースで 1 単位過剰な値を許してしまう。minimization で y に正の係数が
  ついていれば自動的に tight 側へ落ちるが、maximize / 制約として現れる場合や、
  目的関数で別変数と打ち消し合うケースで誤った最適値を返す。`ε` 方式は両端閉の
  自然さを保ったまま整数 expr で `y = expr + 1` を排除でき、案 B (`expr ≤ y`
  のみ + 前提条件) より汎用的、案 C (補助変数 z 経由) より定式化がシンプルなため
  採用する。
- `Max(expr − k, 0)` (free-tier shape) → `m ≥ expr − k`, `m ≥ 0`,
  plus big-M to keep `m` tight: `m ≤ expr − k + M(1 − z)`, `m ≤ M · z`.
- `Min` is the dual; use the same trick with reversed signs.

big-M 値は `expr_introspect::expr_bounds(e, params, ranges)` で自動推定する。
無限値が出る場合は `SolverError::UnboundedExpression` で reject。

## Decision

**Option C — 自前 `MilpBackend` trait + `highs` クレートで初期実装。**

### Ceil 定式化の選択

上の sketch に挙げた `expr ≤ y ≤ expr + 1 - ε` (案 A, `ε = 1e-5`) を採用する。
理由: 案 B (`expr ≤ y` のみ) は minimization × 正係数の前提でしか正しくならず
maximize / 制約 / 打ち消し項のあるケースで誤った結果を返す。案 C (補助変数 z で
`y = expr + z`, `0 ≤ z < 1`) は安全だが変数と制約が 1 本ずつ増える。案 A は単一
不等式で両端閉のまま整数 expr を正しく扱え、汎用性とシンプルさのバランスが
最も良い。

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
- `expr_introspect` に `expr_is_linearizable` / `expr_bounds` を新設。
- CLI に solver 選択フラグ + HiGHS チューニングフラグを追加。
- `proptest` ベースの cross-solver 検証スイート追加。
- Enumeration はテスト golden / 小問題の source of truth として残る。
- 将来の CBC / Cplex 等は同じ `MilpBackend` trait の別 impl として追加可能。
- 将来 ADR で stochastic / non-convex shapes (ADR-0001 の為替シナリオ等)
  を扱う。
