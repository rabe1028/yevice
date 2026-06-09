//! Integration tests using realistic `CloudFormation` templates
//!
//! Architecture:
//!   shared-dynamodb (3 tables, 1 with Stream) ─┐
//!   shared-sqs (4 queues: 2 FIFO + 2 Standard) │
//!   shared-s3 (2 buckets)                       ├─> `Fn::ImportValue`
//!   orders-kinesis (1 stream)                   │
//!   catalog-aoss (1 collection)                 │
//!   orders-ingest.sam ─ Kinesis->Lambda ────────┘
//!   catalog-indexing.sam ─ DDB Stream->Lambda->AOSS

use std::collections::HashMap;
use std::path::PathBuf;

use yevice_cfn::convert;
use yevice_cfn::parser;
use yevice_core::bindings::{BindingsFile, to_variable_bindings};
use yevice_core::evaluate::{Params, evaluate_architecture};
use yevice_core::types::VariableName;
use yevice_service_api::{CfnAdapterRegistry, ServiceCatalog, TfAdapterRegistry};
use yevice_services_aws::AwsPricingCatalog;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn load_params(name: &str) -> HashMap<String, String> {
    let path = fixtures_dir().join(name);
    let content = std::fs::read_to_string(&path).unwrap();
    let map: HashMap<String, serde_yaml_ng::Value> = serde_yaml_ng::from_str(&content).unwrap();
    map.into_iter()
        .filter_map(|(k, v)| match v {
            serde_yaml_ng::Value::String(s) => Some((k, s)),
            serde_yaml_ng::Value::Number(n) => Some((k, n.to_string())),
            serde_yaml_ng::Value::Bool(b) => Some((k, b.to_string())),
            _ => None,
        })
        .collect()
}

fn load_usage(name: &str) -> Params {
    let path = fixtures_dir().join(name);
    let content = std::fs::read_to_string(&path).unwrap();
    let map: HashMap<String, serde_yaml_ng::Value> = serde_yaml_ng::from_str(&content).unwrap();
    let mut params = Params::new();
    for (k, v) in map {
        match v {
            serde_yaml_ng::Value::Mapping(sub_map) => {
                for (sub_k, sub_v) in sub_map {
                    let Some(sub_key) = sub_k.as_str() else {
                        continue;
                    };
                    if let Some(val) = extract_f64(&sub_v) {
                        params.insert(VariableName::new(format!("{k}_{sub_key}")), val);
                    }
                }
            }
            _ => {
                if let Some(val) = extract_f64(&v) {
                    params.insert(VariableName::new(k), val);
                }
            }
        }
    }
    params
}

fn extract_f64(v: &serde_yaml_ng::Value) -> Option<f64> {
    match v {
        serde_yaml_ng::Value::Number(n) => n.as_f64(),
        serde_yaml_ng::Value::String(s) => s.parse::<f64>().ok(),
        _ => None,
    }
}

fn build_architecture_cost(
    name: &str,
    resources: &HashMap<String, yevice_cfn::parser::CfnResource>,
    _pricing: (),
    strict: bool,
) -> yevice_core::cost::ArchitectureCost {
    let resolved_template = yevice_cfn::parser::CfnTemplate {
        parameters: HashMap::new(),
        mappings: HashMap::new(),
        conditions: HashMap::new(),
        resources: resources.clone(),
    };
    let mut catalog = ServiceCatalog::new();
    let mut cfn_adapters = CfnAdapterRegistry::new();
    let mut tf_adapters = TfAdapterRegistry::new();
    yevice_services_aws::register(&mut catalog, &mut cfn_adapters, &mut tf_adapters);
    let arch =
        convert::build_architecture(name, "ap-northeast-1", &resolved_template, &cfn_adapters);
    let pricing = AwsPricingCatalog::new("ap-northeast-1");
    catalog.build_cost_model(&arch, &pricing, strict).unwrap()
}

// =========================================================================
// shared-dynamodb: 3 DynamoDB tables
// =========================================================================
#[test]
fn test_base_dynamodb_generates_3_tables() {
    let tmpl = parser::parse_template(fixtures_dir().join("shared-dynamodb.yml").as_ref()).unwrap();
    let params = load_params("params-prd.yaml");
    let resources = parser::resolve_template(&tmpl, &params, &HashMap::new()).unwrap();

    let arch = build_architecture_cost("shared-dynamodb", &resources, (), false);
    assert_eq!(arch.resources.len(), 3);

    // All should be DynamoDB
    for r in &arch.resources {
        assert_eq!(r.resource_type, "AWS::DynamoDB::Table");
    }

    // Each table should require WRU, RRU, storage variables
    for r in &arch.resources {
        assert_eq!(r.required_variables.len(), 3);
    }
}

#[test]
fn test_base_dynamodb_cost_evaluation() {
    let tmpl = parser::parse_template(fixtures_dir().join("shared-dynamodb.yml").as_ref()).unwrap();
    let params = load_params("params-prd.yaml");
    let resources = parser::resolve_template(&tmpl, &params, &HashMap::new()).unwrap();

    let arch = build_architecture_cost("shared-dynamodb", &resources, (), false);
    let usage = load_usage("usage.yaml");
    let result = evaluate_architecture(&arch, &usage).unwrap();

    // Total should be positive
    assert!(result.total_monthly_cost > 0.0);

    // OrderTable (highest usage) should cost the most
    let event = result
        .resources
        .iter()
        .find(|r| r.logical_id == "OrderTable")
        .unwrap();
    let metadata = result
        .resources
        .iter()
        .find(|r| r.logical_id == "CustomerTable")
        .unwrap();
    assert!(event.monthly_cost > metadata.monthly_cost);
}

// =========================================================================
// shared-sqs: FIFO + Standard queues with DLQ pattern
// =========================================================================
#[test]
fn test_base_sqs_fifo_and_standard() {
    let tmpl = parser::parse_template(fixtures_dir().join("shared-sqs.yml").as_ref()).unwrap();
    let params = load_params("params-prd.yaml");
    let resources = parser::resolve_template(&tmpl, &params, &HashMap::new()).unwrap();

    let arch = build_architecture_cost("shared-sqs", &resources, (), false);
    assert_eq!(arch.resources.len(), 4);

    // Check FIFO queues have FIFO label
    let fifo_queues: Vec<_> = arch
        .resources
        .iter()
        .filter(|r| r.label.contains("FIFO"))
        .collect();
    assert_eq!(fifo_queues.len(), 2, "should have 2 FIFO queues");

    let standard_queues: Vec<_> = arch
        .resources
        .iter()
        .filter(|r| r.label.contains("Standard"))
        .collect();
    assert_eq!(standard_queues.len(), 2, "should have 2 Standard queues");
}

// =========================================================================
// shared-s3: 2 buckets with lifecycle rules
// =========================================================================
#[test]
fn test_base_s3_generates_2_buckets() {
    let tmpl = parser::parse_template(fixtures_dir().join("shared-s3.yml").as_ref()).unwrap();
    let params = load_params("params-prd.yaml");
    let resources = parser::resolve_template(&tmpl, &params, &HashMap::new()).unwrap();

    let arch = build_architecture_cost("shared-s3", &resources, (), false);
    assert_eq!(arch.resources.len(), 2);
}

// =========================================================================
// orders-kinesis: env-dependent shard count via !FindInMap
// =========================================================================
#[test]
fn test_kinesis_prd_vs_dev_shard_count() {
    let tmpl_path = fixtures_dir().join("orders-kinesis.yml");

    // prd: 2 shards
    let tmpl = parser::parse_template(tmpl_path.as_ref()).unwrap();
    let prd_params = load_params("params-prd.yaml");
    let prd_resources = parser::resolve_template(&tmpl, &prd_params, &HashMap::new()).unwrap();
    let prd_arch = build_architecture_cost("kinesis-prd", &prd_resources, (), false);

    // dev: 1 shard
    let tmpl = parser::parse_template(tmpl_path.as_ref()).unwrap();
    let dev_params = load_params("params-dev.yaml");
    let dev_resources = parser::resolve_template(&tmpl, &dev_params, &HashMap::new()).unwrap();
    let dev_arch = build_architecture_cost("kinesis-dev", &dev_resources, (), false);

    let usage = load_usage("usage.yaml");
    let prd_result = evaluate_architecture(&prd_arch, &usage).unwrap();
    let dev_result = evaluate_architecture(&dev_arch, &usage).unwrap();

    // prd (2 shards) should cost more than dev (1 shard)
    assert!(
        prd_result.total_monthly_cost > dev_result.total_monthly_cost,
        "prd ({}) should cost more than dev ({})",
        prd_result.total_monthly_cost,
        dev_result.total_monthly_cost
    );

    // Shard cost difference should be exactly 1 shard * hours
    let shard_hour_price = 0.0195;
    let hours_per_month = 730.0;
    let expected_diff = shard_hour_price * hours_per_month;
    let actual_diff = prd_result.total_monthly_cost - dev_result.total_monthly_cost;
    assert!(
        (actual_diff - expected_diff).abs() < 0.01,
        "shard cost diff should be ~${expected_diff:.2}, got ${actual_diff:.2}"
    );
}

// =========================================================================
// catalog-aoss: Condition with !FindInMap boolean
// =========================================================================
#[test]
fn test_aoss_collection_recognized() {
    let tmpl = parser::parse_template(fixtures_dir().join("catalog-aoss.yml").as_ref()).unwrap();
    let params = load_params("params-prd.yaml");
    let resources = parser::resolve_template(&tmpl, &params, &HashMap::new()).unwrap();

    let arch = build_architecture_cost("catalog-aoss", &resources, (), false);
    // Only the Collection has a cost model; SecurityPolicies are ignored
    assert_eq!(arch.resources.len(), 1);
    assert!(arch.resources[0].label.contains("OpenSearch Serverless"));
}

#[test]
fn test_aoss_minimum_ocu_cost() {
    let tmpl = parser::parse_template(fixtures_dir().join("catalog-aoss.yml").as_ref()).unwrap();
    let params = load_params("params-prd.yaml");
    let resources = parser::resolve_template(&tmpl, &params, &HashMap::new()).unwrap();
    let arch = build_architecture_cost("catalog-aoss", &resources, (), false);

    let usage = load_usage("usage.yaml");
    let result = evaluate_architecture(&arch, &usage).unwrap();

    // Min 2 OCU indexing + 2 OCU search = 4 OCU * $0.334/hr * 730 hrs = ~$975
    let min_cost = 4.0 * 0.334 * 730.0;
    assert!(
        result.total_monthly_cost >= min_cost,
        "AOSS cost ${:.2} should be >= min OCU cost ${:.2}",
        result.total_monthly_cost,
        min_cost
    );
}

// =========================================================================
// orders-ingest.sam: Kinesis -> Lambda -> DynamoDB + SQS DLQ
// =========================================================================
#[test]
fn test_ingest_pipeline_resource_count() {
    let tmpl =
        parser::parse_template(fixtures_dir().join("orders-ingest.sam.yml").as_ref()).unwrap();
    let params = load_params("params-prd.yaml");
    let imports = load_params("imports.yaml");
    let resources = parser::resolve_template(&tmpl, &params, &imports).unwrap();

    let arch = build_architecture_cost("ingest", &resources, (), false);

    // Expected: 2 Lambda(Serverless) + 2 SQS + 2 LogGroup + 1 S3
    // EventSourceMapping, IAM roles = no cost model
    let lambda_count = arch
        .resources
        .iter()
        .filter(|r| r.label.starts_with("Lambda:"))
        .count();
    let sqs_count = arch
        .resources
        .iter()
        .filter(|r| r.label.contains("SQS"))
        .count();
    let log_count = arch
        .resources
        .iter()
        .filter(|r| r.label.starts_with("CloudWatch Logs:"))
        .count();
    let s3_count = arch
        .resources
        .iter()
        .filter(|r| r.label.starts_with("S3:"))
        .count();

    assert_eq!(lambda_count, 2, "should have 2 Lambda functions");
    assert_eq!(sqs_count, 2, "should have 2 SQS queues");
    assert_eq!(log_count, 2, "should have 2 LogGroups");
    assert_eq!(s3_count, 1, "should have 1 S3 bucket");
}

#[test]
fn test_ingest_pipeline_cost_evaluation() {
    let tmpl =
        parser::parse_template(fixtures_dir().join("orders-ingest.sam.yml").as_ref()).unwrap();
    let params = load_params("params-prd.yaml");
    let imports = load_params("imports.yaml");
    let resources = parser::resolve_template(&tmpl, &params, &imports).unwrap();

    let arch = build_architecture_cost("ingest", &resources, (), false);
    let usage = load_usage("usage.yaml");
    let result = evaluate_architecture(&arch, &usage).unwrap();

    assert!(result.total_monthly_cost > 0.0);

    // OrderIngestFunction (5M requests, 256MB, 200ms) should be the most expensive Lambda
    let ingest = result
        .resources
        .iter()
        .find(|r| r.logical_id == "OrderIngestFunction")
        .unwrap();
    let backup = result
        .resources
        .iter()
        .find(|r| r.logical_id == "OrderBackupFunction")
        .unwrap();
    assert!(
        ingest.monthly_cost > backup.monthly_cost,
        "OrderIngestFunction (${:.2}) should cost more than OrderBackupFunction (${:.2})",
        ingest.monthly_cost,
        backup.monthly_cost
    );
}

#[test]
fn test_ingest_sam_memory_from_findinmap() {
    // prd MemorySize should be 256 (from Mappings)
    let tmpl =
        parser::parse_template(fixtures_dir().join("orders-ingest.sam.yml").as_ref()).unwrap();
    let prd_params = load_params("params-prd.yaml");
    let imports = load_params("imports.yaml");
    let prd_resources = parser::resolve_template(&tmpl, &prd_params, &imports).unwrap();
    let prd_arch = build_architecture_cost("ingest-prd", &prd_resources, (), false);

    // dev MemorySize should be 128 (from Mappings)
    let tmpl =
        parser::parse_template(fixtures_dir().join("orders-ingest.sam.yml").as_ref()).unwrap();
    let dev_params = load_params("params-dev.yaml");
    let dev_resources = parser::resolve_template(&tmpl, &dev_params, &imports).unwrap();
    let dev_arch = build_architecture_cost("ingest-dev", &dev_resources, (), false);

    // Use high-volume usage to exceed GB-seconds free tier (400K) for both envs.
    // 50M requests * 500ms * 0.25GB = 6.25M GB-seconds (prd, well over 400K free tier)
    // 50M requests * 500ms * 0.125GB = 3.125M GB-seconds (dev, also over free tier)
    let mut usage = load_usage("usage.yaml");
    usage.insert("OrderIngestFunction_requests".into(), 50_000_000.0);
    usage.insert("OrderIngestFunction_avg_duration_ms".into(), 500.0);

    let prd_result = evaluate_architecture(&prd_arch, &usage).unwrap();
    let dev_result = evaluate_architecture(&dev_arch, &usage).unwrap();

    // prd (256MB) should cost more than dev (128MB) for the same workload
    let prd_ingest = prd_result
        .resources
        .iter()
        .find(|r| r.logical_id == "OrderIngestFunction")
        .unwrap();
    let dev_ingest = dev_result
        .resources
        .iter()
        .find(|r| r.logical_id == "OrderIngestFunction")
        .unwrap();
    assert!(
        prd_ingest.monthly_cost > dev_ingest.monthly_cost,
        "prd 256MB (${:.2}) should cost more than dev 128MB (${:.2})",
        prd_ingest.monthly_cost,
        dev_ingest.monthly_cost
    );
}

// =========================================================================
// catalog-indexing.sam: DynamoDB Stream -> Lambda -> AOSS + SQS -> Lambda
// =========================================================================
#[test]
fn test_indexing_pipeline_resource_count() {
    let tmpl =
        parser::parse_template(fixtures_dir().join("catalog-indexing.sam.yml").as_ref()).unwrap();
    let params = load_params("params-prd.yaml");
    let imports = load_params("imports.yaml");
    let resources = parser::resolve_template(&tmpl, &params, &imports).unwrap();

    let arch = build_architecture_cost("indexing", &resources, (), false);

    // Expected: 2 Lambda(Serverless) + 2 LogGroup
    let lambda_count = arch
        .resources
        .iter()
        .filter(|r| r.label.starts_with("Lambda:"))
        .count();
    let log_count = arch
        .resources
        .iter()
        .filter(|r| r.label.starts_with("CloudWatch Logs:"))
        .count();

    assert_eq!(lambda_count, 2, "should have 2 Lambda functions");
    assert_eq!(log_count, 2, "should have 2 LogGroups");
}

// =========================================================================
// Full stack: all templates combined via compare
// =========================================================================
#[test]
fn test_full_stack_prd_vs_dev_comparison() {
    let imports = load_params("imports.yaml");
    let usage = load_usage("usage.yaml");

    let templates = &[
        "shared-dynamodb.yml",
        "shared-sqs.yml",
        "shared-s3.yml",
        "orders-kinesis.yml",
        "catalog-aoss.yml",
    ];

    let mut prd_total = 0.0;
    let mut dev_total = 0.0;

    for tmpl_name in templates {
        let tmpl = parser::parse_template(fixtures_dir().join(tmpl_name).as_ref()).unwrap();

        let prd_params = load_params("params-prd.yaml");
        let prd_res = parser::resolve_template(&tmpl, &prd_params, &imports).unwrap();
        let prd_arch = build_architecture_cost(tmpl_name, &prd_res, (), false);
        if let Ok(r) = evaluate_architecture(&prd_arch, &usage) {
            prd_total += r.total_monthly_cost;
        }

        let tmpl = parser::parse_template(fixtures_dir().join(tmpl_name).as_ref()).unwrap();
        let dev_params = load_params("params-dev.yaml");
        let dev_res = parser::resolve_template(&tmpl, &dev_params, &imports).unwrap();
        let dev_arch = build_architecture_cost(tmpl_name, &dev_res, (), false);
        if let Ok(r) = evaluate_architecture(&dev_arch, &usage) {
            dev_total += r.total_monthly_cost;
        }
    }

    // prd should cost more than dev (at least the Kinesis shard difference)
    assert!(
        prd_total > dev_total,
        "prd total (${prd_total:.2}) should be > dev total (${dev_total:.2})"
    );

    // Both should be non-trivial
    assert!(prd_total > 100.0, "prd total should be > $100");
    assert!(dev_total > 100.0, "dev total should be > $100");
}

// =========================================================================
// Hierarchical usage YAML: same results as flat format
// =========================================================================
#[test]
fn test_hierarchical_usage_produces_same_results() {
    let tmpl = parser::parse_template(fixtures_dir().join("shared-dynamodb.yml").as_ref()).unwrap();
    let params = load_params("params-prd.yaml");
    let resources = parser::resolve_template(&tmpl, &params, &HashMap::new()).unwrap();
    let arch = build_architecture_cost("test", &resources, (), false);

    let flat_usage = load_usage("usage.yaml");
    let hierarchical_usage = load_usage("usage-hierarchical.yaml");

    let flat_result = evaluate_architecture(&arch, &flat_usage).unwrap();
    let hier_result = evaluate_architecture(&arch, &hierarchical_usage).unwrap();

    assert!(
        (flat_result.total_monthly_cost - hier_result.total_monthly_cost).abs() < 0.01,
        "flat (${:.2}) and hierarchical (${:.2}) should produce same results",
        flat_result.total_monthly_cost,
        hier_result.total_monthly_cost
    );
}

// =========================================================================
// Schema generation
// =========================================================================
#[test]
fn test_schema_generation() {
    use yevice_core::schema::generate_usage_schema;

    let tmpl =
        parser::parse_template(fixtures_dir().join("orders-ingest.sam.yml").as_ref()).unwrap();
    let params = load_params("params-prd.yaml");
    let imports = load_params("imports.yaml");
    let resources = parser::resolve_template(&tmpl, &params, &imports).unwrap();
    let arch = build_architecture_cost("test", &resources, (), false);

    let schema = generate_usage_schema(&arch);

    // Should have properties for each resource with non-bound variables.
    // OrderIngestFunction_requests is a binding target (Kinesis EventSourceMapping)
    // so OrderIngestFunction still appears because it has avg_duration_ms and
    // data_transfer_out_gb, but requests is excluded.
    assert!(
        schema.properties.contains_key("OrderIngestFunction"),
        "schema should contain OrderIngestFunction"
    );
    // OrderBackupFunction_requests is a binding target (SQS EventSourceMapping)
    // so OrderBackupFunction still appears because it has avg_duration_ms and
    // data_transfer_out_gb, but requests is excluded.
    assert!(
        schema.properties.contains_key("OrderBackupFunction"),
        "schema should contain OrderBackupFunction"
    );

    // OrderIngestFunction should require avg_duration_ms and data_transfer_out_gb
    // (requests is a binding target and must be excluded).
    let ingest = &schema.properties["OrderIngestFunction"];
    assert!(
        !ingest.properties.contains_key("requests"),
        "requests is a binding target and must be excluded from schema"
    );
    assert!(ingest.properties.contains_key("avg_duration_ms"));
    assert!(ingest.properties.contains_key("data_transfer_out_gb"));
    assert_eq!(ingest.required.len(), 2);
}

#[test]
fn test_template_generation() {
    use yevice_core::schema::generate_usage_template;

    let tmpl =
        parser::parse_template(fixtures_dir().join("orders-ingest.sam.yml").as_ref()).unwrap();
    let params = load_params("params-prd.yaml");
    let imports = load_params("imports.yaml");
    let resources = parser::resolve_template(&tmpl, &params, &imports).unwrap();
    let arch = build_architecture_cost("test", &resources, (), false);

    let template = generate_usage_template(&arch);

    // Should contain resource sections
    assert!(template.contains("OrderIngestFunction:"));
    assert!(template.contains("  avg_duration_ms: 0"));
    assert!(template.contains("OrderBackupFunction:"));
}

// =========================================================================
// Variable bindings: EventSourceMapping auto-detection
// =========================================================================
#[test]
fn test_sqs_lambda_binding_detected() {
    let tmpl =
        parser::parse_template(fixtures_dir().join("orders-ingest.sam.yml").as_ref()).unwrap();
    let params = load_params("params-prd.yaml");
    let imports = load_params("imports.yaml");
    let resources = parser::resolve_template(&tmpl, &params, &imports).unwrap();
    let arch = build_architecture_cost("test", &resources, (), false);

    // Should have detected SQS -> OrderBackupFunction binding
    let sqs_binding = arch
        .bindings
        .iter()
        .find(|b| b.target == "OrderBackupFunction_requests");
    assert!(
        sqs_binding.is_some(),
        "should detect SQS -> Lambda binding for OrderBackupFunction"
    );
    assert!(sqs_binding.unwrap().source.contains("SQS -> Lambda"));
}

#[test]
fn test_binding_derives_lambda_requests_from_sqs() {
    let tmpl =
        parser::parse_template(fixtures_dir().join("orders-ingest.sam.yml").as_ref()).unwrap();
    let params = load_params("params-prd.yaml");
    let imports = load_params("imports.yaml");
    let resources = parser::resolve_template(&tmpl, &params, &imports).unwrap();
    let arch = build_architecture_cost("test", &resources, (), false);

    // Provide SQS requests but NOT OrderBackupFunction_requests
    let mut usage = Params::new();
    usage.insert(VariableName::new("FailedOrderQueue_requests"), 10000.0);
    usage.insert(VariableName::new("FailedOrderDLQ_requests"), 0.0);
    usage.insert(
        VariableName::new("OrderBackupFunction_avg_duration_ms"),
        500.0,
    );
    usage.insert(
        VariableName::new("OrderBackupFunction_data_transfer_out_gb"),
        0.0,
    );
    usage.insert(
        VariableName::new("OrderBackupFunctionLogGroup_ingestion_gb"),
        0.0,
    );
    usage.insert(
        VariableName::new("OrderBackupFunctionLogGroup_storage_gb"),
        0.0,
    );
    usage.insert(VariableName::new("DeadLetterBackupBucket_storage_gb"), 0.0);
    usage.insert(
        VariableName::new("DeadLetterBackupBucket_put_requests"),
        0.0,
    );
    usage.insert(
        VariableName::new("DeadLetterBackupBucket_get_requests"),
        0.0,
    );
    // Kinesis source for OrderIngestFunction binding
    usage.insert(VariableName::new("stream/orders_put_records"), 1000.0);
    usage.insert(
        VariableName::new("OrderIngestFunction_avg_duration_ms"),
        200.0,
    );
    usage.insert(
        VariableName::new("OrderIngestFunction_data_transfer_out_gb"),
        0.0,
    );
    usage.insert(
        VariableName::new("OrderIngestFunctionLogGroup_ingestion_gb"),
        0.0,
    );
    usage.insert(
        VariableName::new("OrderIngestFunctionLogGroup_storage_gb"),
        0.0,
    );

    // Should succeed — OrderBackupFunction_requests derived from FailedOrderQueue_requests / batch_size
    let result = evaluate_architecture(&arch, &usage);
    assert!(
        result.is_ok(),
        "should derive OrderBackupFunction_requests from binding: {result:?}"
    );

    let result = result.unwrap();
    let backup = result
        .resources
        .iter()
        .find(|r| r.logical_id == "OrderBackupFunction")
        .unwrap();
    // 10000 SQS messages / batch_size 1 = 10000 Lambda invocations
    // All within free tier (1M) so cost should be ~0 for requests
    assert!(backup.monthly_cost >= 0.0);
}

#[test]
fn test_binding_can_be_overridden_by_explicit_param() {
    let tmpl =
        parser::parse_template(fixtures_dir().join("orders-ingest.sam.yml").as_ref()).unwrap();
    let params = load_params("params-prd.yaml");
    let imports = load_params("imports.yaml");
    let resources = parser::resolve_template(&tmpl, &params, &imports).unwrap();
    let arch = build_architecture_cost("test", &resources, (), false);

    let mut usage = Params::new();
    usage.insert(VariableName::new("FailedOrderQueue_requests"), 10000.0);
    usage.insert(VariableName::new("FailedOrderDLQ_requests"), 0.0);
    // Explicitly override with 5M requests (well above 1M free tier)
    usage.insert(
        VariableName::new("OrderBackupFunction_requests"),
        5_000_000.0,
    );
    usage.insert(
        VariableName::new("OrderBackupFunction_avg_duration_ms"),
        500.0,
    );
    usage.insert(
        VariableName::new("OrderBackupFunction_data_transfer_out_gb"),
        0.0,
    );
    usage.insert(
        VariableName::new("OrderBackupFunctionLogGroup_ingestion_gb"),
        0.0,
    );
    usage.insert(
        VariableName::new("OrderBackupFunctionLogGroup_storage_gb"),
        0.0,
    );
    usage.insert(VariableName::new("DeadLetterBackupBucket_storage_gb"), 0.0);
    usage.insert(
        VariableName::new("DeadLetterBackupBucket_put_requests"),
        0.0,
    );
    usage.insert(
        VariableName::new("DeadLetterBackupBucket_get_requests"),
        0.0,
    );
    usage.insert(VariableName::new("stream/orders_put_records"), 1000.0);
    usage.insert(
        VariableName::new("OrderIngestFunction_avg_duration_ms"),
        200.0,
    );
    usage.insert(
        VariableName::new("OrderIngestFunction_data_transfer_out_gb"),
        0.0,
    );
    usage.insert(
        VariableName::new("OrderIngestFunctionLogGroup_ingestion_gb"),
        0.0,
    );
    usage.insert(
        VariableName::new("OrderIngestFunctionLogGroup_storage_gb"),
        0.0,
    );

    let result_with_override = evaluate_architecture(&arch, &usage).unwrap();

    // Without override — binding derives 10000 from SQS
    usage.remove(&VariableName::new("OrderBackupFunction_requests"));
    let result_without = evaluate_architecture(&arch, &usage).unwrap();

    let cost_with = result_with_override
        .resources
        .iter()
        .find(|r| r.logical_id == "OrderBackupFunction")
        .unwrap()
        .monthly_cost;
    let cost_without = result_without
        .resources
        .iter()
        .find(|r| r.logical_id == "OrderBackupFunction")
        .unwrap()
        .monthly_cost;

    // 5M requests (above free tier) > 10000 (within free tier) => cost_with > cost_without
    assert!(
        cost_with > cost_without,
        "explicit override (5M) should cost more than binding-derived (10K): {cost_with} vs {cost_without}"
    );
}

// =========================================================================
// Batch scenario: StepFunctions -> Batch (Fargate+EBS) -> S3
// =========================================================================
#[test]
fn test_batch_job_resource_recognized() {
    let tmpl = parser::parse_template(fixtures_dir().join("batch-job.yml").as_ref()).unwrap();
    let resources = parser::resolve_template(&tmpl, &HashMap::new(), &HashMap::new()).unwrap();
    let arch = build_architecture_cost("batch", &resources, (), false);

    // Step Functions + Batch Job + S3 = 3 cost resources
    assert_eq!(arch.resources.len(), 3);

    let batch = arch
        .resources
        .iter()
        .find(|r| r.label.contains("Batch Job"));
    assert!(batch.is_some(), "should recognize Batch JobDefinition");
    assert!(batch.unwrap().label.contains("4vCPU"));
    assert!(batch.unwrap().label.contains("EBS 512GB"));
}

#[test]
fn test_batch_job_cost_calculation() {
    let tmpl = parser::parse_template(fixtures_dir().join("batch-job.yml").as_ref()).unwrap();
    let resources = parser::resolve_template(&tmpl, &HashMap::new(), &HashMap::new()).unwrap();
    let arch = build_architecture_cost("batch", &resources, (), false);

    let mut usage = Params::new();
    // 1000 executions, 220 sec each
    usage.insert(VariableName::new("BatchWorkflow_transitions"), 3000.0);
    usage.insert(VariableName::new("ProcessingJob_executions"), 1000.0);
    usage.insert(VariableName::new("ProcessingJob_avg_duration_sec"), 220.0);
    usage.insert(VariableName::new("OutputBucket_storage_gb"), 163.3);
    usage.insert(VariableName::new("OutputBucket_put_requests"), 1000.0);
    usage.insert(VariableName::new("OutputBucket_get_requests"), 1000.0);

    let result = evaluate_architecture(&arch, &usage).unwrap();

    // Batch Job should be the most expensive component
    let batch_cost = result
        .resources
        .iter()
        .find(|r| r.logical_id == "ProcessingJob")
        .unwrap()
        .monthly_cost;

    // Fargate: (4*0.05056 + 30*0.00553) * (220/3600) * 1000 = ~$22.50
    // EBS: (512*0.096 + 2000*0.006 + 875*0.048) / 730 * (220/3600) * 1000 = ~$8.64
    assert!(
        batch_cost > 20.0,
        "Batch cost should be > $20, got ${batch_cost:.2}"
    );
    assert!(
        batch_cost < 150.0,
        "Batch cost should be < $150, got ${batch_cost:.2}"
    );

    // S3 should be modest
    let s3_cost = result
        .resources
        .iter()
        .find(|r| r.logical_id == "OutputBucket")
        .unwrap()
        .monthly_cost;
    assert!(
        s3_cost > 1.0 && s3_cost < 20.0,
        "S3 cost should be $1-20, got ${s3_cost:.2}"
    );

    // Total
    assert!(result.total_monthly_cost > 25.0, "Total should be > $25");
}

// =========================================================================
// batch-scenario: examples/batch-scenario.yaml with user-defined bindings
// =========================================================================

/// YAML content equivalent to examples/batch-scenario-bindings.yaml.
const BATCH_SCENARIO_BINDINGS_YAML: &str = r#"
bindings:
  # Step Functions -> Batch Job (3 transitions per execution)
  - target: ProcessingJob_executions
    source: BatchWorkflow_transitions
    batch_size: 3
    description: "3 transitions per workflow = 1 batch job"

  # Batch Job -> S3 PUT (1 execution = 1 file)
  - target: OutputBucket_put_requests
    source: ProcessingJob_executions
    description: "1 batch job = 1 S3 PUT"

  # S3 storage = executions * avg_object_size_gb * retention_days / 30
  - target: OutputBucket_storage_gb
    expr: "ProcessingJob_executions * OutputBucket_avg_object_size_gb * OutputBucket_retention_days / 30"
    description: "Average S3 storage from file size and retention"
"#;

#[test]
fn test_batch_scenario_parse_and_resource_spec() {
    let tmpl = parser::parse_template(fixtures_dir().join("batch-scenario.yml").as_ref()).unwrap();
    let resources = parser::resolve_template(&tmpl, &HashMap::new(), &HashMap::new()).unwrap();
    let resolved_template = yevice_cfn::parser::CfnTemplate {
        parameters: HashMap::new(),
        mappings: HashMap::new(),
        conditions: HashMap::new(),
        resources,
    };
    let mut cfn_adapters = CfnAdapterRegistry::new();
    let mut tf_adapters = TfAdapterRegistry::new();
    yevice_services_aws::register(
        &mut ServiceCatalog::new(),
        &mut cfn_adapters,
        &mut tf_adapters,
    );
    let arch = convert::build_architecture(
        "batch-scenario",
        "ap-northeast-1",
        &resolved_template,
        &cfn_adapters,
    );

    // Template has 3 resources: BatchWorkflow (StepFunctions), ProcessingJob (Batch), OutputBucket (S3)
    assert_eq!(
        arch.resources.len(),
        3,
        "batch-scenario should have 3 resources"
    );

    let batch_job = arch
        .resources
        .iter()
        .find(|r| r.resource_type == "AWS::Batch::JobDefinition");
    assert!(
        batch_job.is_some(),
        "ProcessingJob should be recognized as BatchJobDefinition"
    );
    assert_eq!(
        batch_job.unwrap().logical_id,
        "ProcessingJob",
        "BatchJobDefinition logical id should be ProcessingJob"
    );

    let step_fn = arch
        .resources
        .iter()
        .find(|r| r.resource_type == "AWS::StepFunctions::StateMachine");
    assert!(
        step_fn.is_some(),
        "BatchWorkflow should be recognized as StepFunctions"
    );
    assert_eq!(
        step_fn.unwrap().logical_id,
        "BatchWorkflow",
        "StepFunctions logical id should be BatchWorkflow"
    );

    let s3 = arch
        .resources
        .iter()
        .find(|r| r.resource_type == "AWS::S3::Bucket");
    assert!(s3.is_some(), "OutputBucket should be recognized as S3");
    assert_eq!(
        s3.unwrap().logical_id,
        "OutputBucket",
        "S3 logical id should be OutputBucket"
    );
}

#[test]
fn test_batch_scenario_cost_model_generation() {
    let tmpl = parser::parse_template(fixtures_dir().join("batch-scenario.yml").as_ref()).unwrap();
    let resources = parser::resolve_template(&tmpl, &HashMap::new(), &HashMap::new()).unwrap();
    let arch_cost = build_architecture_cost("batch-scenario", &resources, (), false);

    // Should produce cost models for all 3 resources
    assert_eq!(
        arch_cost.resources.len(),
        3,
        "cost model should have 3 resource entries"
    );

    // Batch Job cost model should have compute and storage components
    let batch = arch_cost
        .resources
        .iter()
        .find(|r| r.logical_id == "ProcessingJob")
        .expect("ProcessingJob cost model should exist");

    assert!(
        batch.label.contains("Batch Job"),
        "Batch Job label should contain 'Batch Job', got: {}",
        batch.label
    );
    assert!(
        batch.label.contains("4vCPU"),
        "Batch Job label should contain '4vCPU', got: {}",
        batch.label
    );
    assert!(
        batch.label.contains("EBS 512GB"),
        "Batch Job label should mention EBS volume, got: {}",
        batch.label
    );

    // Batch Job should have exactly 2 cost components: compute and storage
    assert_eq!(
        batch.components.len(),
        2,
        "Batch Job should have compute and storage components"
    );
    let component_names: Vec<&str> = batch.components.iter().map(|c| c.name.as_str()).collect();
    assert!(
        component_names.iter().any(|n| n.contains("Compute")),
        "Batch Job should have a Compute component, got: {component_names:?}"
    );
    assert!(
        component_names
            .iter()
            .any(|n| n.contains("EBS") || n.contains("Storage")),
        "Batch Job should have a Storage/EBS component, got: {component_names:?}"
    );

    // StepFunctions should be present with Standard label
    let sfn = arch_cost
        .resources
        .iter()
        .find(|r| r.logical_id == "BatchWorkflow")
        .expect("BatchWorkflow cost model should exist");
    assert!(
        sfn.label.contains("Step Functions"),
        "StepFunctions label should contain 'Step Functions', got: {}",
        sfn.label
    );

    // S3 bucket should be present
    let s3 = arch_cost
        .resources
        .iter()
        .find(|r| r.logical_id == "OutputBucket")
        .expect("OutputBucket cost model should exist");
    assert!(
        s3.label.contains("S3"),
        "S3 label should contain 'S3', got: {}",
        s3.label
    );
}

#[test]
fn test_batch_scenario_evaluation_with_user_bindings() {
    let tmpl = parser::parse_template(fixtures_dir().join("batch-scenario.yml").as_ref()).unwrap();
    let resources = parser::resolve_template(&tmpl, &HashMap::new(), &HashMap::new()).unwrap();
    let mut arch_cost = build_architecture_cost("batch-scenario", &resources, (), false);

    // Apply user-defined bindings from batch-scenario-bindings.yaml
    let bindings_file: BindingsFile =
        serde_yaml_ng::from_str(BATCH_SCENARIO_BINDINGS_YAML).unwrap();
    let user_bindings = to_variable_bindings(&bindings_file.bindings);
    arch_cost.bindings.extend(user_bindings);

    // Usage parameters from batch-scenario-usage.yaml (root metrics only)
    let mut params = Params::new();
    params.insert(VariableName::new("BatchWorkflow_transitions"), 3000.0);
    params.insert(VariableName::new("ProcessingJob_avg_duration_sec"), 220.0);
    params.insert(VariableName::new("OutputBucket_avg_object_size_gb"), 0.7);
    params.insert(VariableName::new("OutputBucket_retention_days"), 7.0);
    params.insert(VariableName::new("OutputBucket_get_requests"), 1000.0);

    let result = evaluate_architecture(&arch_cost, &params)
        .expect("evaluation with user bindings should succeed");

    // Verify that evaluation produced results for all 3 resources
    assert_eq!(
        result.resources.len(),
        3,
        "result should have 3 resource cost entries"
    );

    // Total cost should be positive (sanity check)
    assert!(
        result.total_monthly_cost > 0.0,
        "total monthly cost should be positive, got: {}",
        result.total_monthly_cost
    );

    // Batch Job (ProcessingJob) should be the most expensive resource:
    // 1000 executions * 220s = Fargate + EBS cost, roughly $20-$150
    let batch_cost = result
        .resources
        .iter()
        .find(|r| r.logical_id == "ProcessingJob")
        .unwrap()
        .monthly_cost;
    assert!(
        batch_cost > 20.0,
        "ProcessingJob cost should be > $20 (1000 executions x 220s), got: ${batch_cost:.2}"
    );
    assert!(
        batch_cost < 150.0,
        "ProcessingJob cost should be < $150, got: ${batch_cost:.2}"
    );

    // S3 cost should be modest (163 GB storage + 1000 PUT + 1000 GET)
    let s3_cost = result
        .resources
        .iter()
        .find(|r| r.logical_id == "OutputBucket")
        .unwrap()
        .monthly_cost;
    assert!(
        s3_cost > 1.0 && s3_cost < 20.0,
        "OutputBucket cost should be $1-20, got: ${s3_cost:.2}"
    );

    // Total should be greater than batch alone
    assert!(
        result.total_monthly_cost > batch_cost,
        "total (${:.2}) should exceed batch-only cost (${:.2})",
        result.total_monthly_cost,
        batch_cost
    );
}

#[test]
fn test_batch_scenario_bindings_derive_correct_values() {
    let tmpl = parser::parse_template(fixtures_dir().join("batch-scenario.yml").as_ref()).unwrap();
    let resources = parser::resolve_template(&tmpl, &HashMap::new(), &HashMap::new()).unwrap();
    let mut arch_cost = build_architecture_cost("batch-scenario", &resources, (), false);

    // Apply user-defined bindings
    let bindings_file: BindingsFile =
        serde_yaml_ng::from_str(BATCH_SCENARIO_BINDINGS_YAML).unwrap();
    let user_bindings = to_variable_bindings(&bindings_file.bindings);
    arch_cost.bindings.extend(user_bindings);

    // 3 user bindings should have been added
    // (auto-detect bindings may be 0 for batch-scenario since there are no EventSourceMappings)
    let user_binding_count = arch_cost
        .bindings
        .iter()
        .filter(|b| b.source.contains("user-defined"))
        .count();
    assert_eq!(
        user_binding_count, 3,
        "should have 3 user-defined bindings, got: {user_binding_count}"
    );

    // Usage: only root metrics (derived values should come from bindings)
    // BatchWorkflow_transitions=3000 => ProcessingJob_executions = ceil(3000/3) = 1000
    // ProcessingJob_executions=1000 => OutputBucket_put_requests = 1000
    // executions=1000, avg_object_size_gb=0.7, retention_days=7 => storage = 1000*0.7*7/30 ≈ 163.33
    let mut params = Params::new();
    params.insert(VariableName::new("BatchWorkflow_transitions"), 3000.0);
    params.insert(VariableName::new("ProcessingJob_avg_duration_sec"), 220.0);
    params.insert(VariableName::new("OutputBucket_avg_object_size_gb"), 0.7);
    params.insert(VariableName::new("OutputBucket_retention_days"), 7.0);
    params.insert(VariableName::new("OutputBucket_get_requests"), 1000.0);

    // Compare against baseline where we explicitly provide all derived values
    let mut explicit_params = params.clone();
    explicit_params.insert(VariableName::new("ProcessingJob_executions"), 1000.0);
    explicit_params.insert(VariableName::new("OutputBucket_put_requests"), 1000.0);
    // 1000 * 0.7 * 7 / 30 = 163.333...
    explicit_params.insert(
        VariableName::new("OutputBucket_storage_gb"),
        163.333_333_333_333_33,
    );

    let result_bindings = evaluate_architecture(&arch_cost, &params)
        .expect("evaluation with user bindings should succeed");
    let result_explicit = evaluate_architecture(&arch_cost, &explicit_params)
        .expect("evaluation with explicit params should succeed");

    // Both evaluations should produce the same total cost (bindings derive the same values)
    assert!(
        (result_bindings.total_monthly_cost - result_explicit.total_monthly_cost).abs() < 0.01,
        "binding-derived cost (${:.4}) should match explicitly-provided cost (${:.4})",
        result_bindings.total_monthly_cost,
        result_explicit.total_monthly_cost
    );
}

#[test]
fn test_batch_scenario_bindings_overridable() {
    let tmpl = parser::parse_template(fixtures_dir().join("batch-scenario.yml").as_ref()).unwrap();
    let resources = parser::resolve_template(&tmpl, &HashMap::new(), &HashMap::new()).unwrap();
    let mut arch_cost = build_architecture_cost("batch-scenario", &resources, (), false);

    let bindings_file: BindingsFile =
        serde_yaml_ng::from_str(BATCH_SCENARIO_BINDINGS_YAML).unwrap();
    let user_bindings = to_variable_bindings(&bindings_file.bindings);
    arch_cost.bindings.extend(user_bindings);

    // Base: derived executions = 1000 (3000 / 3)
    let mut base_params = Params::new();
    base_params.insert(VariableName::new("BatchWorkflow_transitions"), 3000.0);
    base_params.insert(VariableName::new("ProcessingJob_avg_duration_sec"), 220.0);
    base_params.insert(VariableName::new("OutputBucket_avg_object_size_gb"), 0.7);
    base_params.insert(VariableName::new("OutputBucket_retention_days"), 7.0);
    base_params.insert(VariableName::new("OutputBucket_get_requests"), 1000.0);

    // Override: provide 5000 executions explicitly (much more expensive)
    let mut override_params = base_params.clone();
    override_params.insert(VariableName::new("ProcessingJob_executions"), 5000.0);
    // Also override dependent derived values consistently
    override_params.insert(VariableName::new("OutputBucket_put_requests"), 5000.0);
    override_params.insert(VariableName::new("OutputBucket_storage_gb"), 816.67);

    let result_base =
        evaluate_architecture(&arch_cost, &base_params).expect("base evaluation should succeed");
    let result_override = evaluate_architecture(&arch_cost, &override_params)
        .expect("override evaluation should succeed");

    let batch_base = result_base
        .resources
        .iter()
        .find(|r| r.logical_id == "ProcessingJob")
        .unwrap()
        .monthly_cost;
    let batch_override = result_override
        .resources
        .iter()
        .find(|r| r.logical_id == "ProcessingJob")
        .unwrap()
        .monthly_cost;

    // 5000 executions should cost ~5x more than 1000 executions
    assert!(
        batch_override > batch_base * 4.0,
        "5000 executions (${batch_override:.2}) should cost >4x more than 1000 (${batch_base:.2})"
    );
}
