# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `yevice generate` / `yevice validate` now accept CloudFormation,
  Terraform, and Cloudflare Wrangler inputs via auto-detection or explicit
  `--input-format`, including provider-aware AWS/GCP pricing for Terraform.
- **Currency / billing-period metadata (ADR-0001, Issue #36).** New
  `yevice_core::currency::{Currency<T, C>, CurrencyCode, USD, JPY, EUR,
  BillingPeriod, Money}` types implement the phantom-typed currency at the
  SKU layer and the runtime-tagged `Money` at the architecture aggregation
  layer. New `yevice_core::fx::{ExchangeRates, StaticRates, RateDate,
  convert_to}` provide FX conversion plumbing.
- `yevice_pricing::{PricedValue, PricedTier, TypedPriceRecord<C>, TypedTier<C>,
  TypedPricingProvider<C>}`. `AwsPricingCatalog` implements
  `TypedPricingProvider<USD>` and the existing dyn-friendly `PriceCatalog`.
- New `PricingError::CurrencyMismatch { expected, actual, sku }` for Bulk-API
  metadata vs. provider-declared currency mismatch.
- `CostBuildError::ComponentCurrencyMismatch` raised by `ResourceCost::new`
  (and `ResourceCost::validate`) when a resource's components disagree on
  currency.
- `yevice eval` / `yevice compare` accept `--display-currency <CODE>` plus
  repeatable `--exchange-rate FROM=TO:RATE` for FX conversion of
  mixed-currency totals. Missing rate → hard error; mixed currencies with no
  `--display-currency` → warning + per-currency breakdown.
- `yevice simulate` is now currency-aware: `ArchSimulation` tracks
  `totals_by_currency: BTreeMap<String, f64>` and `display_total: Option<Money>`
  matching the `eval`/`compare`/`sensitivity` pattern. The `simulate` command
  accepts `--display-currency` and `--exchange-rate` flags. Mixed-currency
  models without `--display-currency` print a per-currency breakdown and warn
  instead of silently summing incompatible amounts. The `$`-hardcoded renderer
  is replaced by `fmt_money`-based formatting that respects the declared
  currency (e.g., `¥`-style or `JPY` suffix).
- `ParsePolicy` (`Lenient` / `Strict`, default `Lenient`) and
  `IacParseDiagnostic` in `yevice-core`. The top-level CLI `--strict` flag
  now also drives the parse policy and aborts when the parsers raise any
  error-severity diagnostic (ADR-0003, Issue #38).
- Terraform parser surfaces unresolved `var.*` / `local.*` references via
  the new `TfError::UnresolvedSymbol { kind, name, location }` variant under
  Strict, and as `IacParseDiagnostic` entries under Lenient.

### Changed (BREAKING — major)

- `ArchitectureResult.total_monthly_cost: f64` has been **removed**. Use
  `ArchitectureResult.totals_by_currency: BTreeMap<String, f64>` for
  per-currency totals, `ArchitectureResult.display_total: Option<Money>` for
  the FX-converted single-currency summary populated by the CLI, or
  `ArchitectureResult::naive_total()` for a raw sum (only meaningful in
  single-currency models).
- `ResourceResult.monthly_cost: f64` is now `Money`. Access the scalar with
  `.value` and the ISO 4217 tag with `.currency`.
- `ResourceResult.component_costs: Vec<(String, f64)>` is now
  `Vec<(String, Money)>`.
- `cost_model.json` schema: `ResourceCost` and `CostComponent` carry an
  optional `currency: Option<String>` field. Pre-ADR `cost_model.json`
  deserializes with `currency = None`, evaluated as `USD` with a one-shot
  warning.
- `PriceCatalog::lookup` returns `PricedValue` (currency-tagged Scalar /
  Tiered enum) in place of the deprecated `PriceRecord` alias.

### Changed

- **Breaking (schema):** `cost_model.json` now contains a top-level
  `diagnostics: []` array. Consumers that strictly validate the object must
  accept the new field.
- **Breaking (API):** `yevice_engine::generate_cost_model` returns
  `ParseOutcome<ArchitectureCost>` instead of `ArchitectureCost`; new
  `*_with_policy` variants (`resolve_cfn_template_with_policy`,
  `resolve_tf_input_with_policy`, `build_architecture_from_input_with_policy`,
  `parse_wrangler_with_policy`, `yevice_cfn::parser::resolve_template_with_policy`,
  `yevice_tf::resolver::resolve_config_with_policy`) accept a `ParsePolicy`
  argument. The legacy non-policy entry points keep their old signature for
  source-compatibility (Strict for CFN-shaped callers, Lenient + dropped
  diagnostics for the TF shim).
- `GenerateRequest` gained a `policy: ParsePolicy` field.
- `commands::validate` gained a `strict: bool` parameter so the
  `--strict` global flag now flows into capacity validation too.

## [0.1.0] - 2026-05-28

### Added

- Initial public release.
- CloudFormation, Terraform (HCL), and Cloudflare Wrangler input parsers.
- Cost model generation, evaluation, comparison, sensitivity analysis,
  capacity validation, and load-profile simulation.
- AWS service plugins covering Lambda, EC2, ECS (Fargate/EC2), RDS, S3,
  DynamoDB, Kinesis, SQS, SNS, CloudFront, NAT Gateway, Step Functions,
  EventBridge, ElastiCache, EKS, EFS, MSK, OpenSearch (Service / Serverless),
  DocumentDB, Redshift, Cognito, WAF, Secrets Manager, AppSync, Glue, Athena,
  Route53, ECR, ALB, API Gateway, Firehose, Batch, CloudWatch Logs.
- GCP service skeletons (BigQuery, Cloud Function, Cloud Run, Cloud SQL,
  Cloud Storage, Pub/Sub) — library-only, not wired into the CLI yet.

[Unreleased]: https://github.com/rabe1028/yevice/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/rabe1028/yevice/releases/tag/v0.1.0
