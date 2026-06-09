//! AWS-specific [`QuotaProvider`] implementation.
//!
//! The quota key constants defined here are consumed by the AWS service
//! implementations (LambdaService, DynamoDbService, KinesisService) when
//! building capacity models. They are also re-exported from the crate root
//! so that callers (CLI, tests) can refer to them without coupling to internal
//! module paths.

use yevice_core::capacity::{QuotaProvider, Quotas};

// ---------------------------------------------------------------------------
// Quota key constants
// ---------------------------------------------------------------------------

pub const LAMBDA_CONCURRENT_EXECUTIONS: &str = "aws.lambda.concurrent_executions";
pub const DYNAMODB_MAX_WCU_PER_TABLE: &str = "aws.dynamodb.max_wcu_per_table";
pub const DYNAMODB_MAX_RCU_PER_TABLE: &str = "aws.dynamodb.max_rcu_per_table";
pub const DYNAMODB_MAX_TABLES: &str = "aws.dynamodb.max_tables";
pub const DYNAMODB_ONDEMAND_INITIAL_THROUGHPUT: &str = "aws.dynamodb.ondemand_initial_throughput";
pub const KINESIS_MAX_SHARDS_PER_STREAM: &str = "aws.kinesis.max_shards_per_stream";
pub const KINESIS_MAX_RECORDS_PER_SEC_PER_SHARD: &str = "aws.kinesis.max_records_per_sec_per_shard";
pub const KINESIS_MAX_MB_PER_SEC_PER_SHARD: &str = "aws.kinesis.max_mb_per_sec_per_shard";

// ---------------------------------------------------------------------------
// Default values (mirror the old RegionQuotas::default() for ap-northeast-1)
// ---------------------------------------------------------------------------

pub const DEFAULT_LAMBDA_CONCURRENT_EXECUTIONS: f64 = 1000.0;
pub const DEFAULT_DYNAMODB_MAX_WCU_PER_TABLE: f64 = 40_000.0;
pub const DEFAULT_DYNAMODB_MAX_RCU_PER_TABLE: f64 = 40_000.0;
pub const DEFAULT_DYNAMODB_MAX_TABLES: f64 = 2500.0;
pub const DEFAULT_DYNAMODB_ONDEMAND_INITIAL_THROUGHPUT: f64 = 40_000.0;
pub const DEFAULT_KINESIS_MAX_SHARDS_PER_STREAM: f64 = 200.0;
pub const DEFAULT_KINESIS_MAX_RECORDS_PER_SEC_PER_SHARD: f64 = 1000.0;
pub const DEFAULT_KINESIS_MAX_MB_PER_SEC_PER_SHARD: f64 = 1.0;

// ---------------------------------------------------------------------------
// AwsQuotaProvider
// ---------------------------------------------------------------------------

/// Provides default AWS service quotas for a region.
///
/// The current implementation returns the ap-northeast-1 hardcoded defaults
/// (identical to the old `RegionQuotas::default()`). The `region` argument is
/// accepted for future per-region customisation but is not yet used.
pub struct AwsQuotaProvider;

impl QuotaProvider for AwsQuotaProvider {
    fn default_quotas(&self, _region: &str) -> Quotas {
        Quotas::default()
            .with(
                LAMBDA_CONCURRENT_EXECUTIONS,
                DEFAULT_LAMBDA_CONCURRENT_EXECUTIONS,
            )
            .with(
                DYNAMODB_MAX_WCU_PER_TABLE,
                DEFAULT_DYNAMODB_MAX_WCU_PER_TABLE,
            )
            .with(
                DYNAMODB_MAX_RCU_PER_TABLE,
                DEFAULT_DYNAMODB_MAX_RCU_PER_TABLE,
            )
            .with(DYNAMODB_MAX_TABLES, DEFAULT_DYNAMODB_MAX_TABLES)
            .with(
                DYNAMODB_ONDEMAND_INITIAL_THROUGHPUT,
                DEFAULT_DYNAMODB_ONDEMAND_INITIAL_THROUGHPUT,
            )
            .with(
                KINESIS_MAX_SHARDS_PER_STREAM,
                DEFAULT_KINESIS_MAX_SHARDS_PER_STREAM,
            )
            .with(
                KINESIS_MAX_RECORDS_PER_SEC_PER_SHARD,
                DEFAULT_KINESIS_MAX_RECORDS_PER_SEC_PER_SHARD,
            )
            .with(
                KINESIS_MAX_MB_PER_SEC_PER_SHARD,
                DEFAULT_KINESIS_MAX_MB_PER_SEC_PER_SHARD,
            )
    }
}
