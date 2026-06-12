//! Absolute-amount tests for service `build_cost` formulas.
//!
//! These pin the exact monetary result of each service's cost expression given
//! round-number test prices and explicit variable bindings, so that mutations
//! to the cost arithmetic (e.g. `*` -> `+`, `-` -> `/`) are caught.

use yevice_core::{
    cost::Tier,
    evaluate::{Params, evaluate},
    expr::Expr,
    types::{LogicalId, ResourceType, VariableName},
};
use yevice_pricing::{
    catalog::{PriceCatalog, PriceRecord, Sku},
    error::PricingError,
};
use yevice_service_api::Service;

use yevice_services_aws::services::{
    alb::{AlbService, AlbSpec},
    api_gateway::{ApiGatewayService, ApiGatewaySpec, ApiGatewayType},
    appsync::{AppSyncService, AppSyncSpec},
    athena::{AthenaService, AthenaSpec},
    batch::{BatchEbsConfig, BatchJobDefinitionSpec, BatchLaunchType, BatchService},
    cloudfront::{CloudFrontService, CloudFrontSpec},
    cloudwatch_logs::{CloudWatchLogsService, CloudWatchLogsSpec},
    cognito::{CognitoService, CognitoUserPoolSpec},
    documentdb::{DocumentDbService, DocumentDbSpec},
    dynamodb::{DynamoDbBillingMode, DynamoDbService, DynamoDbSpec},
    ec2::{Ec2Service, Ec2Spec},
    ecr::{EcrService, EcrSpec},
    ecs_ec2::{EcsEc2Service, EcsEc2Spec},
    ecs_fargate::{EcsFargateService, EcsFargateSpec},
    efs::{EfsService, EfsSpec},
    eks::{EksClusterSpec, EksService},
    elasticache::{ElastiCacheService, ElastiCacheSpec},
    eventbridge_rule::{EventBridgeRuleService, EventBridgeRuleSpec},
    eventbridge_scheduler::{EventBridgeSchedulerService, EventBridgeSchedulerSpec},
    firehose::{FirehoseService, KinesisFirehoseSpec},
    glue::{GlueDpuType, GlueJobSpec, GlueService},
    kinesis::{KinesisService, KinesisSpec, KinesisStreamMode},
    lambda::{LambdaService, LambdaSpec},
    msk::{MskClusterSpec, MskService},
    nat_gateway::{NatGatewayService, NatGatewaySpec},
    opensearch_serverless::{OpenSearchServerlessService, OpenSearchServerlessSpec},
    opensearch_service::{OpenSearchServiceService, OpenSearchServiceSpec},
    rds::{RdsEngine, RdsService, RdsSpec},
    redshift::{RedshiftService, RedshiftSpec},
    route53::{Route53HostedZoneSpec, Route53Service},
    s3::{S3Service, S3Spec},
    secrets_manager::{SecretsManagerService, SecretsManagerSpec},
    sns::{SnsService, SnsSpec},
    sqs::{SqsService, SqsSpec},
    step_functions::{StepFunctionsService, StepFunctionsSpec, StepFunctionsType},
    waf::{WafService, WafSpec},
};

/// Catalog with round-number prices so expected totals are trivial to verify
/// by hand. Add SKU arms here as more services are covered.
struct TestCatalog;

fn free_then(free: f64, unit_price: f64) -> PriceRecord {
    PriceRecord::tiered(vec![
        Tier {
            upper_limit: Some(free),
            unit_price: 0.0,
        },
        Tier {
            upper_limit: None,
            unit_price,
        },
    ])
}

impl PriceCatalog for TestCatalog {
    fn region(&self) -> &'static str {
        "test"
    }

    fn lookup(&self, sku: &Sku) -> Result<PriceRecord, PricingError> {
        let price = match sku.as_str() {
            "aws.alb.alb_hour_price" => PriceRecord::flat(0.10),
            "aws.alb.lcu_hour_price" => PriceRecord::flat(0.01),
            "aws.api_gateway.rest_api_request_price" => PriceRecord::flat(1.0),
            "aws.api_gateway.http_api_request_price" => PriceRecord::flat(2.0),
            "aws.api_gateway.free_tier_requests" => PriceRecord::flat(100.0),
            "aws.appsync.operation_price_per_million" => PriceRecord::flat(1_000_000.0),
            "aws.appsync.free_tier_operations" => PriceRecord::flat(100.0),
            "aws.athena.scan_price_per_tb" => PriceRecord::flat(1_000.0),
            "aws.batch.fargate_vcpu_hour_price" => PriceRecord::flat(2.0),
            "aws.batch.fargate_memory_gb_hour_price" => PriceRecord::flat(1.0),
            "aws.batch.fargate_ephemeral_storage_gb_hour_price" => PriceRecord::flat(0.5),
            "aws.batch.fargate_ephemeral_free_gb" => PriceRecord::flat(20.0),
            "aws.batch.ebs_gp3_gb_month_price" => PriceRecord::flat(1.0),
            "aws.batch.ebs_gp3_iops_month_price" => PriceRecord::flat(1.0),
            "aws.batch.ebs_gp3_iops_free" => PriceRecord::flat(3_000.0),
            "aws.batch.ebs_gp3_throughput_mibps_month_price" => PriceRecord::flat(1.0),
            "aws.batch.ebs_gp3_throughput_free_mibps" => PriceRecord::flat(125.0),
            "aws.cloudfront.request_price_per_10k" => PriceRecord::flat(10.0),
            "aws.cloudfront.data_transfer_price_per_gb" => PriceRecord::flat(2.0),
            "aws.cloudfront.free_tier_data_transfer_gb" => PriceRecord::flat(100.0),
            "aws.cloudwatch_logs.ingestion_price_per_gb" => PriceRecord::flat(2.0),
            "aws.cloudwatch_logs.storage_price_per_gb" => PriceRecord::flat(1.0),
            "aws.cloudwatch_logs.free_tier_ingestion_gb" => PriceRecord::flat(10.0),
            "aws.cloudwatch_logs.free_tier_storage_gb" => PriceRecord::flat(20.0),
            "aws.cognito.free_tier_mau" => PriceRecord::flat(0.0),
            "aws.cognito.tier1_price" => PriceRecord::flat(1.0),
            "aws.cognito.tier2_price" => PriceRecord::flat(0.5),
            "aws.cognito.tier3_price" => PriceRecord::flat(0.25),
            "aws.dynamodb.write_request_price" => PriceRecord::flat(2.0),
            "aws.dynamodb.read_request_price" => PriceRecord::flat(1.0),
            "aws.dynamodb.wcu_hour_price" => PriceRecord::flat(1.0),
            "aws.dynamodb.rcu_hour_price" => PriceRecord::flat(2.0),
            "aws.dynamodb.storage_price_per_gb" => PriceRecord::flat(3.0),
            "aws.dynamodb.free_tier_wru" => PriceRecord::flat(10.0),
            "aws.dynamodb.free_tier_rru" => PriceRecord::flat(20.0),
            "aws.dynamodb.free_tier_storage_gb" => PriceRecord::flat(5.0),
            "aws.data_transfer.egress_tiers" => free_then(100.0, 1.0),
            "aws.ecr.private_storage_gb_month" => PriceRecord::flat(2.0),
            "aws.fargate.vcpu_hour_price" => PriceRecord::flat(0.1),
            "aws.fargate.memory_gb_hour_price" => PriceRecord::flat(0.01),
            "aws.efs.standard_gb_month_price" => PriceRecord::flat(1.0),
            "aws.efs.ia_gb_month_price" => PriceRecord::flat(0.5),
            "aws.eks.cluster_hour_price" => PriceRecord::flat(1.0),
            "aws.eventbridge_rule.custom_event_price_per_million" => PriceRecord::flat(1_000_000.0),
            "aws.eventbridge_scheduler.invocation_price" => PriceRecord::flat(2.0),
            "aws.eventbridge_scheduler.free_tier_invocations" => PriceRecord::flat(10.0),
            "aws.firehose.ingestion_price_per_gb" => PriceRecord::flat(3.0),
            "aws.glue.standard_dpu_hour_price" => PriceRecord::flat(2.0),
            "aws.glue.flex_dpu_hour_price" => PriceRecord::flat(1.0),
            "aws.kinesis.shard_hour_price" => PriceRecord::flat(1.0),
            "aws.kinesis.put_payload_unit_price" => PriceRecord::flat(0.5),
            "aws.kinesis.on_demand_ingestion_price_per_gb" => PriceRecord::flat(2.0),
            "aws.kinesis.on_demand_retrieval_price_per_gb" => PriceRecord::flat(3.0),
            "aws.kinesis.on_demand_stream_hour_price" => PriceRecord::flat(1.0),
            "aws.lambda.request_price" => PriceRecord::flat(2.0),
            "aws.lambda.gb_second" => PriceRecord::flat(1.0),
            "aws.lambda.free_tier_requests" => PriceRecord::flat(10.0),
            "aws.lambda.free_tier_gb_seconds" => PriceRecord::flat(20.0),
            "aws.nat_gateway.hourly_price" => PriceRecord::flat(1.0),
            "aws.nat_gateway.data_processing_price_per_gb" => PriceRecord::flat(2.0),
            "aws.opensearch_serverless.ocu_hour_price" => PriceRecord::flat(1.0),
            "aws.opensearch_serverless.storage_price_per_gb" => PriceRecord::flat(2.0),
            "aws.route53.hosted_zone_month_price" => PriceRecord::flat(5.0),
            "aws.route53.query_price_per_million" => PriceRecord::flat(1_000_000.0),
            "aws.s3.put_request_price" => PriceRecord::flat(2.0),
            "aws.s3.get_request_price" => PriceRecord::flat(3.0),
            "aws.s3.storage_tiers" => free_then(100.0, 1.0),
            "aws.secrets_manager.secret_month_price" => PriceRecord::flat(5.0),
            "aws.secrets_manager.api_call_price_per_10k" => PriceRecord::flat(10_000.0),
            "aws.sns.delivery_price_per_million" => PriceRecord::flat(1_000_000.0),
            "aws.sns.free_tier_deliveries" => PriceRecord::flat(10.0),
            "aws.sqs.standard_request_price" => PriceRecord::flat(1.0),
            "aws.sqs.fifo_request_price" => PriceRecord::flat(2.0),
            "aws.sqs.free_tier_requests" => PriceRecord::flat(10.0),
            "aws.step_functions.standard_transition_price" => PriceRecord::flat(2.0),
            "aws.step_functions.express_request_price" => PriceRecord::flat(3.0),
            "aws.step_functions.express_duration_price_per_gb_second" => PriceRecord::flat(2.0),
            "aws.step_functions.free_tier_transitions" => PriceRecord::flat(10.0),
            "aws.waf.web_acl_month_price" => PriceRecord::flat(5.0),
            "aws.waf.rule_month_price" => PriceRecord::flat(2.0),
            "aws.waf.request_price_per_million" => PriceRecord::flat(1_000_000.0),
            sku if sku.starts_with("aws.documentdb_storage.") => PriceRecord::flat(2.0),
            sku if sku.starts_with("aws.documentdb.") => PriceRecord::flat(1.0),
            sku if sku.starts_with("aws.ec2.instance.") => PriceRecord::flat(1.0),
            sku if sku.starts_with("aws.elasticache.") => PriceRecord::flat(1.0),
            sku if sku.starts_with("aws.msk_storage.") => PriceRecord::flat(2.0),
            sku if sku.starts_with("aws.msk.") => PriceRecord::flat(1.0),
            sku if sku.starts_with("aws.opensearch_service_storage.") => PriceRecord::flat(2.0),
            sku if sku.starts_with("aws.opensearch_service.") => PriceRecord::flat(1.0),
            sku if sku.starts_with("aws.rds_storage.") => PriceRecord::flat(2.0),
            sku if sku.starts_with("aws.rds.") => PriceRecord::flat(1.0),
            sku if sku.starts_with("aws.redshift.") => PriceRecord::flat(1.0),
            other => {
                return Err(PricingError::NotFound {
                    service: other.to_string(),
                    region: "test".to_string(),
                });
            }
        };
        Ok(price)
    }
}

#[track_caller]
fn approx(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-9,
        "expected {expected}, got {actual}"
    );
}

#[track_caller]
fn approx_expr(expr: &Expr, params: &Params, expected: f64) {
    approx(evaluate(expr, params).expect("eval"), expected);
}

fn rt(name: &str) -> ResourceType {
    ResourceType::new(name)
}

fn params<const N: usize>(entries: [(VariableName, f64); N]) -> Params {
    entries.into_iter().collect()
}

// 730 hours per month is the constant used across the hourly cost formulas.

#[test]
fn alb_application_load_balancer_cost_is_fixed_plus_lcu() {
    let id = LogicalId::new("lb");
    let spec = AlbSpec {
        load_balancer_type: "application".to_string(),
    };
    let cost = AlbService
        .build_cost(
            &id,
            &rt("AWS::ElasticLoadBalancingV2::LoadBalancer"),
            &spec,
            &TestCatalog,
        )
        .expect("build cost");

    let params = params([(id.var("lcu"), 10.0)]);

    approx_expr(&cost.expr, &params, 146.0);
    approx_expr(&cost.components[0].expr, &params, 73.0);
    approx_expr(&cost.components[1].expr, &params, 73.0);
}

#[test]
fn alb_network_load_balancer_has_no_lcu_cost() {
    let id = LogicalId::new("lb");
    let spec = AlbSpec {
        load_balancer_type: "network".to_string(),
    };
    let cost = AlbService
        .build_cost(
            &id,
            &rt("AWS::ElasticLoadBalancingV2::LoadBalancer"),
            &spec,
            &TestCatalog,
        )
        .expect("build cost");

    let params = params([]);

    approx_expr(&cost.expr, &params, 73.0);
    approx_expr(&cost.components[0].expr, &params, 73.0);
    approx_expr(&cost.components[1].expr, &params, 0.0);
}

#[test]
fn api_gateway_rest_requests_charge_after_free_tier() {
    let id = LogicalId::new("api");
    let spec = ApiGatewaySpec {
        api_type: ApiGatewayType::Rest,
    };
    let cost = ApiGatewayService
        .build_cost(&id, &rt("AWS::ApiGateway::RestApi"), &spec, &TestCatalog)
        .expect("build cost");

    let params = params([(id.var("api_requests"), 250.0)]);

    approx_expr(&cost.expr, &params, 150.0);
}

#[test]
fn api_gateway_http_requests_use_http_rate() {
    let id = LogicalId::new("http_api");
    let spec = ApiGatewaySpec {
        api_type: ApiGatewayType::Http,
    };
    let cost = ApiGatewayService
        .build_cost(&id, &rt("AWS::ApiGatewayV2::Api"), &spec, &TestCatalog)
        .expect("build cost");

    let params = params([(id.var("api_requests"), 250.0)]);

    approx_expr(&cost.expr, &params, 300.0);
}

#[test]
fn appsync_operations_cost_matches_absolute_amount() {
    let id = LogicalId::new("graphql");
    let cost = AppSyncService
        .build_cost(
            &id,
            &rt("AWS::AppSync::GraphQLApi"),
            &AppSyncSpec {},
            &TestCatalog,
        )
        .expect("build cost");

    let params = params([(id.var("operations"), 250.0)]);

    approx_expr(&cost.expr, &params, 150.0);
    approx_expr(&cost.components[0].expr, &params, 150.0);
}

#[test]
fn athena_scanned_data_cost_matches_absolute_amount() {
    let id = LogicalId::new("athena");
    let cost = AthenaService
        .build_cost(
            &id,
            &rt("AWS::Athena::WorkGroup"),
            &AthenaSpec {},
            &TestCatalog,
        )
        .expect("build cost");

    let params = params([(id.var("scan_gb"), 250.0)]);

    approx_expr(&cost.expr, &params, 250.0);
    approx_expr(&cost.components[0].expr, &params, 250.0);
}

#[test]
fn batch_fargate_job_cost_includes_compute_and_ephemeral_storage() {
    let id = LogicalId::new("batch_fargate");
    let spec = BatchJobDefinitionSpec {
        launch_type: BatchLaunchType::Fargate,
        vcpu: 2.0,
        memory_gb: 4.0,
        ephemeral_storage_gb: Some(30.0),
        ebs: None,
    };
    let cost = BatchService
        .build_cost(&id, &rt("AWS::Batch::JobDefinition"), &spec, &TestCatalog)
        .expect("build cost");

    let params = params([
        (id.var("executions"), 10.0),
        (id.var("avg_duration_sec"), 3600.0),
    ]);

    approx_expr(&cost.expr, &params, 130.0);
    approx_expr(&cost.components[0].expr, &params, 80.0);
    approx_expr(&cost.components[1].expr, &params, 50.0);
}

#[test]
fn batch_ebs_storage_cost_is_prorated_by_runtime() {
    let id = LogicalId::new("batch_ebs");
    let spec = BatchJobDefinitionSpec {
        launch_type: BatchLaunchType::Ec2,
        vcpu: 1.0,
        memory_gb: 1.0,
        ephemeral_storage_gb: None,
        ebs: Some(BatchEbsConfig {
            size_gb: 10.0,
            volume_type: "gp3".to_string(),
            iops: Some(3100.0),
            throughput_mibps: Some(130.0),
        }),
    };
    let cost = BatchService
        .build_cost(&id, &rt("AWS::Batch::JobDefinition"), &spec, &TestCatalog)
        .expect("build cost");

    let params = params([
        (id.var("executions"), 730.0),
        (id.var("avg_duration_sec"), 3600.0),
    ]);

    approx_expr(&cost.expr, &params, 2305.0);
    approx_expr(&cost.components[0].expr, &params, 2190.0);
    approx_expr(&cost.components[1].expr, &params, 115.0);
}

#[test]
fn cloudfront_cost_is_requests_plus_paid_transfer() {
    let id = LogicalId::new("cdn");
    let cost = CloudFrontService
        .build_cost(
            &id,
            &rt("AWS::CloudFront::Distribution"),
            &CloudFrontSpec {},
            &TestCatalog,
        )
        .expect("build cost");

    let params = params([
        (id.var("http_requests"), 20_000.0),
        (id.var("data_transfer_gb"), 150.0),
    ]);

    approx_expr(&cost.expr, &params, 120.0);
    approx_expr(&cost.components[0].expr, &params, 20.0);
    approx_expr(&cost.components[1].expr, &params, 100.0);
}

#[test]
fn cloudwatch_logs_cost_is_ingestion_plus_storage_over_free_tier() {
    let id = LogicalId::new("logs");
    let spec = CloudWatchLogsSpec {
        retention_days: Some(14),
    };
    let cost = CloudWatchLogsService
        .build_cost(&id, &rt("AWS::Logs::LogGroup"), &spec, &TestCatalog)
        .expect("build cost");

    let params = params([(id.var("ingestion_gb"), 25.0), (id.var("storage_gb"), 50.0)]);

    approx_expr(&cost.expr, &params, 60.0);
    approx_expr(&cost.components[0].expr, &params, 30.0);
    approx_expr(&cost.components[1].expr, &params, 30.0);
}

#[test]
fn cognito_mau_cost_matches_hard_cutoff_tiers() {
    let id = LogicalId::new("pool");
    let cost = CognitoService
        .build_cost(
            &id,
            &rt("AWS::Cognito::UserPool"),
            &CognitoUserPoolSpec {},
            &TestCatalog,
        )
        .expect("build cost");

    let params = params([(id.var("mau"), 1_500_000.0)]);

    approx_expr(&cost.expr, &params, 675_000.0);
    approx_expr(&cost.components[0].expr, &params, 675_000.0);
}

#[test]
fn documentdb_cluster_cost_is_instances_plus_storage() {
    let id = LogicalId::new("docdb");
    let spec = DocumentDbSpec {
        instance_type: "db.r6g.large".to_string(),
        instance_count: None,
    };
    let cost = DocumentDbService
        .build_cost(&id, &rt("AWS::DocDB::DBCluster"), &spec, &TestCatalog)
        .expect("build cost");

    let params = params([
        (id.var("instance_count"), 2.0),
        (id.var("storage_gb"), 10.0),
    ]);

    approx_expr(&cost.expr, &params, 1480.0);
    approx_expr(&cost.components[0].expr, &params, 1460.0);
    approx_expr(&cost.components[1].expr, &params, 20.0);
}

#[test]
fn dynamodb_on_demand_cost_matches_requests_and_storage() {
    let id = LogicalId::new("dynamo_ondemand");
    let spec = DynamoDbSpec {
        billing_mode: DynamoDbBillingMode::OnDemand,
        has_stream: false,
        gsi_count: 0,
    };
    let cost = DynamoDbService
        .build_cost(&id, &rt("AWS::DynamoDB::Table"), &spec, &TestCatalog)
        .expect("build cost");

    let params = params([
        (id.var("write_request_units"), 25.0),
        (id.var("read_request_units"), 50.0),
        (id.var("storage_gb"), 10.0),
    ]);

    approx_expr(&cost.expr, &params, 75.0);
    approx_expr(&cost.components[0].expr, &params, 30.0);
    approx_expr(&cost.components[1].expr, &params, 30.0);
    approx_expr(&cost.components[2].expr, &params, 15.0);
}

#[test]
fn dynamodb_provisioned_cost_matches_capacity_hours_and_storage() {
    let id = LogicalId::new("dynamo_provisioned");
    let spec = DynamoDbSpec {
        billing_mode: DynamoDbBillingMode::Provisioned {
            write_capacity_units: None,
            read_capacity_units: None,
        },
        has_stream: false,
        gsi_count: 0,
    };
    let cost = DynamoDbService
        .build_cost(&id, &rt("AWS::DynamoDB::Table"), &spec, &TestCatalog)
        .expect("build cost");

    let params = params([
        (id.var("write_capacity_units"), 2.0),
        (id.var("read_capacity_units"), 3.0),
        (id.var("storage_gb"), 10.0),
    ]);

    approx_expr(&cost.expr, &params, 5855.0);
    approx_expr(&cost.components[0].expr, &params, 1460.0);
    approx_expr(&cost.components[1].expr, &params, 4380.0);
    approx_expr(&cost.components[2].expr, &params, 15.0);
}

#[test]
fn ec2_cost_is_instance_plus_egress() {
    let id = LogicalId::new("vm");
    let spec = Ec2Spec {
        instance_type: "t3.micro".to_string(),
        os: yevice_services_aws::services::ec2::Ec2Os::Linux,
    };
    let cost = Ec2Service
        .build_cost(&id, &rt("AWS::EC2::Instance"), &spec, &TestCatalog)
        .expect("build cost");

    let params = params([(id.var("data_transfer_out_gb"), 150.0)]);

    approx_expr(&cost.expr, &params, 780.0);
    approx_expr(&cost.components[0].expr, &params, 730.0);
    approx_expr(&cost.components[1].expr, &params, 50.0);
}

#[test]
fn ecr_private_repository_storage_cost_matches_absolute_amount() {
    let id = LogicalId::new("ecr_private");
    let spec = EcrSpec { is_private: true };
    let cost = EcrService
        .build_cost(&id, &rt("AWS::ECR::Repository"), &spec, &TestCatalog)
        .expect("build cost");

    let params = params([(id.var("storage_gb"), 10.0)]);

    approx_expr(&cost.expr, &params, 20.0);
    approx_expr(&cost.components[0].expr, &params, 20.0);
}

#[test]
fn ecr_public_repository_has_zero_storage_cost() {
    let id = LogicalId::new("ecr_public");
    let spec = EcrSpec { is_private: false };
    let cost = EcrService
        .build_cost(&id, &rt("AWS::ECR::Repository"), &spec, &TestCatalog)
        .expect("build cost");

    let params = params([(id.var("storage_gb"), 10.0)]);

    approx_expr(&cost.expr, &params, 0.0);
    approx_expr(&cost.components[0].expr, &params, 0.0);
}

#[test]
fn ecs_on_ec2_cost_is_instances_plus_egress() {
    let id = LogicalId::new("ecs_ec2");
    let spec = EcsEc2Spec {
        instance_type: "t3.micro".to_string(),
        instance_count: None,
    };
    let cost = EcsEc2Service
        .build_cost(&id, &rt("AWS::ECS::Service"), &spec, &TestCatalog)
        .expect("build cost");

    let params = params([
        (id.var("instance_count"), 2.0),
        (id.var("data_transfer_out_gb"), 150.0),
    ]);

    approx_expr(&cost.expr, &params, 1510.0);
    approx_expr(&cost.components[0].expr, &params, 1460.0);
    approx_expr(&cost.components[1].expr, &params, 50.0);
}

#[test]
fn ecs_fargate_cost_scales_task_components_and_egress() {
    let id = LogicalId::new("ecs_fargate");
    let spec = EcsFargateSpec {
        desired_count: None,
    };
    let cost = EcsFargateService
        .build_cost(&id, &rt("AWS::ECS::Service"), &spec, &TestCatalog)
        .expect("build cost");

    let params = params([
        (id.var("desired_count"), 3.0),
        (id.var("vcpu"), 2.0),
        (id.var("memory_gb"), 4.0),
        (id.var("data_transfer_out_gb"), 150.0),
    ]);

    approx_expr(&cost.expr, &params, 575.6);
    approx_expr(&cost.components[0].expr, &params, 438.0);
    approx_expr(&cost.components[1].expr, &params, 87.6);
    approx_expr(&cost.components[2].expr, &params, 50.0);
}

#[test]
fn efs_standard_storage_cost_matches_absolute_amount() {
    let id = LogicalId::new("efs_standard");
    let spec = EfsSpec {
        has_ia_lifecycle: false,
    };
    let cost = EfsService
        .build_cost(&id, &rt("AWS::EFS::FileSystem"), &spec, &TestCatalog)
        .expect("build cost");

    let params = params([(id.var("storage_gb"), 10.0)]);

    approx_expr(&cost.expr, &params, 10.0);
    approx_expr(&cost.components[0].expr, &params, 10.0);
}

#[test]
fn efs_ia_storage_cost_uses_ia_rate() {
    let id = LogicalId::new("efs_ia");
    let spec = EfsSpec {
        has_ia_lifecycle: true,
    };
    let cost = EfsService
        .build_cost(&id, &rt("AWS::EFS::FileSystem"), &spec, &TestCatalog)
        .expect("build cost");

    let params = params([(id.var("storage_gb"), 10.0)]);

    approx_expr(&cost.expr, &params, 5.0);
    approx_expr(&cost.components[0].expr, &params, 5.0);
}

#[test]
fn eks_cluster_management_fee_is_hourly_times_month() {
    let id = LogicalId::new("eks");
    let cost = EksService
        .build_cost(
            &id,
            &rt("AWS::EKS::Cluster"),
            &EksClusterSpec {},
            &TestCatalog,
        )
        .expect("build cost");

    let params = params([]);

    approx_expr(&cost.expr, &params, 730.0);
    approx_expr(&cost.components[0].expr, &params, 730.0);
}

#[test]
fn elasticache_cost_uses_spec_node_count() {
    let id = LogicalId::new("cache");
    let spec = ElastiCacheSpec {
        node_type: "cache.t4g.small".to_string(),
        num_nodes: 3.0,
    };
    let cost = ElastiCacheService
        .build_cost(
            &id,
            &rt("AWS::ElastiCache::CacheCluster"),
            &spec,
            &TestCatalog,
        )
        .expect("build cost");

    let params = params([]);

    approx_expr(&cost.expr, &params, 2190.0);
}

#[test]
fn eventbridge_rule_cost_matches_custom_events_count() {
    let id = LogicalId::new("rule");
    let cost = EventBridgeRuleService
        .build_cost(
            &id,
            &rt("AWS::Events::Rule"),
            &EventBridgeRuleSpec {},
            &TestCatalog,
        )
        .expect("build cost");

    let params = params([(id.var("events"), 250.0)]);

    approx_expr(&cost.expr, &params, 250.0);
}

#[test]
fn eventbridge_scheduler_cost_respects_free_tier() {
    let id = LogicalId::new("schedule");
    let cost = EventBridgeSchedulerService
        .build_cost(
            &id,
            &rt("AWS::Scheduler::Schedule"),
            &EventBridgeSchedulerSpec {},
            &TestCatalog,
        )
        .expect("build cost");

    let params = params([(id.var("invocations"), 25.0)]);

    approx_expr(&cost.expr, &params, 30.0);
}

#[test]
fn firehose_ingestion_cost_matches_absolute_amount() {
    let id = LogicalId::new("firehose");
    let cost = FirehoseService
        .build_cost(
            &id,
            &rt("AWS::KinesisFirehose::DeliveryStream"),
            &KinesisFirehoseSpec {},
            &TestCatalog,
        )
        .expect("build cost");

    let params = params([(id.var("ingestion_gb"), 10.0)]);

    approx_expr(&cost.expr, &params, 30.0);
    approx_expr(&cost.components[0].expr, &params, 30.0);
}

#[test]
fn glue_standard_dpu_cost_matches_absolute_amount() {
    let id = LogicalId::new("glue_standard");
    let spec = GlueJobSpec {
        dpu_type: GlueDpuType::Standard,
        max_dpu: Some(3.0),
    };
    let cost = GlueService
        .build_cost(&id, &rt("AWS::Glue::Job"), &spec, &TestCatalog)
        .expect("build cost");

    let params = params([(id.var("job_hours"), 10.0)]);

    approx_expr(&cost.expr, &params, 60.0);
    approx_expr(&cost.components[0].expr, &params, 60.0);
}

#[test]
fn glue_flex_defaults_to_ten_dpu_when_max_is_missing() {
    let id = LogicalId::new("glue_flex");
    let spec = GlueJobSpec {
        dpu_type: GlueDpuType::Flex,
        max_dpu: None,
    };
    let cost = GlueService
        .build_cost(&id, &rt("AWS::Glue::Job"), &spec, &TestCatalog)
        .expect("build cost");

    let params = params([(id.var("job_hours"), 2.0)]);

    approx_expr(&cost.expr, &params, 20.0);
    approx_expr(&cost.components[0].expr, &params, 20.0);
}

#[test]
fn kinesis_provisioned_cost_is_shards_plus_put_payload() {
    let id = LogicalId::new("kinesis_provisioned");
    let spec = KinesisSpec {
        stream_mode: KinesisStreamMode::Provisioned { shard_count: None },
        retention_hours: 24.0,
    };
    let cost = KinesisService
        .build_cost(&id, &rt("AWS::Kinesis::Stream"), &spec, &TestCatalog)
        .expect("build cost");

    let params = params([(id.var("shard_count"), 2.0), (id.var("put_records"), 10.0)]);

    approx_expr(&cost.expr, &params, 1465.0);
    approx_expr(&cost.components[0].expr, &params, 1460.0);
    approx_expr(&cost.components[1].expr, &params, 5.0);
}

#[test]
fn kinesis_on_demand_cost_is_stream_hours_plus_io() {
    let id = LogicalId::new("kinesis_ondemand");
    let spec = KinesisSpec {
        stream_mode: KinesisStreamMode::OnDemand,
        retention_hours: 24.0,
    };
    let cost = KinesisService
        .build_cost(&id, &rt("AWS::Kinesis::Stream"), &spec, &TestCatalog)
        .expect("build cost");

    let params = params([
        (id.var("data_ingestion_gb"), 10.0),
        (id.var("retrieval_gb"), 5.0),
    ]);

    approx_expr(&cost.expr, &params, 765.0);
    approx_expr(&cost.components[0].expr, &params, 730.0);
    approx_expr(&cost.components[1].expr, &params, 20.0);
    approx_expr(&cost.components[2].expr, &params, 15.0);
}

#[test]
fn lambda_cost_is_requests_compute_and_egress() {
    let id = LogicalId::new("lambda");
    let spec = LambdaSpec {
        memory_mb: 1024.0,
        timeout_sec: 3.0,
        runtime: None,
    };
    let cost = LambdaService
        .build_cost(&id, &rt("AWS::Lambda::Function"), &spec, &TestCatalog)
        .expect("build cost");

    let params = params([
        (id.var("requests"), 30.0),
        (id.var("avg_duration_ms"), 1000.0),
        (id.var("data_transfer_out_gb"), 150.0),
    ]);

    approx_expr(&cost.expr, &params, 100.0);
    approx_expr(&cost.components[0].expr, &params, 40.0);
    approx_expr(&cost.components[1].expr, &params, 10.0);
    approx_expr(&cost.components[2].expr, &params, 50.0);
}

#[test]
fn msk_cluster_cost_is_brokers_plus_storage() {
    let id = LogicalId::new("msk");
    let spec = MskClusterSpec {
        broker_instance_type: "kafka.m5.large".to_string(),
        broker_count: None,
    };
    let cost = MskService
        .build_cost(&id, &rt("AWS::MSK::Cluster"), &spec, &TestCatalog)
        .expect("build cost");

    let params = params([(id.var("broker_count"), 2.0), (id.var("storage_gb"), 10.0)]);

    approx_expr(&cost.expr, &params, 1480.0);
    approx_expr(&cost.components[0].expr, &params, 1460.0);
    approx_expr(&cost.components[1].expr, &params, 20.0);
}

#[test]
fn nat_gateway_cost_is_hourly_plus_data_processing() {
    let id = LogicalId::new("nat");
    let cost = NatGatewayService
        .build_cost(
            &id,
            &rt("AWS::EC2::NatGateway"),
            &NatGatewaySpec {},
            &TestCatalog,
        )
        .expect("build cost");

    let params = params([(id.var("data_processed_gb"), 10.0)]);

    approx_expr(&cost.expr, &params, 750.0);
    approx_expr(&cost.components[0].expr, &params, 730.0);
    approx_expr(&cost.components[1].expr, &params, 20.0);
}

#[test]
fn opensearch_serverless_cost_honours_fractional_ocu() {
    let id = LogicalId::new("oss");
    let spec = OpenSearchServerlessSpec {
        collection_type: None,
    };
    let cost = OpenSearchServerlessService
        .build_cost(
            &id,
            &rt("AWS::OpenSearchServerless::Collection"),
            &spec,
            &TestCatalog,
        )
        .expect("build cost");

    let params = params([
        (id.var("indexing_ocu"), 1.0),
        (id.var("search_ocu"), 1.0),
        (id.var("storage_gb"), 10.0),
    ]);

    // No 2-OCU floor: fractional/sub-2-OCU inputs are honoured verbatim.
    //   indexing = 1.0 OCU * 1.0/h * 730 = 730; search = 730; storage = 10 * 2.0 = 20.
    approx_expr(&cost.expr, &params, 1480.0);
    approx_expr(&cost.components[0].expr, &params, 730.0);
    approx_expr(&cost.components[1].expr, &params, 730.0);
    approx_expr(&cost.components[2].expr, &params, 20.0);
}

#[test]
fn opensearch_service_cost_is_instances_plus_per_node_storage() {
    let id = LogicalId::new("domain");
    let spec = OpenSearchServiceSpec {
        instance_type: "t3.small.search".to_string(),
        instance_count: None,
        storage_gb: None,
    };
    let cost = OpenSearchServiceService
        .build_cost(
            &id,
            &rt("AWS::OpenSearchService::Domain"),
            &spec,
            &TestCatalog,
        )
        .expect("build cost");

    let params = params([
        (id.var("instance_count"), 2.0),
        (id.var("storage_gb"), 10.0),
    ]);

    approx_expr(&cost.expr, &params, 1500.0);
    approx_expr(&cost.components[0].expr, &params, 1460.0);
    approx_expr(&cost.components[1].expr, &params, 40.0);
}

#[test]
fn rds_aurora_cost_uses_hardcoded_storage_rate() {
    let id = LogicalId::new("aurora");
    let spec = RdsSpec {
        instance_type: "db.t3.small".to_string(),
        engine: RdsEngine::AuroraMysql,
        allocated_storage_gb: 100.0,
        storage_type: "gp2".to_string(),
        iops: None,
        multi_az: false,
    };
    let cost = RdsService
        .build_cost(&id, &rt("AWS::RDS::DBInstance"), &spec, &TestCatalog)
        .expect("build cost");

    let params = params([(id.var("aurora_storage_gb"), 10.0)]);

    approx_expr(&cost.expr, &params, 731.2);
    approx_expr(&cost.components[0].expr, &params, 730.0);
    approx_expr(&cost.components[1].expr, &params, 1.2);
}

#[test]
fn rds_gp3_cost_uses_hardcoded_gp3_formula() {
    let id = LogicalId::new("rds_gp3");
    let spec = RdsSpec {
        instance_type: "db.t3.small".to_string(),
        engine: RdsEngine::Postgres,
        allocated_storage_gb: 100.0,
        storage_type: "gp3".to_string(),
        iops: Some(4000.0),
        multi_az: true,
    };
    let cost = RdsService
        .build_cost(&id, &rt("AWS::RDS::DBInstance"), &spec, &TestCatalog)
        .expect("build cost");

    let params = params([]);

    // multi_az: true doubles both instance and storage (primary + standby).
    //   storage per-AZ = 0.1216 * 100 + (4000 - 3000) * 0.008 = 20.16
    //   storage total  = 20.16 * 2 = 40.32
    approx_expr(&cost.expr, &params, 1500.32);
    approx_expr(&cost.components[0].expr, &params, 1460.0);
    approx_expr(&cost.components[1].expr, &params, 40.32);
}

#[test]
fn redshift_cost_scales_with_node_count() {
    let id = LogicalId::new("redshift");
    let spec = RedshiftSpec {
        node_type: "ra3.xlplus".to_string(),
        node_count: None,
        hours: None, // defaults to a full month (730)
    };
    let cost = RedshiftService
        .build_cost(&id, &rt("AWS::Redshift::Cluster"), &spec, &TestCatalog)
        .expect("build cost");

    // node_hour(1.0) x node_count(2) x hours(730 default) = 1460; storage/spectrum = 0
    let params = params([
        (id.var("node_count"), 2.0),
        (id.var("storage_gb"), 0.0),
        (id.var("spectrum_tb"), 0.0),
    ]);

    approx_expr(&cost.expr, &params, 1460.0);
    approx_expr(&cost.components[0].expr, &params, 1460.0);
}

#[test]
fn route53_cost_is_hosted_zone_plus_queries() {
    let id = LogicalId::new("zone");
    let spec = Route53HostedZoneSpec {
        zone_type: "Public".to_string(),
    };
    let cost = Route53Service
        .build_cost(&id, &rt("AWS::Route53::HostedZone"), &spec, &TestCatalog)
        .expect("build cost");

    let params = params([(id.var("queries"), 10.0)]);

    approx_expr(&cost.expr, &params, 15.0);
    approx_expr(&cost.components[0].expr, &params, 5.0);
    approx_expr(&cost.components[1].expr, &params, 10.0);
}

#[test]
fn s3_cost_is_storage_plus_puts_and_gets() {
    let id = LogicalId::new("bucket");
    let spec = S3Spec {
        versioning_enabled: false,
        storage_class: None,
    };
    let cost = S3Service
        .build_cost(&id, &rt("AWS::S3::Bucket"), &spec, &TestCatalog)
        .expect("build cost");

    let params = params([
        (id.var("storage_gb"), 150.0),
        (id.var("put_requests"), 10.0),
        (id.var("get_requests"), 10.0),
    ]);

    approx_expr(&cost.expr, &params, 100.0);
    approx_expr(&cost.components[0].expr, &params, 50.0);
    approx_expr(&cost.components[1].expr, &params, 20.0);
    approx_expr(&cost.components[2].expr, &params, 30.0);
}

#[test]
fn secrets_manager_cost_is_secret_fee_plus_api_calls() {
    let id = LogicalId::new("secret");
    let cost = SecretsManagerService
        .build_cost(
            &id,
            &rt("AWS::SecretsManager::Secret"),
            &SecretsManagerSpec {},
            &TestCatalog,
        )
        .expect("build cost");

    let params = params([(id.var("api_calls"), 20.0)]);

    approx_expr(&cost.expr, &params, 25.0);
    approx_expr(&cost.components[0].expr, &params, 5.0);
    approx_expr(&cost.components[1].expr, &params, 20.0);
}

#[test]
fn sns_delivery_cost_matches_absolute_amount() {
    let id = LogicalId::new("topic");
    let spec = SnsSpec { fifo: false };
    let cost = SnsService
        .build_cost(&id, &rt("AWS::SNS::Topic"), &spec, &TestCatalog)
        .expect("build cost");

    let params = params([(id.var("deliveries"), 25.0)]);

    approx_expr(&cost.expr, &params, 15.0);
}

#[test]
fn sqs_standard_cost_matches_absolute_amount() {
    let id = LogicalId::new("queue_standard");
    let spec = SqsSpec { fifo: false };
    let cost = SqsService
        .build_cost(&id, &rt("AWS::SQS::Queue"), &spec, &TestCatalog)
        .expect("build cost");

    let params = params([(id.var("requests"), 25.0)]);

    approx_expr(&cost.expr, &params, 15.0);
}

#[test]
fn sqs_fifo_cost_uses_fifo_rate() {
    let id = LogicalId::new("queue_fifo");
    let spec = SqsSpec { fifo: true };
    let cost = SqsService
        .build_cost(&id, &rt("AWS::SQS::Queue"), &spec, &TestCatalog)
        .expect("build cost");

    let params = params([(id.var("requests"), 25.0)]);

    approx_expr(&cost.expr, &params, 30.0);
}

#[test]
fn step_functions_standard_cost_matches_transitions_over_free_tier() {
    let id = LogicalId::new("sfn_standard");
    let spec = StepFunctionsSpec {
        workflow_type: StepFunctionsType::Standard,
    };
    let cost = StepFunctionsService
        .build_cost(
            &id,
            &rt("AWS::StepFunctions::StateMachine"),
            &spec,
            &TestCatalog,
        )
        .expect("build cost");

    let params = params([(id.var("transitions"), 25.0)]);

    approx_expr(&cost.expr, &params, 30.0);
}

#[test]
fn step_functions_express_cost_is_requests_plus_duration() {
    let id = LogicalId::new("sfn_express");
    let spec = StepFunctionsSpec {
        workflow_type: StepFunctionsType::Express,
    };
    let cost = StepFunctionsService
        .build_cost(
            &id,
            &rt("AWS::StepFunctions::StateMachine"),
            &spec,
            &TestCatalog,
        )
        .expect("build cost");

    let params = params([
        (id.var("requests"), 10.0),
        (id.var("duration_gb_seconds"), 5.0),
    ]);

    approx_expr(&cost.expr, &params, 40.0);
    approx_expr(&cost.components[0].expr, &params, 30.0);
    approx_expr(&cost.components[1].expr, &params, 10.0);
}

#[test]
fn waf_cost_is_acl_rules_and_requests() {
    let id = LogicalId::new("waf");
    let spec = WafSpec { rule_count: None };
    let cost = WafService
        .build_cost(&id, &rt("AWS::WAFv2::WebACL"), &spec, &TestCatalog)
        .expect("build cost");

    let params = params([(id.var("rule_count"), 3.0), (id.var("requests"), 10.0)]);

    approx_expr(&cost.expr, &params, 21.0);
    approx_expr(&cost.components[0].expr, &params, 5.0);
    approx_expr(&cost.components[1].expr, &params, 6.0);
    approx_expr(&cost.components[2].expr, &params, 10.0);
}
