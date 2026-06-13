# ADR-0005: IaC Adapter Implementation Pattern

**Status:** Accepted (2026-06-13). Refs #39.

## Context

`yevice` は現状 3 つの IaC 形式 (CloudFormation / Terraform / Wrangler) を
取り込むが、コード構造が揃っていない。

| 形式 | 参照モデル | 実装パターン |
|---|---|---|
| CFN | `!Ref` / `!GetAtt` / `Fn::ImportValue` / `Fn::FindInMap` 等の構造化 intrinsic | resource type ごとの `CfnAdapter` trait |
| TF | `var.*` / `local.*` / `aws_*.name.attr` の構造化参照 | resource type ごとの `TfAdapter` trait |
| Wrangler | `bindings.bucket_name = "my-bucket"` のような string ID 直書きのみ | trait なし、`WranglerConfig` から直接 `Architecture` を構築 |

この非対称は「Wrangler に adapter trait を足すべきか?」という疑問を生むが、
**現時点では正しい設計**である。本 ADR でその判断軸を明文化する。

## Decision

**「リソース参照モデルの複雑さに応じて adapter trait pattern の採用を
判断する」** を原則とする。

### Adapter trait が必須となる条件

次のいずれかを満たす形式は adapter trait pattern を採用する:

1. 構造化された intrinsic / reference 構文を持つ (`!Ref`, `!GetAtt`,
   `var.*`, `resource.name.attr` 等)
2. リソース間の依存解決にスコープ・名前空間の解決層が必要
3. リソース種別ごとに property → cost-model フィールドのマッピングが
   非自明 (e.g. EC2 instance type → vCPU / memory のテーブル参照)

CFN / TF はいずれも該当する。

### Adapter trait を採用しない条件

string ID 参照のみで完結し、property がほぼフラットな key-value の形式は
直接 `Architecture` を構築する。Wrangler が該当する。

trait を導入してもボイラープレートが増えるだけで抽象化の利益がない。

### 共通化されているもの (型レベル)

`IacPropertyValue` 型は **全 IaC 形式で共有する**:

- CFN の `CfnPropertyValue` (#33 で導入された typed property value) を
  source として、IaC-neutral な `IacPropertyValue` に汎化する (Issue #39)。
- TF / Wrangler の property 表現もこの型に揃える。
- 結果として cross-IaC コードが property value を扱う際に共通の型で
  reasoning できる。

### 共通化しないもの (コードパターン)

adapter trait の採否は形式の構造で決まる。Wrangler に「将来のために」
trait を生やすことはしない。

## Migration trigger for Wrangler

Wrangler が将来 adapter trait pattern に移行する閾値:

- **string ID のみで表現困難な参照が登場した時点**
- 具体例:
  - Worker → R2 bucket / KV namespace への参照を環境変数経由で動的に
    解決する仕様が Wrangler 側に入る
  - `service_bindings` で別 Worker を name 参照する際に、その Worker の
    deployment 出力属性 (URL 等) を再利用する仕組み
  - 複数 Worker 間で settings を共有する `[shared]` ブロック等

これらが追加されたら adapter trait を導入し、現状の直接構築コードを
trait 配下に移す。

## Checklist for adding a 4th+ IaC format

新しい IaC 形式 (例: Pulumi / SAM / Bicep) を追加するときの判定手順:

1. **参照モデルの確認** — 構造化 intrinsic を持つか? string ID のみか?
2. **adapter trait の要否判定** — 上記 Decision 節の基準を適用。
3. **`IacPropertyValue` の対応確認** — 既存型でカバーできるか、拡張が要るか。
4. **Error enum の設計** — ADR-0003 の error variant matrix に新形式の
   行を追加。policy 対象バリアントを 〇/△/✕ で分類。
5. **CLI 統合** — `--format <name>` 選択肢追加、auto-detect ロジック更新。
6. **Test fixture** — 既存 fixture のディレクトリ規約に合わせる
   (`crates/iac/yevice-<format>/tests/fixtures/`).

## Consequences

- Wrangler は当面 trait なしのまま据え置き。trait 化の作業は移行 trigger
  まで発生しない。
- `IacPropertyValue` 化 (Issue #39) は CFN/TF/Wrangler 全形式に波及する。
- 「IaC は全部 adapter trait を持つべき」という素朴な対称性原則を採らない
  ことを明示する。形式の構造に応じて差別化する。
- 将来の 4 番目以降の形式追加時に、本 ADR のチェックリストに従って
  trait の要否を判断する。
