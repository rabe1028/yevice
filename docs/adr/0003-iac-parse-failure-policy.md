# ADR 0003: IaC Parse Failure Policy

**Status:** Proposed

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

TF の本命である「未解決 `var.*` / `local.*` / cross-resource ref」は **そもそも
`TfError` に届かず `tracing::warn!` で握り潰されている** (parser/adapter 層)。
Strict 化はこれらを `TfError` の新バリアント (例: `UnresolvedReference`) に昇格
させる作業を伴う。Phase 1 のスコープに含めるかは Migration 節で別途検討。

### Wrangler — `WranglerError` (2 variants)

| Variant | 発生原因 | Policy対象 | Lenient | Strict |
|---|---|---|---|---|
| `ParseError(String)` | TOML/JSONC 構文エラー (`toml::de::Error` / `serde_json::Error` from 経由) | ✕ | remain hard error | remain hard error |
| `Io(std::io::Error)` | ファイル IO 失敗 | ✕ | remain hard error | remain hard error |

Wrangler は変数層を持たず構造体に直接 deserialize するため、policy の影響
を受ける余地が現状ゼロ。`ParsePolicy` 引数はシグネチャ統一のために受け取る
が挙動は変わらない。

### Phase スコープ

- **Phase 1 (〇 のみ実装):** CFN の 4 バリアント
  (`MissingParameters` / `ParameterNotFound` / `ImportValueNotFound` /
  `MappingNotFound`) を `IacParseDiagnostic` 化。TF/Wrangler は policy
  引数受け取りのみで挙動は据え置き。
- **Phase 2 (△ の取り込み):** CFN の `ConditionNotFound` /
  `UnsupportedResourceType` / `MissingProperty` を診断化。同時に TF の
  warn! 経路を `TfError::UnresolvedReference` (新規) に置き換え、
  `MissingAttribute` と合わせて policy 制御下に置く。
- **Phase 範囲外（恒久 hard error）:** すべての ✕ 行 — 構文エラー・IO・
  programmer error。これらは lenient でスキップすると下流で必ず破綻する。

### 判定根拠 (要約)

- **「未解決参照は demote 候補、構文エラーは hard error 据え置き」が原則。**
  前者は入力の不完全さで、後者は入力の壊れ。
- 〇 の 4 バリアントは ADR 本文の Context 表で挙げた CFN 早期 abort の原因
  そのもの。これらだけ片付ければ「CFN だけ厳しい」非対称は実質解消する。
- △ の 3 つは「未解決」に近いが副作用が大きい（条件分岐が黙って倒れる /
  unknown resource を黙ってスキップ）ため Phase 2 送り。
- TF の `warn!` 経路を「すでに lenient 相当」と見なせば、Phase 1 の追加実装
  は CFN 側のみで済み、対象バリアントは合計 4 で打ち止め。バリアント総数
  16 に対し policy 対象が 4 (Phase 1) / 7 (Phase 2 累計) と限定的なため、
  enum 拡張ではなく既存 4 バリアント → diagnostic への変換ロジックで完結
  する見込み。

## Out of scope

Generalising `CfnPropertyValue` to an IaC-neutral `IacPropertyValue` is a
separate, larger refactor (it touches all three converters) and is **not**
addressed here. It is noted as a follow-up so this ADR's scope stays
focused on failure policy. Refs #21.
