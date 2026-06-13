# ADR 0003: IaC Parse Failure Policy

**Status:** Accepted (2026-06-13). Refs #38.

## Context

The three IaC parsers disagree on what "failure" means today:

| Parser | Behaviour on unresolved / missing input |
|---|---|
| **CFN** | Aborts the whole template on a missing parameter (`CfnError::MissingParameters`); same for unresolved `Fn::ImportValue` / mapping lookups. |
| **TF** | Drops unresolved `var.*` / `local.*` / cross-resource refs with `tracing::warn!`; adapters fall back to defaults; an Architecture is still produced. Only HCL syntax errors abort. |
| **Wrangler** | Strict by construction — TOML/JSONC errors abort; no variable layer to be partial about. |

The asymmetry surprises users: a single missing parameter kills CFN
end-to-end, while a similar gap in TF produces a partially populated (and
quietly less accurate) cost model.

## Options

- **A. Strict everywhere** — promote TF warnings to errors. Predictable, but
  regresses today's "TF without tfvars" UX.
- **B. Lenient everywhere** — drop CFN's `MissingParameters` to a warning.
  Quietly wrong numbers are dangerous downstream.
- **C. Two-mode policy with a CLI flag** (recommended).

## Recommendation — Option C, default Lenient

Introduce `ParsePolicy { Lenient, Strict }` threaded through each parser's
public entry point and surfaced as `--strict` / `--lenient`, default
`--lenient`:

- **Lenient:** unresolved refs / missing params emit `tracing::warn!`,
  adapters use fallback defaults, parsing succeeds.
- **Strict:** any unresolved input is a hard error — matches today's CFN
  semantics; lifts TF and Wrangler to the same bar.

A shared `IacParseDiagnostic` list (in `yevice-core::io`) is returned next
to the Architecture so CI can fail on diagnostics even under Lenient.

Lenient by default keeps today's TF UX intact (largest current user base);
`--strict` reinstates CFN's hard-failure for users who relied on it.

## Decision — API shape

採用案は Recommendation 通り Option C / default Lenient。確定した API
形状を以下に固定する。

### Core types (`yevice-core::io`)

```rust
pub enum ParsePolicy { Lenient, Strict }   // default: Lenient

pub enum Severity { Error, Warning, Info }

pub enum DiagnosticSource { Cfn, Tf, Wrangler }

pub struct SourceLocation {
    pub file: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

pub struct IacParseDiagnostic {
    pub severity: Severity,
    pub source: DiagnosticSource,
    pub location: Option<SourceLocation>,
    pub code: String,              // stable ASCII snake_case identifier
                                   // (e.g. "missing_parameter", "unresolved_var_ref")
    pub message: String,
}
```

`code` は serde 経由で `cost_model.json` の `diagnostics: []` 配列から
deserialize される値であり、JSON 中の任意文字列を `&'static str` に詰めることは
できない (`'static` 寿命を満たすには `Box::leak` や intern が必要)。値は ASCII
snake_case の安定識別子で表現し、`{source}.{code}` の組み合わせで一意化する
(例: `source: Cfn` + `code: "missing_parameter"`)。

```rust

pub struct ParseOutcome<T> {
    pub value: T,
    pub diagnostics: Vec<IacParseDiagnostic>,
    pub had_errors: bool,           // any Severity::Error present
}
```

### Parser signatures

3 パーサとも返り値を `Result<ParseOutcome<T>, *Error>` に揃える。
構文/IO 等の **「✕ 行に該当する case-by-case error」は引き続き
`Err(*Error)` で返す**。policy で握り潰し可能な失敗のみ
`ParseOutcome.diagnostics` に積む。

### Engine 配管

```rust
pub struct GenerateRequest {
    /* existing fields */
    pub policy: ParsePolicy,        // default Lenient
}
pub fn generate_cost_model(req: &GenerateRequest)
    -> Result<ParseOutcome<ArchitectureCost>, EngineError>;
```

戻り値型を `ArchitectureCost` 単体から `ParseOutcome<ArchitectureCost>`
に変更 (破壊変更)。

### CLI

- トップレベル global flag `--strict` (boolean) のみ追加
- `--lenient` は **追加しない** (未指定は Lenient なので冗長)
- 終了コード:
  - Strict + error 級 diagnostic 1 件以上 → exit code 1
  - Lenient → exit code 0 + diagnostics は stderr に `tracing::warn!` で表示

### JSON 出力

`cost_model.json` のトップレベルに `diagnostics: []` 配列を追加。
スキーマの破壊変更としてメジャー扱い、CHANGELOG に明記する。

## Migration

1. Add `ParsePolicy` + `IacParseDiagnostic` to `yevice-core`.
2. Plumb through `yevice-engine::generate` and the CLI as `--strict`.
3. Convert CFN's hard errors into diagnostics; raise a new
   `CfnError::PolicyViolation` only when `Strict` is requested.

Steps 1–2 are non-breaking; step 3 is gated on a deprecation cycle for the
existing `MissingParameters` variant.

## Error Variant Coverage Matrix

The matrix below classifies every variant of the three IaC error enums
(post-#33) against `ParsePolicy`. Legend for **Policy対象**: 〇 = policy
should change behaviour, △ = boundary case, ✕ = policy-neutral (syntax /
IO / programmer error). Sources: `crates/iac/yevice-{cfn,tf,wrangler}/src/error.rs`.

### CFN — `CfnError` (11 variants)

| Variant | 発生原因 | Policy対象 | Lenient | Strict |
|---|---|---|---|---|
| `MissingParameters(String)` | ユーザー入力欠落 (Parameters block) | 〇 | demote to diagnostic | remain hard error |
| `ParameterNotFound(String)` | 未解決 `!Ref` parameter | 〇 | demote to diagnostic (fallback default) | remain hard error |
| `ImportValueNotFound(String)` | 未解決 `Fn::ImportValue` (cross-stack export 未提供) | 〇 | demote to diagnostic | remain hard error |
| `MappingNotFound { .. }` | `Fn::FindInMap` のキー欠落 | 〇 | demote to diagnostic | remain hard error |
| `ConditionNotFound(String)` | `Fn::If` で未定義 condition 参照 | △ | demote to diagnostic | remain hard error |
| `UnsupportedResourceType(String)` | アダプタ未対応の `AWS::*::*` | △ | demote to diagnostic (skip resource) | remain hard error |
| `MissingProperty { .. }` | アダプタが要求する必須プロパティ欠落 | △ | demote to diagnostic (skip resource) | remain hard error |
| `IntrinsicError(String)` | Intrinsic の構文/型違反 (引数列の型誤り、深さ超過 など) | ✕ | remain hard error | remain hard error |
| `ParseError(String)` | テンプレートの構造違反 (root 非 mapping、Resources 欠落、Type 欠落) | ✕ | remain hard error | remain hard error |
| `Yaml(serde_yaml_ng::Error)` | YAML 構文エラー | ✕ | remain hard error | remain hard error |
| `Io(std::io::Error)` | ファイル IO 失敗 (read_iac_file 経由含む) | ✕ | remain hard error | remain hard error |

`ConditionNotFound` を △ にした理由: 形式的には「未解決参照」だがテンプレ
作者の typo が大半で、lenient で握り潰すと条件分岐が黙って `false` 側に
倒れて誤った Architecture を生む懸念がある。`UnsupportedResourceType` /
`MissingProperty` も △ — どちらも現状コードベースで未使用（reserved for
future adapter work）であり、policy 設計より先にアダプタ層の責務分離が要る。

### TF — `TfError` (3 variants)

| Variant | 発生原因 | Policy対象 | Lenient | Strict |
|---|---|---|---|---|
| `MissingAttribute { .. }` | アダプタが要求する必須 attribute 欠落 (現状未使用、reserved) | △ | demote to diagnostic | remain hard error |
| `ParseError(String)` | HCL 構文エラー (`hcl::Error` from 経由含む) | ✕ | remain hard error | remain hard error |
| `Io(std::io::Error)` | ファイル IO 失敗 | ✕ | remain hard error | remain hard error |

TF の本命である「未定義 `var.*` / `local.*` 参照 (resolver が解決できず残った
シンボル)」は現状 `TfError` に届かず `tracing::warn!` で握り潰されている
(`resolver.rs` の variable warn + `convert.rs::tf_resource_to_raw` の attr drop)。
**Phase 1 でこれらを `TfError::UnresolvedSymbol { kind, name, location }`
として型化し、policy 制御下に置く。** Phase スコープ節を参照。

なお `TfValue::ResourceRef` (`resource.<type>.<name>.<attr>` 形式の
cross-resource 参照) は **正常に解決された参照** として `build_connections` が
topology edge (S3 bucket notification → Lambda, IAM role attachment 等) を
構築するのに使う。`tf_resource_to_raw` 内で `ResourceRef` がスカラー attr 値
としては JSON 化できず drop される (`tf_value_to_json` が `None` を返す) 経路は
ある (`convert.rs:114` のコメント参照) が、これは「未解決」ではなく
「dyn 表面 = JSON 化できない型」だからであり、診断対象外。`UnresolvedSymbol`
は **resolver が解決できなかった `var.*` / `local.*` のみ** が対象で、
cross-resource ref は対象外とする。

### Wrangler — `WranglerError` (2 variants)

| Variant | 発生原因 | Policy対象 | Lenient | Strict |
|---|---|---|---|---|
| `ParseError(String)` | TOML/JSONC 構文エラー (`toml::de::Error` / `serde_json::Error` from 経由) | ✕ | remain hard error | remain hard error |
| `Io(std::io::Error)` | ファイル IO 失敗 | ✕ | remain hard error | remain hard error |

Wrangler は変数層を持たず構造体に直接 deserialize するため、policy の影響
を受ける余地が現状ゼロ。`ParsePolicy` 引数はシグネチャ統一のために受け取る
が挙動は変わらない。

### Phase スコープ

- **Phase 1 (〇 + TF 型化):**
  - CFN の 4 バリアント (`MissingParameters` / `ParameterNotFound` /
    `ImportValueNotFound` / `MappingNotFound`) を `IacParseDiagnostic` 化。
  - **TF: `TfError::UnresolvedSymbol { kind: VariableOrLocal, name: String, location: Option<SourceLocation> }`
    バリアントを新設**し、現状 `tracing::warn!` で握り潰している
    **未定義の `var.*` / `local.*` 参照のみ** を型化。policy 制御下に置く。
    - `kind`: `VariableOrLocal::Variable` (未定義 `var.*`) / `VariableOrLocal::Local`
      (未解決 `local.*`) の二択。
    - `name`: 未定義の変数 / local 名 (例: `instance_type_var`)。
    - `TfValue::ResourceRef` (`resource.<type>.<name>.<attr>`) は **対象外**。
      resolver で解決済みの正常な参照であり `build_connections` が topology に
      使うため、これを `UnresolvedSymbol` 扱いにすると有効な TF テンプレートを
      reject してしまう。`tf_resource_to_raw` で `ResourceRef` がスカラー attr
      JSON 化に失敗して drop される経路も診断対象外 (型上 JSON 化できない
      ことに起因し、未解決ではない)。
    - type system 拡張として `TfError` に新バリアントを追加する作業を含む
      (詳細は次節)。
  - Wrangler は policy 引数受け取りのみで挙動は据え置き (現状で
    policy 対象バリアントなし)。
- **Phase 2 (△ の取り込み):** CFN の `ConditionNotFound` /
  `UnsupportedResourceType` / `MissingProperty` を診断化。TF の
  `MissingAttribute` も合わせて policy 制御下に置く
  (Phase 1 で `UnresolvedSymbol` が片付いているため、ここは残務)。
- **Phase 範囲外（恒久 hard error）:** すべての ✕ 行 — 構文エラー・IO・
  programmer error。これらは lenient でスキップすると下流で必ず破綻する。

### 判定根拠 (要約)

- **「未解決参照は demote 候補、構文エラーは hard error 据え置き」が原則。**
  前者は入力の不完全さで、後者は入力の壊れ。
- 〇 の 4 バリアント (CFN) は ADR 本文の Context 表で挙げた CFN 早期 abort
  の原因そのもの。これらを片付けることが「CFN だけ厳しい」非対称解消の中核。
- △ の 3 つ (CFN) は「未解決」に近いが副作用が大きい（条件分岐が黙って倒れる /
  unknown resource を黙ってスキップ）ため Phase 2 送り。
- TF の現状 `warn!` 経路 (resolver の variable-without-default warn と
  `tf_resource_to_raw` の attr drop warn) のうち、**未定義 `var.*` / `local.*`
  に起因するもののみ** を型に上げる。`ResourceRef` の JSON 化失敗経路は
  正常動作 (cross-resource ref は topology へ流す) であり対象外。policy
  制御下に置くには **新バリアント `TfError::UnresolvedSymbol` を Phase 1
  で導入する** 必要がある (型システム拡張)。これは CFN 側の diagnostic 化
  (既存バリアント変換) とは作業性質が異なるため Phase 1 の二本柱として明示。
- まとめると Phase 1 は **CFN 既存 4 バリアント → diagnostic 変換 + TF
  `UnresolvedSymbol` 新設 (var/local のみ、型拡張)** の二本立て。Phase 2 は
  △ バリアント (CFN の `ConditionNotFound` / `UnsupportedResourceType` /
  `MissingProperty` と TF の `MissingAttribute`) を追加で policy 制御下に
  取り込む。バリアント総数 16 に対し policy 対象は Phase 1 で 4 (CFN) +
  1 (TF 新設)、Phase 2 累計で 8 と限定的。

## Out of scope

Generalising `CfnPropertyValue` to an IaC-neutral `IacPropertyValue` is a
separate, larger refactor (it touches all three converters) and is **not**
addressed here. It is noted as a follow-up so this ADR's scope stays
focused on failure policy. Refs #21.
