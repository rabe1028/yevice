//! Integration tests for data pipeline and content delivery architectures.
//!
//! Architectures tested:
//!   streaming-pipeline   — Kinesis (Provisioned, 2 shards) -> Lambda -> S3
//!   firehose-delivery    — Kinesis Firehose -> Lambda -> S3
//!   cdn-static-website   — CloudFront + 2x S3
//!   vpc-with-nat         — NAT Gateway + Lambda

mod common;
use common::{build_arch as build_architecture_cost, load_fixture, p};

use yevice_core::evaluate::evaluate_architecture;

// ============================================================================
// streaming-pipeline.yml — Kinesis -> Lambda -> S3
// ============================================================================

#[test]
fn test_streaming_pipeline_resource_count() {
    let resources = load_fixture("streaming-pipeline.yml");
    let arch = build_architecture_cost("streaming-pipeline", &resources, false);

    // InputStream (Kinesis) + ProcessorFunction (Lambda) + OutputBucket (S3) + ProcessorLogs (CW Logs)
    assert_eq!(
        arch.resources.len(),
        4,
        "streaming-pipeline should have exactly 4 cost-modeled resources"
    );
}

#[test]
fn test_kinesis_provisioned_base_cost() {
    // 2 shards * $0.0195/shard-hour * 730 hours/month = $28.47/month (base, no records)
    let resources = load_fixture("streaming-pipeline.yml");
    let arch = build_architecture_cost("streaming-pipeline", &resources, false);

    // Only measure the Kinesis stream by providing 0 put_records
    let params = p(&[
        ("InputStream_put_records", 0.0),
        // Lambda and S3 and CW Logs variables set to 0 to isolate Kinesis cost
        ("ProcessorFunction_requests", 0.0),
        ("ProcessorFunction_avg_duration_ms", 0.0),
        ("ProcessorFunction_data_transfer_out_gb", 0.0),
        ("OutputBucket_storage_gb", 0.0),
        ("OutputBucket_put_requests", 0.0),
        ("OutputBucket_get_requests", 0.0),
        ("ProcessorLogs_ingestion_gb", 0.0),
        ("ProcessorLogs_storage_gb", 0.0),
    ]);
    let result = evaluate_architecture(&arch, &params).unwrap();

    let kinesis = result
        .resources
        .iter()
        .find(|r| r.logical_id == "InputStream")
        .unwrap();
    let expected_base = 2.0 * 0.0195 * 730.0; // $28.47
    assert!(
        (kinesis.monthly_cost.value - expected_base).abs() < 0.01,
        "2-shard Kinesis base cost should be ~${expected_base:.2}, got ${:.2}",
        kinesis.monthly_cost.value
    );
}

#[test]
fn test_kinesis_provisioned_shard_cost() {
    // With the same put_records, a 2-shard stream costs exactly 2x the shard component of a 1-shard
    // stream (records cost is the same). We verify by comparing base (0 records) costs.
    // 1-shard base = $0.0195 * 730 = $14.235
    // 2-shard base = $0.0195 * 730 * 2 = $28.47
    // ratio = 2.0
    let resources = load_fixture("streaming-pipeline.yml");
    let arch = build_architecture_cost("streaming-pipeline", &resources, false);

    let params_zero = p(&[
        ("InputStream_put_records", 0.0),
        ("ProcessorFunction_requests", 0.0),
        ("ProcessorFunction_avg_duration_ms", 0.0),
        ("ProcessorFunction_data_transfer_out_gb", 0.0),
        ("OutputBucket_storage_gb", 0.0),
        ("OutputBucket_put_requests", 0.0),
        ("OutputBucket_get_requests", 0.0),
        ("ProcessorLogs_ingestion_gb", 0.0),
        ("ProcessorLogs_storage_gb", 0.0),
    ]);

    let result_zero = evaluate_architecture(&arch, &params_zero).unwrap();
    let kinesis_zero = result_zero
        .resources
        .iter()
        .find(|r| r.logical_id == "InputStream")
        .unwrap();

    // The shard-only cost for 2 shards should be exactly 2x the per-shard rate * hours
    let per_shard_month = 0.0195 * 730.0;
    let expected_two_shards = 2.0 * per_shard_month;

    assert!(
        (kinesis_zero.monthly_cost.value - expected_two_shards).abs() < 0.01,
        "2-shard provisioned cost should be 2x one-shard rate: expected ${expected_two_shards:.3}, got ${:.3}",
        kinesis_zero.monthly_cost.value
    );
}

// ============================================================================
// firehose-delivery.yml — Kinesis Firehose -> Lambda -> S3
// ============================================================================

#[test]
fn test_firehose_delivery_resource_count() {
    let resources = load_fixture("firehose-delivery.yml");
    let arch = build_architecture_cost("firehose-delivery", &resources, false);

    // LogDelivery (Firehose) + LogBucket (S3) + TransformerFunction (Lambda) + TransformerLogs (CW Logs)
    assert_eq!(
        arch.resources.len(),
        4,
        "firehose-delivery should have exactly 4 cost-modeled resources"
    );
}

#[test]
fn test_firehose_scales_with_ingestion() {
    // 1000 GB ingested costs more than 100 GB (pure pay-per-GB model, no fixed cost)
    let resources = load_fixture("firehose-delivery.yml");
    let arch = build_architecture_cost("firehose-delivery", &resources, false);

    let common_other = [
        ("LogBucket_storage_gb", 0.0),
        ("LogBucket_put_requests", 0.0),
        ("LogBucket_get_requests", 0.0),
        ("TransformerFunction_requests", 0.0),
        ("TransformerFunction_avg_duration_ms", 0.0),
        ("TransformerFunction_data_transfer_out_gb", 0.0),
        ("TransformerLogs_ingestion_gb", 0.0),
        ("TransformerLogs_storage_gb", 0.0),
    ];

    let mut low_params: Vec<(&str, f64)> = vec![("LogDelivery_ingestion_gb", 100.0)];
    low_params.extend_from_slice(&common_other);

    let mut high_params: Vec<(&str, f64)> = vec![("LogDelivery_ingestion_gb", 1000.0)];
    high_params.extend_from_slice(&common_other);

    let low_result = evaluate_architecture(&arch, &p(&low_params)).unwrap();
    let high_result = evaluate_architecture(&arch, &p(&high_params)).unwrap();

    assert!(
        high_result.naive_total() > low_result.naive_total(),
        "1000 GB firehose ingestion (${:.2}) should cost more than 100 GB (${:.2})",
        high_result.naive_total(),
        low_result.naive_total()
    );
}

#[test]
fn test_firehose_exact_cost() {
    // 500 GB * $0.031/GB = $15.50 exactly
    let resources = load_fixture("firehose-delivery.yml");
    let arch = build_architecture_cost("firehose-delivery", &resources, false);

    let params = p(&[
        ("LogDelivery_ingestion_gb", 500.0),
        ("LogBucket_storage_gb", 0.0),
        ("LogBucket_put_requests", 0.0),
        ("LogBucket_get_requests", 0.0),
        ("TransformerFunction_requests", 0.0),
        ("TransformerFunction_avg_duration_ms", 0.0),
        ("TransformerFunction_data_transfer_out_gb", 0.0),
        ("TransformerLogs_ingestion_gb", 0.0),
        ("TransformerLogs_storage_gb", 0.0),
    ]);
    let result = evaluate_architecture(&arch, &params).unwrap();

    let firehose = result
        .resources
        .iter()
        .find(|r| r.logical_id == "LogDelivery")
        .unwrap();
    let expected = 500.0 * 0.031; // $15.50
    assert!(
        (firehose.monthly_cost.value - expected).abs() < 0.001,
        "500 GB Firehose ingestion should cost ${expected:.2}, got ${:.2}",
        firehose.monthly_cost.value
    );
}

// ============================================================================
// cdn-static-website.yml — CloudFront + 2x S3
// ============================================================================

#[test]
fn test_cdn_resource_count() {
    let resources = load_fixture("cdn-static-website.yml");
    let arch = build_architecture_cost("cdn-static-website", &resources, false);

    // ContentCDN (CloudFront) + AssetsBucket (S3) + WwwBucket (S3) = 3 resources
    assert_eq!(
        arch.resources.len(),
        3,
        "cdn-static-website should have exactly 3 cost-modeled resources"
    );
}

#[test]
fn test_cloudfront_free_tier_applies() {
    // CloudFront free tier = 1000 GB/month data transfer.
    // At 500 GB (under free tier), data transfer cost should be $0.
    let resources = load_fixture("cdn-static-website.yml");
    let arch = build_architecture_cost("cdn-static-website", &resources, false);

    let params = p(&[
        ("ContentCDN_http_requests", 0.0),
        ("ContentCDN_data_transfer_gb", 500.0), // under 1000 GB free tier
        ("AssetsBucket_storage_gb", 0.0),
        ("AssetsBucket_put_requests", 0.0),
        ("AssetsBucket_get_requests", 0.0),
        ("WwwBucket_storage_gb", 0.0),
        ("WwwBucket_put_requests", 0.0),
        ("WwwBucket_get_requests", 0.0),
    ]);
    let result = evaluate_architecture(&arch, &params).unwrap();

    let cdn = result
        .resources
        .iter()
        .find(|r| r.logical_id == "ContentCDN")
        .unwrap();
    // With 0 requests and 500 GB transfer (within free tier), CloudFront cost should be $0
    assert!(
        cdn.monthly_cost.value.abs() < 0.001,
        "CloudFront at 500 GB (within 1000 GB free tier) should cost $0, got ${:.4}",
        cdn.monthly_cost.value
    );
}

#[test]
fn test_cloudfront_above_free_tier() {
    // At 2000 GB transfer (1000 GB above free tier), cost should be positive.
    // Expected transfer cost = (2000 - 1000) * $0.114 = $114.00
    let resources = load_fixture("cdn-static-website.yml");
    let arch = build_architecture_cost("cdn-static-website", &resources, false);

    let params_under = p(&[
        ("ContentCDN_http_requests", 0.0),
        ("ContentCDN_data_transfer_gb", 500.0),
        ("AssetsBucket_storage_gb", 0.0),
        ("AssetsBucket_put_requests", 0.0),
        ("AssetsBucket_get_requests", 0.0),
        ("WwwBucket_storage_gb", 0.0),
        ("WwwBucket_put_requests", 0.0),
        ("WwwBucket_get_requests", 0.0),
    ]);
    let params_over = p(&[
        ("ContentCDN_http_requests", 0.0),
        ("ContentCDN_data_transfer_gb", 2000.0),
        ("AssetsBucket_storage_gb", 0.0),
        ("AssetsBucket_put_requests", 0.0),
        ("AssetsBucket_get_requests", 0.0),
        ("WwwBucket_storage_gb", 0.0),
        ("WwwBucket_put_requests", 0.0),
        ("WwwBucket_get_requests", 0.0),
    ]);

    let result_under = evaluate_architecture(&arch, &params_under).unwrap();
    let result_over = evaluate_architecture(&arch, &params_over).unwrap();

    assert!(
        result_over.naive_total() > result_under.naive_total(),
        "2000 GB transfer (${:.2}) should cost more than 500 GB (${:.2})",
        result_over.naive_total(),
        result_under.naive_total()
    );

    let cdn_over = result_over
        .resources
        .iter()
        .find(|r| r.logical_id == "ContentCDN")
        .unwrap();
    let expected_transfer_cost = (2000.0 - 1000.0) * 0.114; // $114.00
    assert!(
        (cdn_over.monthly_cost.value - expected_transfer_cost).abs() < 0.01,
        "CloudFront at 2000 GB should cost ~${expected_transfer_cost:.2} (1000 GB over free tier), got ${:.2}",
        cdn_over.monthly_cost.value
    );
}

// ============================================================================
// vpc-with-nat.yml — NAT Gateway + Lambda
// ============================================================================

#[test]
fn test_nat_gateway_base_cost() {
    // NAT Gateway fixed cost: $0.062/hr * 730 hr/month = $45.26/month
    let resources = load_fixture("vpc-with-nat.yml");
    let arch = build_architecture_cost("vpc-with-nat", &resources, false);

    // Zero data processed — only the fixed hourly component
    let params = p(&[
        ("EgressNatGateway_data_processed_gb", 0.0),
        ("AppFunction_requests", 0.0),
        ("AppFunction_avg_duration_ms", 0.0),
        ("AppFunction_data_transfer_out_gb", 0.0),
        ("AppLogs_ingestion_gb", 0.0),
        ("AppLogs_storage_gb", 0.0),
    ]);
    let result = evaluate_architecture(&arch, &params).unwrap();

    let nat = result
        .resources
        .iter()
        .find(|r| r.logical_id == "EgressNatGateway")
        .unwrap();
    let expected_base = 0.062 * 730.0; // $45.26
    assert!(
        (nat.monthly_cost.value - expected_base).abs() < 0.01,
        "NAT Gateway base (0 data) should cost ~${expected_base:.2}, got ${:.2}",
        nat.monthly_cost.value
    );
}

#[test]
fn test_nat_gateway_scales_with_data() {
    // Base ($45.26) + 100 GB * $0.062/GB ($6.20) = $51.46
    let resources = load_fixture("vpc-with-nat.yml");
    let arch = build_architecture_cost("vpc-with-nat", &resources, false);

    let params = p(&[
        ("EgressNatGateway_data_processed_gb", 100.0),
        ("AppFunction_requests", 0.0),
        ("AppFunction_avg_duration_ms", 0.0),
        ("AppFunction_data_transfer_out_gb", 0.0),
        ("AppLogs_ingestion_gb", 0.0),
        ("AppLogs_storage_gb", 0.0),
    ]);
    let result = evaluate_architecture(&arch, &params).unwrap();

    let nat = result
        .resources
        .iter()
        .find(|r| r.logical_id == "EgressNatGateway")
        .unwrap();
    let expected = 0.062 * 730.0 + 100.0 * 0.062; // $45.26 + $6.20 = $51.46
    assert!(
        (nat.monthly_cost.value - expected).abs() < 0.01,
        "NAT Gateway with 100 GB data should cost ~${expected:.2}, got ${:.2}",
        nat.monthly_cost.value
    );
}

// ============================================================================
// S3 storage cost scaling (using streaming-pipeline fixture)
// ============================================================================

#[test]
fn test_s3_storage_cost_scales() {
    // S3 standard storage: $0.025/GB for the first 50 TB.
    // 10000 GB * $0.025/GB = $250.00
    let resources = load_fixture("streaming-pipeline.yml");
    let arch = build_architecture_cost("streaming-pipeline", &resources, false);

    let params = p(&[
        ("InputStream_put_records", 0.0),
        ("ProcessorFunction_requests", 0.0),
        ("ProcessorFunction_avg_duration_ms", 0.0),
        ("ProcessorFunction_data_transfer_out_gb", 0.0),
        ("OutputBucket_storage_gb", 10_000.0),
        ("OutputBucket_put_requests", 0.0),
        ("OutputBucket_get_requests", 0.0),
        ("ProcessorLogs_ingestion_gb", 0.0),
        ("ProcessorLogs_storage_gb", 0.0),
    ]);
    let result = evaluate_architecture(&arch, &params).unwrap();

    let bucket = result
        .resources
        .iter()
        .find(|r| r.logical_id == "OutputBucket")
        .unwrap();
    let expected = 10_000.0 * 0.025; // $250.00
    assert!(
        (bucket.monthly_cost.value - expected).abs() < 0.01,
        "S3 10000 GB storage should cost ~${expected:.2}, got ${:.2}",
        bucket.monthly_cost.value
    );
}
