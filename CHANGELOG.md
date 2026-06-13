# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `yevice generate` / `yevice validate` now accept CloudFormation,
  Terraform, and Cloudflare Wrangler inputs via auto-detection or explicit
  `--input-format`, including provider-aware AWS/GCP pricing for Terraform.
- `ParsePolicy` (`Lenient` / `Strict`, default `Lenient`) and
  `IacParseDiagnostic` in `yevice-core`. The top-level CLI `--strict` flag
  now also drives the parse policy and aborts when the parsers raise any
  error-severity diagnostic (ADR-0003, Issue #38).
- Terraform parser surfaces unresolved `var.*` / `local.*` references via
  the new `TfError::UnresolvedSymbol { kind, name, location }` variant under
  Strict, and as `IacParseDiagnostic` entries under Lenient.

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
