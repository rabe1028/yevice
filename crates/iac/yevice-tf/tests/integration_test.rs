//! Integration tests for the Terraform parser + service plugin pipeline.

use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use yevice_core::resource::{Architecture, Provider, Resource};
use yevice_pricing::gcp_hardcoded_pricing;
use yevice_service_api::{CfnAdapterRegistry, MultiProviderCatalog, ServiceCatalog, TfAdapterRegistry};
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
