//! Integration tests for the Terraform parser + service plugin pipeline.

use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use yevice_core::resource::{Architecture, ConnectionType, Provider, Resource};
use yevice_pricing::gcp_hardcoded_pricing;
use yevice_service_api::{
    CfnAdapterRegistry, MultiProviderCatalog, ServiceCatalog, TfAdapterRegistry,
};
use yevice_services_aws::{
    AwsPricingCatalog,
    services::{
        dynamodb::{DynamoDbBillingMode, DynamoDbSpec},
        ec2::Ec2Spec,
        ecs_fargate::EcsFargateSpec,
        kinesis::{KinesisSpec, KinesisStreamMode},
        lambda::LambdaSpec,
        s3::S3Spec,
    },
};
use yevice_services_gcp::{
    GcpPricingCatalog,
    services::{cloud_function::GcpCloudFunctionSpec, cloud_sql::GcpCloudSqlSpec},
};
use yevice_tf::{convert, parser, resolver};

// ---------------------------------------------------------------------------
// Fixture management — parse_tf_dir reads every .tf file in a directory, so
// each test gets its own scratch dir populated with only the fixtures it needs.
// ---------------------------------------------------------------------------

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

struct FixtureDir {
    path: PathBuf,
}

impl FixtureDir {
    fn new(name: &str, files: &[&str]) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock drift")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("yevice-tf-{name}-{unique}"));
        std::fs::create_dir_all(&path).unwrap();
        for file in files {
            std::fs::copy(fixtures_dir().join(file), path.join(file)).unwrap();
        }
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for FixtureDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

// ---------------------------------------------------------------------------
// Registry builders
// ---------------------------------------------------------------------------

fn aws_registries() -> (ServiceCatalog, TfAdapterRegistry) {
    let mut catalog = ServiceCatalog::new();
    let mut cfn = CfnAdapterRegistry::new();
    let mut tf = TfAdapterRegistry::new();
    yevice_services_aws::register(&mut catalog, &mut cfn, &mut tf);
    (catalog, tf)
}

fn aws_and_gcp_registries() -> (ServiceCatalog, TfAdapterRegistry) {
    let mut catalog = ServiceCatalog::new();
    let mut cfn = CfnAdapterRegistry::new();
    let mut tf = TfAdapterRegistry::new();
    yevice_services_aws::register(&mut catalog, &mut cfn, &mut tf);
    yevice_services_gcp::register(&mut catalog, &mut tf);
    (catalog, tf)
}

fn resource<'a>(arch: &'a Architecture, logical_id: &str) -> &'a Resource {
    arch.resources
        .iter()
        .find(|r| r.logical_id.as_str() == logical_id)
        .unwrap_or_else(|| panic!("resource {logical_id} not found"))
}

// ---------------------------------------------------------------------------
// Parser-only tests
// ---------------------------------------------------------------------------

#[test]
fn parses_simple_lambda_directory() {
    let dir = FixtureDir::new("simple-parse", &["simple_lambda.tf"]);
    let config = parser::parse_tf_dir(dir.path()).unwrap();

    assert_eq!(config.resources.len(), 3);
    assert_eq!(
        config
            .resources
            .iter()
            .filter(|r| r.resource_type == "aws_lambda_function")
            .count(),
        1
    );
}

// ---------------------------------------------------------------------------
// Spec extraction tests — verify TF resources convert into the expected
// service spec via the AWS/GCP TF adapters.
// ---------------------------------------------------------------------------

#[test]
fn lambda_uses_literal_defaults_from_tf() {
    let dir = FixtureDir::new("lambda-defaults", &["simple_lambda.tf"]);
    let config = parser::parse_tf_dir(dir.path()).unwrap();
    let resolved = resolver::resolve_config(config, None).unwrap();
    let (_, tf) = aws_registries();
    let arch = convert::build_architecture("test", "ap-northeast-1", &resolved, &tf);

    let lambda = resource(&arch, "aws_lambda_function_handler");
    let spec: LambdaSpec = lambda.shell.decode().unwrap();
    assert_eq!(spec.memory_mb, 512.0);
    assert_eq!(spec.timeout_sec, 30.0);
    assert_eq!(spec.runtime.as_deref(), Some("python3.12"));
}

#[test]
fn variables_resolve_with_declared_defaults() {
    let dir = FixtureDir::new("vars-defaults", &["variables.tf"]);
    let config = parser::parse_tf_dir(dir.path()).unwrap();
    let resolved = resolver::resolve_config(config, None).unwrap();
    let (_, tf) = aws_registries();
    let arch = convert::build_architecture("test", "ap-northeast-1", &resolved, &tf);

    let lambda: LambdaSpec = resource(&arch, "aws_lambda_function_handler")
        .shell
        .decode()
        .unwrap();
    assert_eq!(lambda.memory_mb, 256.0);
    assert_eq!(lambda.timeout_sec, 10.0);
    assert_eq!(lambda.runtime.as_deref(), Some("nodejs20.x"));

    let ec2: Ec2Spec = resource(&arch, "aws_instance_server")
        .shell
        .decode()
        .unwrap();
    assert_eq!(ec2.instance_type, "t3.micro");
}

#[test]
fn tfvars_override_variable_defaults() {
    let dir = FixtureDir::new("vars-override", &["variables.tf"]);
    let config = parser::parse_tf_dir(dir.path()).unwrap();
    let tfvars = parser::parse_tfvars(&fixtures_dir().join("terraform.tfvars")).unwrap();
    let resolved = resolver::resolve_config(config, Some(tfvars)).unwrap();
    let (_, tf) = aws_registries();
    let arch = convert::build_architecture("test", "ap-northeast-1", &resolved, &tf);

    let lambda: LambdaSpec = resource(&arch, "aws_lambda_function_handler")
        .shell
        .decode()
        .unwrap();
    assert_eq!(lambda.memory_mb, 1024.0);

    let ec2: Ec2Spec = resource(&arch, "aws_instance_server")
        .shell
        .decode()
        .unwrap();
    assert_eq!(ec2.instance_type, "m5.large");
}

#[test]
fn dynamodb_pay_per_request_decodes_as_on_demand() {
    let dir = FixtureDir::new("ddb", &["simple_lambda.tf"]);
    let config = parser::parse_tf_dir(dir.path()).unwrap();
    let resolved = resolver::resolve_config(config, None).unwrap();
    let (_, tf) = aws_registries();
    let arch = convert::build_architecture("test", "ap-northeast-1", &resolved, &tf);

    let spec: DynamoDbSpec = resource(&arch, "aws_dynamodb_table_data")
        .shell
        .decode()
        .unwrap();
    assert!(matches!(spec.billing_mode, DynamoDbBillingMode::OnDemand));
}

#[test]
fn multi_resource_converts_s3_kinesis_and_ecs() {
    let dir = FixtureDir::new("multi", &["multi_resource.tf"]);
    let config = parser::parse_tf_dir(dir.path()).unwrap();
    let resolved = resolver::resolve_config(config, None).unwrap();
    let (_, tf) = aws_registries();
    let arch = convert::build_architecture("test", "ap-northeast-1", &resolved, &tf);

    let s3: S3Spec = resource(&arch, "aws_s3_bucket_storage")
        .shell
        .decode()
        .unwrap();
    assert!(s3.versioning_enabled);

    let kinesis: KinesisSpec = resource(&arch, "aws_kinesis_stream_events")
        .shell
        .decode()
        .unwrap();
    assert_eq!(kinesis.retention_hours, 48.0);
    assert!(matches!(
        kinesis.stream_mode,
        KinesisStreamMode::Provisioned {
            shard_count: Some(4.0)
        }
    ));

    let ecs: EcsFargateSpec = resource(&arch, "aws_ecs_service_api")
        .shell
        .decode()
        .unwrap();
    assert_eq!(ecs.desired_count, Some(3.0));
}

// ---------------------------------------------------------------------------
// End-to-end cost-model generation tests
// ---------------------------------------------------------------------------

#[test]
fn aws_cost_model_builds_from_multiple_tf_files() {
    let dir = FixtureDir::new("aws-cost", &["simple_lambda.tf", "multi_resource.tf"]);
    let config = parser::parse_tf_dir(dir.path()).unwrap();
    let resolved = resolver::resolve_config(config, None).unwrap();
    let (catalog, tf) = aws_registries();
    let arch = convert::build_architecture("test", "ap-northeast-1", &resolved, &tf);

    let pricing = AwsPricingCatalog::new("ap-northeast-1");
    let cost = catalog.build_cost_model(&arch, &pricing, true).unwrap();

    assert!(!cost.resources.is_empty());
    // serialisation round-trip must succeed
    let _json = serde_json::to_string(&cost).unwrap();
}

// ---------------------------------------------------------------------------
// GCP-specific tests
// ---------------------------------------------------------------------------

#[test]
fn gcp_cloud_function_decodes_with_memory_and_generation() {
    let dir = FixtureDir::new("gcp-function", &["gcp_simple.tf"]);
    let config = parser::parse_tf_dir(dir.path()).unwrap();
    let resolved = resolver::resolve_config(config, None).unwrap();
    let (_, tf) = aws_and_gcp_registries();
    let arch = convert::build_architecture("gcp-test", "asia-northeast1", &resolved, &tf);

    let function = arch
        .resources
        .iter()
        .find(|r| r.shell.service_id == "gcp.cloud_function")
        .expect("expected a GcpCloudFunction resource");
    let spec: GcpCloudFunctionSpec = function.shell.decode().unwrap();
    assert_eq!(spec.generation, 2);
    assert_eq!(spec.memory_mb, 512.0);
}

#[test]
fn gcp_sql_decodes_regional_ha_tier() {
    let dir = FixtureDir::new("gcp-sql", &["gcp_simple.tf"]);
    let config = parser::parse_tf_dir(dir.path()).unwrap();
    let resolved = resolver::resolve_config(config, None).unwrap();
    let (_, tf) = aws_and_gcp_registries();
    let arch = convert::build_architecture("gcp-test", "asia-northeast1", &resolved, &tf);

    let sql = arch
        .resources
        .iter()
        .find(|r| r.shell.service_id == "gcp.cloud_sql")
        .expect("expected a GcpCloudSql resource");
    let spec: GcpCloudSqlSpec = sql.shell.decode().unwrap();
    assert_eq!(spec.tier, "db-n1-standard-2");
    assert_eq!(spec.availability_type, "REGIONAL");
}

#[test]
fn gcp_cost_model_builds_from_tf_fixture() {
    let dir = FixtureDir::new("gcp-cost", &["gcp_simple.tf"]);
    let config = parser::parse_tf_dir(dir.path()).unwrap();
    let resolved = resolver::resolve_config(config, None).unwrap();
    let (catalog, tf) = aws_and_gcp_registries();
    let arch = convert::build_architecture("gcp-test", "asia-northeast1", &resolved, &tf);

    let pricing = GcpPricingCatalog(gcp_hardcoded_pricing("asia-northeast1"));
    let cost = catalog.build_cost_model(&arch, &pricing, true).unwrap();

    assert!(
        !cost.resources.is_empty(),
        "expected at least one GCP cost resource"
    );
    let _json = serde_json::to_string(&cost).unwrap();
}

// ---------------------------------------------------------------------------
// Mixed-provider test — AWS + GCP resources in one architecture
// ---------------------------------------------------------------------------

#[test]
fn mixed_provider_cost_model_prices_both_aws_and_gcp_resources() {
    // Combine both fixture files: simple_lambda.tf (AWS) + gcp_simple.tf (GCP)
    let dir = FixtureDir::new("mixed-cost", &["simple_lambda.tf", "gcp_simple.tf"]);
    let config = parser::parse_tf_dir(dir.path()).unwrap();
    let resolved = resolver::resolve_config(config, None).unwrap();
    let (catalog, tf) = aws_and_gcp_registries();
    // Use "ap-northeast-1" as the region; the GCP fixture is region-agnostic
    // for cost model purposes since we override it with a hardcoded catalog.
    let arch = convert::build_architecture("mixed-test", "ap-northeast-1", &resolved, &tf);

    assert!(arch.has_provider(Provider::Aws), "expected AWS resources");
    assert!(arch.has_provider(Provider::Gcp), "expected GCP resources");

    let pricing = MultiProviderCatalog::new()
        .with(
            Provider::Aws,
            Box::new(AwsPricingCatalog::new("ap-northeast-1")),
        )
        .with(
            Provider::Gcp,
            Box::new(GcpPricingCatalog(gcp_hardcoded_pricing("asia-northeast1"))),
        );

    let cost = catalog
        .build_cost_model(&arch, &pricing, true)
        .expect("mixed-provider cost model should succeed");

    let aws_resources: Vec<_> = cost
        .resources
        .iter()
        .filter(|r| r.label.as_str().contains("lambda") || r.label.as_str().contains("Lambda"))
        .collect();
    let gcp_resources: Vec<_> = cost
        .resources
        .iter()
        .filter(|r| {
            r.label.as_str().contains("Cloud")
                || r.label.as_str().contains("BigQuery")
                || r.label.as_str().contains("Pub/Sub")
        })
        .collect();

    assert!(
        !aws_resources.is_empty(),
        "expected at least one AWS resource cost"
    );
    assert!(
        !gcp_resources.is_empty(),
        "expected at least one GCP resource cost"
    );

    // Serialisation round-trip must succeed
    let _json = serde_json::to_string(&cost).unwrap();
}

// ---------------------------------------------------------------------------
// Cross-resource reference (ResourceRef) and connections tests
// ---------------------------------------------------------------------------

#[test]
fn event_source_mapping_produces_event_source_edge() {
    // cross_resource.tf has:
    //   aws_sqs_queue.input_queue
    //   aws_dynamodb_table.state_table
    //   aws_lambda_function.processor  (refs state_table via environment)
    //   aws_lambda_event_source_mapping.sqs_trigger  (sqs → processor)
    let dir = FixtureDir::new("cross-resource", &["cross_resource.tf"]);
    let config = parser::parse_tf_dir(dir.path()).unwrap();
    let resolved = resolver::resolve_config(config, None).unwrap();
    let (_, tf) = aws_registries();
    let arch = convert::build_architecture("test", "ap-northeast-1", &resolved, &tf);

    // EventSource edge: sqs_queue → lambda
    let event_source_edge = arch.connections.iter().find(|c| {
        c.connection_type == ConnectionType::EventSource
            && c.source.as_str() == "aws_sqs_queue_input_queue"
            && c.target.as_str() == "aws_lambda_function_processor"
    });
    assert!(
        event_source_edge.is_some(),
        "expected EventSource edge from aws_sqs_queue_input_queue to aws_lambda_function_processor; connections = {:?}",
        arch.connections,
    );
}

#[test]
fn lambda_to_dynamodb_produces_data_flow_edge() {
    // aws_lambda_function.processor references aws_dynamodb_table.state_table
    // in its environment block (captured as a top-level attr via collect_block_attrs).
    let dir = FixtureDir::new("cross-resource-df", &["cross_resource.tf"]);
    let config = parser::parse_tf_dir(dir.path()).unwrap();
    let resolved = resolver::resolve_config(config, None).unwrap();
    let (_, tf) = aws_registries();
    let arch = convert::build_architecture("test", "ap-northeast-1", &resolved, &tf);

    // DataFlow edge: lambda → dynamodb (from environment variable reference)
    let data_flow_edge = arch.connections.iter().find(|c| {
        c.connection_type == ConnectionType::DataFlow
            && c.source.as_str() == "aws_lambda_function_processor"
            && c.target.as_str() == "aws_dynamodb_table_state_table"
    });
    assert!(
        data_flow_edge.is_some(),
        "expected DataFlow edge from aws_lambda_function_processor to aws_dynamodb_table_state_table; connections = {:?}",
        arch.connections,
    );
}

#[test]
fn connections_have_no_duplicates() {
    let dir = FixtureDir::new("cross-resource-dedup", &["cross_resource.tf"]);
    let config = parser::parse_tf_dir(dir.path()).unwrap();
    let resolved = resolver::resolve_config(config, None).unwrap();
    let (_, tf) = aws_registries();
    let arch = convert::build_architecture("test", "ap-northeast-1", &resolved, &tf);

    // Verify no (source, target, type) triple appears more than once.
    let mut seen = std::collections::HashSet::new();
    for conn in &arch.connections {
        let key = (
            conn.source.as_str().to_string(),
            conn.target.as_str().to_string(),
            format!("{:?}", conn.connection_type),
        );
        assert!(
            seen.insert(key.clone()),
            "duplicate connection detected: {key:?}",
        );
    }
}

#[test]
fn connections_have_no_dangling_endpoints() {
    let dir = FixtureDir::new("cross-resource-dangle", &["cross_resource.tf"]);
    let config = parser::parse_tf_dir(dir.path()).unwrap();
    let resolved = resolver::resolve_config(config, None).unwrap();
    let (_, tf) = aws_registries();
    let arch = convert::build_architecture("test", "ap-northeast-1", &resolved, &tf);

    let node_ids: std::collections::HashSet<&str> = arch
        .resources
        .iter()
        .map(|r| r.logical_id.as_str())
        .collect();

    for conn in &arch.connections {
        assert!(
            node_ids.contains(conn.source.as_str()),
            "dangling source in connection: {conn:?}",
        );
        assert!(
            node_ids.contains(conn.target.as_str()),
            "dangling target in connection: {conn:?}",
        );
    }
}

// ---------------------------------------------------------------------------
// Nested Object/Array ResourceRef tests
// ---------------------------------------------------------------------------

#[test]
fn nested_object_ref_produces_data_flow_edge() {
    // nested_ref.tf: aws_lambda_function.my_function has an environment block
    // with variables = { TABLE_ARN = aws_dynamodb_table.my_table.arn }.
    // The ResourceRef is nested inside Object(variables) inside the environment
    // block — verify it still produces a DataFlow edge.
    let dir = FixtureDir::new("nested-ref", &["nested_ref.tf"]);
    let config = parser::parse_tf_dir(dir.path()).unwrap();
    let resolved = resolver::resolve_config(config, None).unwrap();
    let (_, tf) = aws_registries();
    let arch = convert::build_architecture("test", "ap-northeast-1", &resolved, &tf);

    let data_flow_edge = arch.connections.iter().find(|c| {
        c.connection_type == ConnectionType::DataFlow
            && c.source.as_str() == "aws_lambda_function_my_function"
            && c.target.as_str() == "aws_dynamodb_table_my_table"
    });
    assert!(
        data_flow_edge.is_some(),
        "expected DataFlow edge from aws_lambda_function_my_function to \
         aws_dynamodb_table_my_table (nested object ref); connections = {:?}",
        arch.connections,
    );
}

#[test]
fn nested_array_ref_produces_data_flow_edge() {
    // nested_ref.tf: aws_lambda_function.list_fn has an environment block with
    // variables = { QUEUE_URLS = [aws_sqs_queue.my_queue.url] }.
    // The ResourceRef is nested inside Array inside Object(variables) — verify
    // it produces a DataFlow edge.
    let dir = FixtureDir::new("nested-ref-array", &["nested_ref.tf"]);
    let config = parser::parse_tf_dir(dir.path()).unwrap();
    let resolved = resolver::resolve_config(config, None).unwrap();
    let (_, tf) = aws_registries();
    let arch = convert::build_architecture("test", "ap-northeast-1", &resolved, &tf);

    let data_flow_edge = arch.connections.iter().find(|c| {
        c.connection_type == ConnectionType::DataFlow
            && c.source.as_str() == "aws_lambda_function_list_fn"
            && c.target.as_str() == "aws_sqs_queue_my_queue"
    });
    assert!(
        data_flow_edge.is_some(),
        "expected DataFlow edge from aws_lambda_function_list_fn to \
         aws_sqs_queue_my_queue (nested array ref); connections = {:?}",
        arch.connections,
    );
}

#[test]
fn nested_ref_no_duplicates() {
    // Verify that nested refs do not produce duplicate edges.
    let dir = FixtureDir::new("nested-ref-dedup", &["nested_ref.tf"]);
    let config = parser::parse_tf_dir(dir.path()).unwrap();
    let resolved = resolver::resolve_config(config, None).unwrap();
    let (_, tf) = aws_registries();
    let arch = convert::build_architecture("test", "ap-northeast-1", &resolved, &tf);

    let mut seen = std::collections::HashSet::new();
    for conn in &arch.connections {
        let key = (
            conn.source.as_str().to_string(),
            conn.target.as_str().to_string(),
            format!("{:?}", conn.connection_type),
        );
        assert!(
            seen.insert(key.clone()),
            "duplicate connection detected: {key:?}",
        );
    }
}

// ---------------------------------------------------------------------------
// aws_s3_bucket_notification: Notification edge direction (#2)
// ---------------------------------------------------------------------------

#[test]
fn s3_bucket_notification_produces_bucket_to_lambda_edge() {
    // s3_notification.tf has:
    //   aws_s3_bucket.my_bucket
    //   aws_lambda_function.my_lambda
    //   aws_s3_bucket_notification.bucket_notif  (bucket = my_bucket, lambda = my_lambda)
    //
    // Expected: Notification edge from aws_s3_bucket_my_bucket → aws_lambda_function_my_lambda
    let dir = FixtureDir::new("s3-notif", &["s3_notification.tf"]);
    let config = parser::parse_tf_dir(dir.path()).unwrap();
    let resolved = resolver::resolve_config(config, None).unwrap();
    let (_, tf) = aws_registries();
    let arch = convert::build_architecture("test", "ap-northeast-1", &resolved, &tf);

    let notif_edge = arch.connections.iter().find(|c| {
        c.connection_type == ConnectionType::Notification
            && c.source.as_str() == "aws_s3_bucket_my_bucket"
            && c.target.as_str() == "aws_lambda_function_my_lambda"
    });
    assert!(
        notif_edge.is_some(),
        "expected Notification edge from aws_s3_bucket_my_bucket to aws_lambda_function_my_lambda; connections = {:?}",
        arch.connections,
    );

    // The notification config resource itself must NOT be a source.
    let wrong_src = arch
        .connections
        .iter()
        .any(|c| c.source.as_str() == "aws_s3_bucket_notification_bucket_notif");
    assert!(
        !wrong_src,
        "aws_s3_bucket_notification must not be an edge source; connections = {:?}",
        arch.connections,
    );
}

#[test]
fn nested_ref_spec_json_has_no_resource_ref() {
    // Verify that ResourceRef values nested in objects do not leak into the
    // spec JSON (i.e. tf_value_to_json drops them, leaving only concrete values).
    let dir = FixtureDir::new("nested-ref-spec", &["nested_ref.tf"]);
    let config = parser::parse_tf_dir(dir.path()).unwrap();
    let resolved = resolver::resolve_config(config, None).unwrap();
    let (_, tf) = aws_registries();
    let arch = convert::build_architecture("test", "ap-northeast-1", &resolved, &tf);

    // Serialise the whole architecture to JSON and verify no raw ResourceRef
    // tokens appear (they should be silently dropped by tf_value_to_json).
    let json_str = serde_json::to_string(&arch).expect("serialisation must succeed");

    // ResourceRef placeholders that must never appear in the output JSON.
    assert!(
        !json_str.contains("ResourceRef"),
        "ResourceRef leaked into spec JSON: {json_str}",
    );
}

// ---------------------------------------------------------------------------
// #3: classify_connection — unrecognised pairs must NOT produce edges
// ---------------------------------------------------------------------------

/// Lambda → IAM role (role attr) and lambda → CloudWatch log group must not
/// generate any connection edges. Lambda → S3 / DynamoDB must still appear.
#[test]
fn lambda_to_iam_role_does_not_produce_edge() {
    let dir = FixtureDir::new("classify-iam", &["classify_connection.tf"]);
    let config = parser::parse_tf_dir(dir.path()).unwrap();
    let resolved = resolver::resolve_config(config, None).unwrap();
    let (_, tf) = aws_registries();
    let arch = convert::build_architecture("test", "ap-northeast-1", &resolved, &tf);

    // IAM role must NOT be a connection target from any lambda.
    let iam_edge = arch
        .connections
        .iter()
        .find(|c| c.target.as_str() == "aws_iam_role_lambda_exec");
    assert!(
        iam_edge.is_none(),
        "unexpected edge to aws_iam_role_lambda_exec; connections = {:?}",
        arch.connections,
    );
}

/// Lambda → S3 bucket produces a DataFlow edge (STORAGE_RESOURCE_TYPES).
#[test]
fn lambda_to_s3_produces_data_flow_edge() {
    let dir = FixtureDir::new("classify-s3", &["classify_connection.tf"]);
    let config = parser::parse_tf_dir(dir.path()).unwrap();
    let resolved = resolver::resolve_config(config, None).unwrap();
    let (_, tf) = aws_registries();
    let arch = convert::build_architecture("test", "ap-northeast-1", &resolved, &tf);

    let edge = arch.connections.iter().find(|c| {
        c.connection_type == ConnectionType::DataFlow
            && c.source.as_str() == "aws_lambda_function_writer"
            && c.target.as_str() == "aws_s3_bucket_uploads"
    });
    assert!(
        edge.is_some(),
        "expected DataFlow edge from aws_lambda_function_writer to aws_s3_bucket_uploads; connections = {:?}",
        arch.connections,
    );
}

/// Lambda → DynamoDB produces a DataFlow edge (STORAGE_RESOURCE_TYPES).
#[test]
fn lambda_to_dynamodb_produces_data_flow_edge_via_classify() {
    let dir = FixtureDir::new("classify-ddb", &["classify_connection.tf"]);
    let config = parser::parse_tf_dir(dir.path()).unwrap();
    let resolved = resolver::resolve_config(config, None).unwrap();
    let (_, tf) = aws_registries();
    let arch = convert::build_architecture("test", "ap-northeast-1", &resolved, &tf);

    let edge = arch.connections.iter().find(|c| {
        c.connection_type == ConnectionType::DataFlow
            && c.source.as_str() == "aws_lambda_function_writer"
            && c.target.as_str() == "aws_dynamodb_table_items"
    });
    assert!(
        edge.is_some(),
        "expected DataFlow edge from aws_lambda_function_writer to aws_dynamodb_table_items; connections = {:?}",
        arch.connections,
    );
}

// ---------------------------------------------------------------------------
// #6: local ref — ResourceRef aliased through locals must produce edges
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// NON_RUNTIME_BLOCKS denylist: dead_letter_config et al. must not become edges
// ---------------------------------------------------------------------------

/// `dead_letter_config { target_arn = aws_sqs_queue.dlq.arn }` describes where
/// failed async invocations are sent — it is a deployment configuration, not a
/// runtime data-flow path.  The generic ref-collection loop must skip this block
/// so that no Lambda→SQS(dlq) DataFlow edge is created.
#[test]
fn dead_letter_config_does_not_produce_data_flow_edge() {
    let dir = FixtureDir::new("non-runtime-dlq", &["non_runtime_blocks.tf"]);
    let config = parser::parse_tf_dir(dir.path()).unwrap();
    let resolved = resolver::resolve_config(config, None).unwrap();
    let (_, tf) = aws_registries();
    let arch = convert::build_architecture("test", "ap-northeast-1", &resolved, &tf);

    let dlq_edge = arch.connections.iter().find(|c| {
        c.connection_type == ConnectionType::DataFlow
            && c.source.as_str() == "aws_lambda_function_my_fn"
            && c.target.as_str() == "aws_sqs_queue_dlq"
    });
    assert!(
        dlq_edge.is_none(),
        "unexpected DataFlow edge from aws_lambda_function_my_fn to aws_sqs_queue_dlq \
         (dead_letter_config must be skipped); connections = {:?}",
        arch.connections,
    );
}

/// `environment { variables = { TABLE_ARN = aws_dynamodb_table.data_table.arn } }`
/// is a runtime block — the denylist must NOT suppress it, so the Lambda→DynamoDB
/// DataFlow edge must still be present.
#[test]
fn environment_block_still_produces_data_flow_edge() {
    let dir = FixtureDir::new("non-runtime-env", &["non_runtime_blocks.tf"]);
    let config = parser::parse_tf_dir(dir.path()).unwrap();
    let resolved = resolver::resolve_config(config, None).unwrap();
    let (_, tf) = aws_registries();
    let arch = convert::build_architecture("test", "ap-northeast-1", &resolved, &tf);

    let table_edge = arch.connections.iter().find(|c| {
        c.connection_type == ConnectionType::DataFlow
            && c.source.as_str() == "aws_lambda_function_my_fn"
            && c.target.as_str() == "aws_dynamodb_table_data_table"
    });
    assert!(
        table_edge.is_some(),
        "expected DataFlow edge from aws_lambda_function_my_fn to \
         aws_dynamodb_table_data_table (environment block must not be suppressed); \
         connections = {:?}",
        arch.connections,
    );
}

/// `local.fn_arn = aws_lambda_function.fn.arn` is used as the
/// `lambda_function_arn` inside an `aws_s3_bucket_notification` block.
/// After the fix the local resolves to a ResourceRef and the notification
/// resource produces a Notification edge from the S3 bucket to the lambda.
#[test]
fn local_resource_ref_alias_produces_notification_edge() {
    let dir = FixtureDir::new("local-ref", &["local_ref.tf"]);
    let config = parser::parse_tf_dir(dir.path()).unwrap();
    let resolved = resolver::resolve_config(config, None).unwrap();
    let (_, tf) = aws_registries();
    let arch = convert::build_architecture("test", "ap-northeast-1", &resolved, &tf);

    let edge = arch.connections.iter().find(|c| {
        c.connection_type == ConnectionType::Notification
            && c.source.as_str() == "aws_s3_bucket_uploads"
            && c.target.as_str() == "aws_lambda_function_fn"
    });
    assert!(
        edge.is_some(),
        "expected Notification edge from aws_s3_bucket_uploads to aws_lambda_function_fn \
         (via local.fn_arn alias); connections = {:?}",
        arch.connections,
    );
}

/// Integration (wiring): a `.tfvars` file exceeding `MAX_IAC_FILE_BYTES` is
/// rejected by `parse_tfvars`' read path (`read_to_string_capped`). Ignored by
/// default because it writes a >16 MiB temp file.
#[test]
#[ignore = "writes a >16 MiB temp file; run with `cargo test -- --ignored`"]
fn parse_tfvars_rejects_oversized_file() {
    let path =
        std::env::temp_dir().join(format!("yevice_tf_oversized_{}.tfvars", std::process::id()));
    std::fs::write(
        &path,
        vec![b' '; (yevice_core::io::MAX_IAC_FILE_BYTES + 1) as usize],
    )
    .unwrap();
    let result = parser::parse_tfvars(&path);
    let _ = std::fs::remove_file(&path);
    assert!(result.is_err(), "oversized tfvars file must be rejected");
}
