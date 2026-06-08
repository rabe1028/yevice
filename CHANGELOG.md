# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `yevice generate` / `yevice validate` now accept CloudFormation,
  Terraform, and Cloudflare Wrangler inputs via auto-detection or explicit
  `--input-format`, including provider-aware AWS/GCP pricing for Terraform.

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
