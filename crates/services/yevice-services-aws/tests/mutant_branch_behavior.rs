use std::collections::HashMap;

use yevice_core::{
    capacity::{QuotaType, RegionQuotas, Severity},
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
    batch::{BatchEbsConfig, BatchJobDefinitionSpec, BatchLaunchType, BatchService},
    dynamodb::{DynamoDbBillingMode, DynamoDbService, DynamoDbSpec},
    kinesis::{KinesisService, KinesisSpec, KinesisStreamMode},
    lambda::{LambdaService, LambdaSpec},
    rds::{RdsEngine, RdsService, RdsSpec},
};

struct BranchCatalog;

impl PriceCatalog for BranchCatalog {
    fn region(&self) -> &'static str {
        "test"
    }

    fn lookup(&self, sku: &Sku) -> Result<PriceRecord, PricingError> {
        let price = match sku.as_str() {
            "aws.batch.fargate_vcpu_hour_price" => 2.3,
            "aws.batch.fargate_memory_gb_hour_price" => 3.7,
            "aws.batch.fargate_ephemeral_storage_gb_hour_price" => 0.6,
            "aws.batch.fargate_ephemeral_free_gb" => 17.0,
            "aws.batch.ebs_gp3_gb_month_price" => 4.1,
            "aws.batch.ebs_gp3_iops_month_price" => 5.3,
            "aws.batch.ebs_gp3_iops_free" => 11.0,
            "aws.batch.ebs_gp3_throughput_mibps_month_price" => 6.7,
            "aws.batch.ebs_gp3_throughput_free_mibps" => 19.0,
            sku if sku.starts_with("aws.rds_storage.") => 8.9,
            sku if sku.starts_with("aws.rds.") => 2.7,
            other => {
                return Err(PricingError::NotFound {
                    service: other.to_string(),
                    region: "test".to_string(),
                });
            }
        };

        Ok(PriceRecord::flat(price))
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
    HashMap::from(entries)
}

fn test_quotas() -> RegionQuotas {
    RegionQuotas {
        lambda_concurrent_executions: 77.0,
        dynamodb_max_wcu_per_table: 100.0,
        dynamodb_max_rcu_per_table: 130.0,
        dynamodb_max_tables: 230.0,
        kinesis_max_shards_per_stream: 50.0,
        kinesis_max_records_per_sec_per_shard: 250.0,
        kinesis_max_mb_per_sec_per_shard: 2.5,
    }
}

#[test]
fn service_ids_match_expected_catalog_keys() {
    assert_eq!(BatchService.id(), "aws.batch");
    assert_eq!(DynamoDbService.id(), "aws.dynamodb");
    assert_eq!(KinesisService.id(), "aws.kinesis");
    assert_eq!(LambdaService.id(), "aws.lambda");
    assert_eq!(RdsService.id(), "aws.rds");
}

#[test]
fn batch_ebs_storage_uses_all_monthly_components() {
    let id = LogicalId::new("job");
    let params = params([
        (id.var("executions"), 4.0),
        (id.var("avg_duration_sec"), 1800.0),
    ]);
    let cost = BatchService
        .build_cost(
            &id,
            &rt("AWS::Batch::JobDefinition"),
            &BatchJobDefinitionSpec {
                launch_type: BatchLaunchType::Ec2,
                vcpu: 2.5,
                memory_gb: 3.5,
                ephemeral_storage_gb: None,
                ebs: Some(BatchEbsConfig {
                    size_gb: 7.0,
                    volume_type: "gp3".to_string(),
                    iops: Some(13.0),
                    throughput_mibps: Some(23.0),
                }),
            },
            &BranchCatalog,
        )
        .expect("build cost");

    assert_eq!(cost.components[1].name, "EBS Storage (gp3)");
    approx_expr(&cost.components[0].expr, &params, 37.4);
    approx_expr(&cost.components[1].expr, &params, 0.1810958904109589);
    approx_expr(&cost.expr, &params, 37.58109589041096);
}

#[test]
fn batch_storage_branches_cover_three_ephemeral_boundaries_and_ec2_zero_arm() {
    let id = LogicalId::new("job");
    let params = params([
        (id.var("executions"), 4.0),
        (id.var("avg_duration_sec"), 1800.0),
    ]);

    for (ephemeral_gb, expected_storage) in [(16.0, 0.0), (17.0, 0.0), (18.0, 1.2)] {
        let cost = BatchService
            .build_cost(
                &id,
                &rt("AWS::Batch::JobDefinition"),
                &BatchJobDefinitionSpec {
                    launch_type: BatchLaunchType::Fargate,
                    vcpu: 2.5,
                    memory_gb: 3.5,
                    ephemeral_storage_gb: Some(ephemeral_gb),
                    ebs: None,
                },
                &BranchCatalog,
            )
            .expect("build cost");

        assert_eq!(cost.components[1].name, "Ephemeral Storage");
        approx_expr(&cost.components[1].expr, &params, expected_storage);
        if ephemeral_gb <= 17.0 {
            assert_eq!(cost.components[1].expr, Expr::constant(0.0));
        }
    }

    let ec2_without_storage = BatchService
        .build_cost(
            &id,
            &rt("AWS::Batch::JobDefinition"),
            &BatchJobDefinitionSpec {
                launch_type: BatchLaunchType::Ec2,
                vcpu: 2.5,
                memory_gb: 3.5,
                ephemeral_storage_gb: Some(23.0),
                ebs: None,
            },
            &BranchCatalog,
        )
        .expect("build cost");
    assert_eq!(ec2_without_storage.components[1].expr, Expr::constant(0.0));
    approx_expr(&ec2_without_storage.components[1].expr, &params, 0.0);
}

#[test]
fn dynamodb_capacity_covers_empty_provisioned_on_demand_and_quota_boundary() {
    let service = DynamoDbService;
    let id = LogicalId::new("table");
    let quotas = test_quotas();

    assert!(
        service
            .build_capacity(
                &id,
                &DynamoDbSpec {
                    billing_mode: DynamoDbBillingMode::Provisioned {
                        write_capacity_units: None,
                        read_capacity_units: None,
                    },
                    has_stream: false,
                    gsi_count: 0,
                },
                &quotas,
            )
            .is_none()
    );

    let required = params([
        (id.var("peak_writes_per_sec"), 73.0),
        (id.var("peak_reads_per_sec"), 29.0),
    ]);

    for (wcu, expected_constraints, expect_warning) in [
        (79.0, 2_usize, false),
        (80.0, 2_usize, false),
        (81.0, 3_usize, true),
    ] {
        let model = service
            .build_capacity(
                &id,
                &DynamoDbSpec {
                    billing_mode: DynamoDbBillingMode::Provisioned {
                        write_capacity_units: Some(wcu),
                        read_capacity_units: Some(37.0),
                    },
                    has_stream: true,
                    gsi_count: 2,
                },
                &quotas,
            )
            .expect("capacity model");

        assert_eq!(model.label, "DynamoDB Provisioned: table");
        assert_eq!(model.constraints.len(), expected_constraints);
        let write = model
            .constraints
            .iter()
            .find(|constraint| constraint.dimension == "write_capacity_units")
            .expect("write constraint");
        approx_expr(&write.required, &required, 73.0);
        assert_eq!(write.limit, wcu);
        assert_eq!(write.quota_type, QuotaType::Soft);
        assert_eq!(write.severity, Severity::Error);

        let read = model
            .constraints
            .iter()
            .find(|constraint| constraint.dimension == "read_capacity_units")
            .expect("read constraint");
        approx_expr(&read.required, &required, 29.0);
        assert_eq!(read.limit, 37.0);

        let warning = model
            .constraints
            .iter()
            .find(|constraint| constraint.dimension == "wcu_quota");
        assert_eq!(warning.is_some(), expect_warning);
        if let Some(warning) = warning {
            approx_expr(&warning.required, &Params::new(), wcu);
            assert_eq!(warning.limit, 100.0);
            assert_eq!(warning.severity, Severity::Warning);
        }
    }

    let on_demand = service
        .build_capacity(
            &id,
            &DynamoDbSpec {
                billing_mode: DynamoDbBillingMode::OnDemand,
                has_stream: false,
                gsi_count: 0,
            },
            &quotas,
        )
        .expect("capacity model");
    assert_eq!(on_demand.label, "DynamoDB On-Demand: table");
    assert_eq!(on_demand.constraints.len(), 1);
    assert_eq!(on_demand.constraints[0].dimension, "peak_writes_per_sec");
    approx_expr(
        &on_demand.constraints[0].required,
        &params([(id.var("peak_writes_per_sec"), 73.0)]),
        73.0,
    );
    assert_eq!(on_demand.constraints[0].limit, 40_000.0);
    assert_eq!(on_demand.constraints[0].severity, Severity::Warning);
}

#[test]
fn kinesis_capacity_covers_non_provisioned_missing_count_and_quota_boundary() {
    let service = KinesisService;
    let id = LogicalId::new("stream");
    let quotas = test_quotas();

    assert!(
        service
            .build_capacity(
                &id,
                &KinesisSpec {
                    stream_mode: KinesisStreamMode::OnDemand,
                    retention_hours: 48.0,
                },
                &quotas,
            )
            .is_none()
    );

    assert!(
        service
            .build_capacity(
                &id,
                &KinesisSpec {
                    stream_mode: KinesisStreamMode::Provisioned { shard_count: None },
                    retention_hours: 48.0,
                },
                &quotas,
            )
            .is_none()
    );

    let required = params([
        (id.var("peak_ingestion_mb_per_sec"), 6.1),
        (id.var("peak_records_per_sec"), 725.0),
    ]);

    for (shard_count, expected_constraints, expect_warning) in [
        (39.0, 2_usize, false),
        (40.0, 2_usize, false),
        (41.0, 3_usize, true),
    ] {
        let model = service
            .build_capacity(
                &id,
                &KinesisSpec {
                    stream_mode: KinesisStreamMode::Provisioned {
                        shard_count: Some(shard_count),
                    },
                    retention_hours: 48.0,
                },
                &quotas,
            )
            .expect("capacity model");

        assert_eq!(model.label, "Kinesis Provisioned: stream");
        assert_eq!(model.constraints.len(), expected_constraints);
        assert_eq!(model.constraints[0].dimension, "shard_throughput");
        approx_expr(&model.constraints[0].required, &required, 3.0);
        assert_eq!(model.constraints[0].limit, shard_count);
        assert_eq!(model.constraints[1].dimension, "shard_record_rate");
        approx_expr(&model.constraints[1].required, &required, 3.0);
        assert_eq!(model.constraints[1].limit, shard_count);

        let warning = model
            .constraints
            .iter()
            .find(|constraint| constraint.dimension == "shard_quota");
        assert_eq!(warning.is_some(), expect_warning);
        if let Some(warning) = warning {
            approx_expr(&warning.required, &Params::new(), shard_count);
            assert_eq!(warning.limit, 50.0);
            assert_eq!(warning.severity, Severity::Warning);
        }
    }
}

#[test]
fn lambda_capacity_returns_scaled_concurrency_constraint() {
    let id = LogicalId::new("fn");
    let model = LambdaService
        .build_capacity(&id, &LambdaSpec::default(), &test_quotas())
        .expect("capacity model");

    assert_eq!(model.label, "Lambda: fn");
    assert_eq!(model.constraints.len(), 1);
    assert_eq!(model.constraints[0].dimension, "concurrent_executions");
    assert_eq!(model.constraints[0].limit, 77.0);
    assert_eq!(model.constraints[0].severity, Severity::Error);
    approx_expr(
        &model.constraints[0].required,
        &params([
            (id.var("peak_requests_per_sec"), 5.5),
            (id.var("avg_duration_ms"), 320.0),
        ]),
        1.76,
    );
}

#[test]
fn rds_engine_pricing_keys_cover_all_match_arms() {
    for (engine, expected) in [
        (RdsEngine::Mysql, "mysql"),
        (RdsEngine::Postgres, "postgres"),
        (RdsEngine::Mariadb, "mariadb"),
        (RdsEngine::AuroraMysql, "aurora-mysql"),
        (RdsEngine::AuroraPostgresql, "aurora-postgresql"),
        (
            RdsEngine::Other("custom-engine".to_string()),
            "custom-engine",
        ),
    ] {
        assert_eq!(engine.as_pricing_key(), expected);
    }
}

#[test]
fn rds_storage_formulas_cover_gp3_boundary_and_other_storage_arms() {
    let id = LogicalId::new("db");

    for (iops, expected_storage) in [(2999.0, 2.7968), (3000.0, 2.7968), (3001.0, 2.8048)] {
        let cost = RdsService
            .build_cost(
                &id,
                &rt("AWS::RDS::DBInstance"),
                &RdsSpec {
                    instance_type: "db.t3.small".to_string(),
                    engine: RdsEngine::Mysql,
                    allocated_storage_gb: 23.0,
                    storage_type: "gp3".to_string(),
                    iops: Some(iops),
                    multi_az: false,
                },
                &BranchCatalog,
            )
            .expect("build cost");
        approx_expr(&cost.components[0].expr, &Params::new(), 1971.0);
        approx_expr(&cost.components[1].expr, &Params::new(), expected_storage);
    }

    // multi_az: true -> storage is billed on both primary and standby (x2):
    //   io1: (0.142 * 29 + 13 * 0.074) * 2 = 5.08  * 2 = 10.16
    //   io2: (0.142 * 29 + 17 * 0.074) * 2 = 5.376 * 2 = 10.752
    for (storage_type, iops, expected_storage) in [("io1", 13.0, 10.16), ("io2", 17.0, 10.752)] {
        let cost = RdsService
            .build_cost(
                &id,
                &rt("AWS::RDS::DBInstance"),
                &RdsSpec {
                    instance_type: "db.t3.small".to_string(),
                    engine: RdsEngine::Mysql,
                    allocated_storage_gb: 29.0,
                    storage_type: storage_type.to_string(),
                    iops: Some(iops),
                    multi_az: true,
                },
                &BranchCatalog,
            )
            .expect("build cost");
        approx_expr(&cost.components[0].expr, &Params::new(), 3942.0);
        approx_expr(&cost.components[1].expr, &Params::new(), expected_storage);
    }

    let gp2 = RdsService
        .build_cost(
            &id,
            &rt("AWS::RDS::DBInstance"),
            &RdsSpec {
                instance_type: "db.t3.small".to_string(),
                engine: RdsEngine::Mysql,
                allocated_storage_gb: 11.0,
                storage_type: "gp2".to_string(),
                iops: None,
                multi_az: false,
            },
            &BranchCatalog,
        )
        .expect("build cost");
    approx_expr(&gp2.components[1].expr, &Params::new(), 97.9);
}
