# AWS CDP アーキテクチャ一致確認カバレッジ

[AWS クラウド構成パターン (CDP)](https://aws.amazon.com/jp/cdp/) の公表月額コストと `yevice`
の算出値の突き合わせ結果。`--list-price` 付きの行は `yevice generate --list-price` で生成
（AWS の見積もりは販促 Free Tier を適用しないため）。

凡例: ✅ 完全一致(セント単位) ／ ◎ 近似一致(差分は文書化) ／ ⚠️ 構造的に一致困難

| CDP アーキ | slug | 公表額(USD) | yevice | 判定 | 備考 |
| --- | --- | --- | --- | --- | --- |
| ウェブサイト(入門編) | migrate-lightsail | 3.43 | 3.43 | ✅ | |
| 社内データ分析(入門編) | analytics-report-basic | 40.25 | 40.25 | ✅ | QuickSight SPICE は製品付属枠 |
| コンテナ(ECS/Fargate) | ec-container | 1236.69 | 1236.69 | ✅ | `--list-price` |
| 社内データ分析(基本編) | analytics-report-advanced | 73.85 | 73.85 | ✅ | S3+Athena+QuickSight |
| 社内データ分析(応用編) | analytics-report-master | 518.93 | 518.93 | ✅ | Redshift+Spectrum, 720h |
| Windows業務アプリ移行(入門編) | windows-bizapp-migration-basic | 1027.68 | 1027.68 | ✅ | `--list-price` EC2 Win + RDS SQL Server |
| Windows業務アプリ移行(基本編) | windows-bizapp-migration | 2034.70 | 2034.70 | ✅ | `--list-price` Multi-AZ RDS storage 倍化 |
| Windows業務アプリ移行(応用編) | windows-bizapp-migration-master | 2059.63 | 2059.62 | ◎ | -$0.01 (CloudWatch Logs セント丸め) |
| Windowsファイルサーバー(基本編) | fileserver-fsx | 1048.87 | 1048.87 | ✅ | `--list-price` 転送無料枠を除去して完全一致 |
| Windowsファイルサーバー(応用編) | windows-managedservice | 1402.57 | 1401.21 | ◎ | -$1.36 (Backup: ページが 3072GB×$0.05=$153.60 を $154.28 と計上=算術不整合) |
| Windowsファイルサーバー(入門編) | fileserver-scaleup | 472.67 | 473.54 | ◎ | +$0.87 (EBS snapshot: ページは増分スナップショット ~3055GB 前提、yevice は全量 3072GB×$0.05) |
| 生成AIチャットボット(社内知識) | ai-chatapp | 941.99 | (新サービス検証) | ⚠️ | Kendra/Bedrock/Transcribe は正確。DynamoDB On-Demand の file-backed 価格データ(`pricing-data/dynamodb.json`)が既存バグで過大 |
| ウェブサイト(応用編) | ec-scaleup | 1008.18 | — | ⚠️ | EC2 AutoScaling の時間加重(2〜4台/日2h)モデル未対応 + ElastiCache cache.m5.large 未収録 |
| ウェブサイト(基本編) | midscale-webservice | 517.26 | — | ⚠️ | EC2 インスタンスクラスがページに未記載($0.0864/h=該当クラスなし) |
| 生成AIチャットボット(Knowledge Bases) | genai-chat-app | 285.69 | — | ⚠️ | OpenSearch Serverless OCU: Tokyo $0.334 が正だが CDP ページは us-east-1 の $0.24 を使用 |

## 未対応3アーキの実現可能性 (検証済みの結論)

いずれも根本原因は **AWS ページ側のデータ欠落・レート不整合** であり、yevice の追加実装だけでは
「金額を逆算した合成値」を置かない限り完全一致しない。

- **genai-chat-app** — *Region 対応では解決しない*。AWS Price List API で ap-northeast-1 の
  OpenSearch Serverless OCU は **$0.334/OCU-h** と確認。CDP ページの $0.24 は us-east-1 レートで、
  日本語(Tokyo)ページが OCU だけ US レートを使う **ページ内のリージョン混在(=AWS側の不整合)**。
  yevice に us-east-1 OCU を足しても、他サービスは Tokyo のままなので「Tokyo + OCU だけ US」という
  ページの不整合を再現する以外に一致手段がなく、原則として実装対象外。
- **midscale-webservice** — *追加実装では不可*。EC2 が「Linuxサーバー × 2 ($126.14)」としか書かれず、
  インスタンスクラスが**ページに未記載**($63.07/台 = $0.0864/h は標準クラスに該当なし)。型番が分からない
  ため再現できない。ページに型番が出れば即対応可能。
- **ec-scaleup** — *部分的に前進、ただし同じEC2データ欠落で完全一致不可*。
  - ElastiCache `cache.m5.large` (6.38GiB) は **追加済み**(本対応)。
  - AutoScaling の時間加重(平常2台・日2h だけ4台)は Redshift と同様に EC2 を「実効インスタンス時間」
    変数化すれば表現可能(追加実装で対応可)。
  - ただし EC2 のクラスがページに未記載($0.102/h 相当=標準クラスなし)なため、midscale と同じ理由で
    最終的な完全一致はできない。

## 設計上の差分 (バグではない)

- **Free Tier (転送無料枠を含む)**: yevice は既定で AWS の販促 Free Tier を控除する(実請求に近い)。
  CDP 見積もりはリスト価格のため `generate --list-price` で一致させる。`--list-price` は
  (1) `free_tier_*` SKU を 0 にし、(2) tiered 価格の**先頭の無料(単価0)tier**(例: 内部データ転送の
  先頭1GB無料)も除去する。製品付属の割当(QuickSight SPICE 10GB、gp3 baseline IOPS 等)は両モード保持。
- **EBS Snapshot / AWS Backup**: いずれも **$0.05/GB-月 (検証済みの正レート)** で第一級にモデル化済み
  (`EBS` の `snapshot_gb`、`backup` の `backup_gb`、`fsx_windows` の `backup_gb`)。close 一致の残差は
  AWS ページ側の事情 — windows-managedservice は backup 行が算術不整合($153.60 を $154.28 と計上)、
  fileserver-scaleup は増分スナップショット前提 — であり、yevice の計算($0.05 × 全量GB)が正しい。
- **DynamoDB On-Demand**: `pricing-data/dynamodb.json` の write/read request 価格が桁ずれしている疑い
  (ユニットテストは mock 価格を使うため露見しない既存課題)。ai-chatapp の DynamoDB 行のみ影響。
