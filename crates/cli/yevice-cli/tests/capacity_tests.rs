//! Integration tests for capacity validation (Lambda concurrency, DynamoDB WCU/RCU,
//! Kinesis shards). Verifies that `build_capacity_models` returns models for the
//! relevant services and that `validate_capacity` flags constraint violations.

mod common;
use common::{Params, VariableName, build_arch, load_fixture};

use yevice_cfn::{convert, parser};
use yevice_core::capacity::{Quotas, Severity, validate_capacity};
use yevice_service_api::{CfnAdapterRegistry, ServiceCatalog, TfAdapterRegistry};
use yevice_services_aws::AwsPricingCatalog;
use yevice_services_aws::quotas::LAMBDA_CONCURRENT_EXECUTIONS;

fn build_capacity(
    name: &str,
    resources: &std::collections::HashMap<String, parser::CfnResource>,
    quotas: &Quotas,
) -> Vec<yevice_core::capacity::CapacityModel> {
    let tmpl = parser::CfnTemplate {
        parameters: std::collections::HashMap::new(),
        mappings: std::collections::HashMap::new(),
        conditions: std::collections::HashMap::new(),
        resources: resources.clone(),
    };
    let mut catalog = ServiceCatalog::new();
    let mut cfn = CfnAdapterRegistry::new();
    let mut tf = TfAdapterRegistry::new();
    yevice_services_aws::register(&mut catalog, &mut cfn, &mut tf);
    let arch = convert::build_architecture(name, "ap-northeast-1", &tmpl, &cfn);
    let _ = AwsPricingCatalog::new("ap-northeast-1"); // unused, but ensures it compiles
    catalog.build_capacity_models(&arch, quotas)
}

#[test]
fn test_lambda_capacity_model_exists() {
    let resources = load_fixture("serverless-rest-api.yml");
    let quotas = Quotas::default();
    let models = build_capacity("serverless-rest-api", &resources, &quotas);

    let lambda_models: Vec<_> = models
        .iter()
        .filter(|m| m.label.starts_with("Lambda:"))
        .collect();
    assert!(
        !lambda_models.is_empty(),
        "Lambda services should produce capacity models"
    );

    let lambda = lambda_models[0];
    assert!(
        lambda
            .constraints
            .iter()
            .any(|c| c.dimension == "concurrent_executions"),
        "Lambda capacity model should have concurrent_executions constraint"
    );
}

#[test]
fn test_lambda_concurrency_violation_detected() {
    let resources = load_fixture("serverless-rest-api.yml");
    let quotas = Quotas::default().with(LAMBDA_CONCURRENT_EXECUTIONS, 100.0);
    let models = build_capacity("serverless-rest-api", &resources, &quotas);
    let arch = build_arch("serverless-rest-api", &resources, false);

    let lambda_id = arch
        .resources
        .iter()
        .find(|r| r.label.starts_with("Lambda:"))
        .map(|r| r.logical_id.clone())
        .expect("should have at least one Lambda");

    // Peak 200 req/sec * 1000ms duration = 200 concurrent => exceeds 100 quota
    let mut params = Params::default();
    params.insert(
        VariableName::new(format!("{lambda_id}_peak_requests_per_sec")),
        200.0,
    );
    params.insert(
        VariableName::new(format!("{lambda_id}_avg_duration_ms")),
        1000.0,
    );

    let result = validate_capacity(&models, &params);
    let lambda_violations: Vec<_> = result
        .violations
        .iter()
        .filter(|v| v.dimension == "concurrent_executions" && v.resource == lambda_id)
        .collect();

    assert!(
        !lambda_violations.is_empty(),
        "200 concurrent executions should exceed quota of 100"
    );
    assert_eq!(lambda_violations[0].severity, Severity::Error);
}

#[test]
fn test_lambda_concurrency_within_quota_passes() {
    let resources = load_fixture("serverless-rest-api.yml");
    let quotas = Quotas::default(); // uses default fallback (1000 concurrent)
    let models = build_capacity("serverless-rest-api", &resources, &quotas);
    let arch = build_arch("serverless-rest-api", &resources, false);

    let lambda_id = arch
        .resources
        .iter()
        .find(|r| r.label.starts_with("Lambda:"))
        .map(|r| r.logical_id.clone())
        .expect("should have at least one Lambda");

    // Peak 100 req/sec * 200ms = 20 concurrent => well under 1000
    let mut params = Params::default();
    params.insert(
        VariableName::new(format!("{lambda_id}_peak_requests_per_sec")),
        100.0,
    );
    params.insert(
        VariableName::new(format!("{lambda_id}_avg_duration_ms")),
        200.0,
    );

    let result = validate_capacity(&models, &params);
    let lambda_errors: Vec<_> = result
        .violations
        .iter()
        .filter(|v| v.resource == lambda_id && v.severity == Severity::Error)
        .collect();
    assert!(
        lambda_errors.is_empty(),
        "20 concurrent should not violate quota 1000, got: {lambda_errors:?}"
    );
}

#[test]
fn test_dynamodb_provisioned_capacity_model() {
    let resources = load_fixture("provisioned-dynamodb.yml");
    let quotas = Quotas::default();
    let models = build_capacity("provisioned-dynamodb", &resources, &quotas);

    let ddb_models: Vec<_> = models
        .iter()
        .filter(|m| m.label.starts_with("DynamoDB Provisioned:"))
        .collect();
    assert!(
        !ddb_models.is_empty(),
        "Provisioned DynamoDB should produce a capacity model"
    );
    assert!(
        ddb_models[0]
            .constraints
            .iter()
            .any(|c| c.dimension == "write_capacity_units"),
        "Provisioned DDB should have WCU constraint"
    );
}

#[test]
fn test_kinesis_provisioned_capacity_model() {
    let resources = load_fixture("streaming-pipeline.yml");
    let quotas = Quotas::default();
    let models = build_capacity("streaming-pipeline", &resources, &quotas);

    let kinesis_models: Vec<_> = models
        .iter()
        .filter(|m| m.label.starts_with("Kinesis Provisioned:"))
        .collect();
    assert!(
        !kinesis_models.is_empty(),
        "Kinesis Provisioned should produce a capacity model"
    );
    let dims: Vec<&str> = kinesis_models[0]
        .constraints
        .iter()
        .map(|c| c.dimension.as_str())
        .collect();
    assert!(
        dims.contains(&"shard_throughput"),
        "should have shard_throughput dimension, got {dims:?}"
    );
    assert!(
        dims.contains(&"shard_record_rate"),
        "should have shard_record_rate dimension"
    );
}

#[test]
fn test_kinesis_shard_throughput_violation() {
    let resources = load_fixture("streaming-pipeline.yml");
    let quotas = Quotas::default(); // uses default fallback (1 MB/sec/shard, 1000 records/sec/shard)
    let models = build_capacity("streaming-pipeline", &resources, &quotas);

    let mut params = Params::default();
    // 2 shards available; need 5 MB/sec => required 5 shards => violation
    params.insert(
        VariableName::new("InputStream_peak_ingestion_mb_per_sec"),
        5.0,
    );
    params.insert(VariableName::new("InputStream_peak_records_per_sec"), 0.0);

    let result = validate_capacity(&models, &params);
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.dimension == "shard_throughput" && v.severity == Severity::Error),
        "5 MB/sec on 2-shard stream should violate shard_throughput, got: {:?}",
        result.violations
    );
}
