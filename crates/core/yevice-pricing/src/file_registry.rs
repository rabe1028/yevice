//! Pricing registry backed by downloaded Bulk API JSON files.
//! Falls back to hardcoded values if files are not available.

use std::path::{Path, PathBuf};

use crate::bulk_api::{PricingEntry, find_entries, first_price, parse_bulk_pricing};
use crate::error::PricingError;
use crate::model::*;

/// Pricing registry that loads from downloaded JSON files.
pub struct FilePricingRegistry {
    pub region: String,
    #[allow(dead_code)]
    data_dir: PathBuf,
    pub fallback: crate::registry::PricingRegistry,
    lambda: Option<Vec<PricingEntry>>,
    ec2: Option<Vec<PricingEntry>>,
    rds: Option<Vec<PricingEntry>>,
    s3: Option<Vec<PricingEntry>>,
    dynamodb: Option<Vec<PricingEntry>>,
    ecs: Option<Vec<PricingEntry>>,
    kinesis: Option<Vec<PricingEntry>>,
    sqs: Option<Vec<PricingEntry>>,
    cloudwatch: Option<Vec<PricingEntry>>,
}

impl FilePricingRegistry {
    pub fn load(region: impl Into<String>, data_dir: impl Into<PathBuf>) -> Self {
        let data_dir = data_dir.into();
        let region = region.into();

        let fallback = crate::registry::PricingRegistry::new(&region);
        let mut reg = Self {
            region,
            data_dir: data_dir.clone(),
            fallback,
            lambda: None,
            ec2: None,
            rds: None,
            s3: None,
            dynamodb: None,
            ecs: None,
            kinesis: None,
            sqs: None,
            cloudwatch: None,
        };

        reg.lambda = load_service(&data_dir, "lambda");
        reg.ec2 = load_service(&data_dir, "ec2");
        reg.rds = load_service(&data_dir, "rds");
        reg.s3 = load_service(&data_dir, "s3");
        reg.dynamodb = load_service(&data_dir, "dynamodb");
        reg.ecs = load_service(&data_dir, "ecs");
        reg.kinesis = load_service(&data_dir, "kinesis");
        reg.sqs = load_service(&data_dir, "sqs");
        reg.cloudwatch = load_service(&data_dir, "cloudwatch");

        let loaded_count = [
            &reg.lambda,
            &reg.ec2,
            &reg.rds,
            &reg.s3,
            &reg.dynamodb,
            &reg.ecs,
            &reg.kinesis,
            &reg.sqs,
            &reg.cloudwatch,
        ]
        .iter()
        .filter(|e| e.is_some())
        .count();

        tracing::info!(
            data_dir = %data_dir.display(),
            loaded_count,
            "loaded pricing data from files"
        );

        reg
    }

    pub fn lambda_price(&self) -> LambdaPrice {
        if let Some(entries) = &self.lambda {
            let request_price = find_entries(entries, &[("group", "AWS-Lambda-Requests")])
                .first()
                .and_then(|e| first_price(e))
                .unwrap_or(0.0000002);

            let gb_second_price = find_entries(entries, &[("group", "AWS-Lambda-Duration")])
                .first()
                .and_then(|e| first_price(e))
                .unwrap_or(0.0000166667);

            return LambdaPrice {
                request_price,
                gb_second_price,
                free_tier_requests: 1_000_000.0,
                free_tier_gb_seconds: 400_000.0,
            };
        }

        // Fallback to hardcoded
        self.fallback.lambda_price()
    }

    pub fn ec2_price(&self, instance_type: &str) -> Result<Ec2Price, PricingError> {
        if let Some(entries) = &self.ec2 {
            let matches = find_entries(
                entries,
                &[
                    ("instanceType", instance_type),
                    ("operatingSystem", "Linux"),
                    ("tenancy", "Shared"),
                    ("preInstalledSw", "NA"),
                    ("capacitystatus", "Used"),
                ],
            );

            if let Some(entry) = matches.first()
                && let Some(price) = first_price(entry)
            {
                return Ok(Ec2Price {
                    instance_type: instance_type.to_string(),
                    hourly_price: price,
                });
            }
        }

        // Fallback
        self.fallback.ec2_price(instance_type)
    }

    pub fn rds_price(&self, instance_type: &str, engine: &str) -> Result<RdsPrice, PricingError> {
        if let Some(entries) = &self.rds {
            let db_engine = match engine {
                "mysql" | "mariadb" => "MySQL",
                "postgres" => "PostgreSQL",
                "aurora-mysql" => "Aurora MySQL",
                "aurora-postgresql" => "Aurora PostgreSQL",
                _ => engine,
            };

            let matches = find_entries(
                entries,
                &[
                    ("instanceType", instance_type),
                    ("databaseEngine", db_engine),
                ],
            );

            if let Some(entry) = matches.first()
                && let Some(price) = first_price(entry)
            {
                return Ok(RdsPrice {
                    instance_type: instance_type.to_string(),
                    hourly_price: price,
                    storage_price_per_gb: 0.138, // gp2 default
                });
            }
        }

        self.fallback.rds_price(instance_type, engine)
    }

    pub fn s3_price(&self) -> S3Price {
        // S3 pricing has complex tiered structure, use hardcoded for now
        self.fallback.s3_price()
    }

    pub fn dynamodb_price(&self) -> DynamoDbPrice {
        if let Some(entries) = &self.dynamodb {
            let write_price = find_entries(entries, &[("group", "DDB-WriteUnits")])
                .first()
                .and_then(|e| first_price(e))
                .unwrap_or(0.000000715);

            let read_price = find_entries(entries, &[("group", "DDB-ReadUnits")])
                .first()
                .and_then(|e| first_price(e))
                .unwrap_or(0.000000143);

            let storage_price = find_entries(entries, &[("group", "DDB-Storage")])
                .first()
                .and_then(|e| first_price(e))
                .unwrap_or(0.285);

            let fallback = self.fallback.dynamodb_price();
            return DynamoDbPrice {
                write_request_price: write_price,
                read_request_price: read_price,
                wcu_hour_price: fallback.wcu_hour_price,
                rcu_hour_price: fallback.rcu_hour_price,
                storage_price_per_gb: storage_price,
                free_tier_wru: 25_000.0,
                free_tier_rru: 25_000.0,
                free_tier_storage_gb: 25.0,
            };
        }

        self.fallback.dynamodb_price()
    }

    pub fn fargate_price(&self) -> FargatePrice {
        self.fallback.fargate_price()
    }

    pub fn opensearch_serverless_price(&self) -> OpenSearchServerlessPrice {
        self.fallback.opensearch_serverless_price()
    }

    pub fn kinesis_price(&self) -> KinesisPrice {
        if let Some(entries) = &self.kinesis {
            let shard_price = find_entries(entries, &[("group", "Kinesis Streams")])
                .iter()
                .find_map(|e| first_price(e))
                .unwrap_or(0.0195);

            let put_price =
                find_entries(entries, &[("group", "Kinesis Streams PUT Payload Units")])
                    .first()
                    .and_then(|e| first_price(e))
                    .unwrap_or(0.0000002);

            let fallback = self.fallback.kinesis_price();
            return KinesisPrice {
                shard_hour_price: shard_price,
                put_payload_unit_price: put_price,
                on_demand_ingestion_price_per_gb: fallback.on_demand_ingestion_price_per_gb,
                on_demand_retrieval_price_per_gb: fallback.on_demand_retrieval_price_per_gb,
                on_demand_stream_hour_price: fallback.on_demand_stream_hour_price,
            };
        }

        self.fallback.kinesis_price()
    }

    pub fn sqs_price(&self) -> SqsPrice {
        self.fallback.sqs_price()
    }

    pub fn cloudwatch_logs_price(&self) -> CloudWatchLogsPrice {
        self.fallback.cloudwatch_logs_price()
    }

    pub fn api_gateway_price(&self) -> ApiGatewayPrice {
        self.fallback.api_gateway_price()
    }

    pub fn nat_gateway_price(&self) -> NatGatewayPrice {
        self.fallback.nat_gateway_price()
    }

    pub fn cloudfront_price(&self) -> CloudFrontPrice {
        self.fallback.cloudfront_price()
    }

    pub fn elasticache_price(&self, node_type: &str) -> Result<ElastiCachePrice, PricingError> {
        self.fallback.elasticache_price(node_type)
    }

    pub fn step_functions_price(&self) -> StepFunctionsPrice {
        self.fallback.step_functions_price()
    }

    pub fn eventbridge_scheduler_price(&self) -> EventBridgeSchedulerPrice {
        self.fallback.eventbridge_scheduler_price()
    }

    pub fn batch_price(&self) -> BatchPrice {
        self.fallback.batch_price()
    }

    pub fn data_transfer_price(&self) -> DataTransferPrice {
        self.fallback.data_transfer_price()
    }
}

fn load_service(data_dir: &Path, name: &str) -> Option<Vec<PricingEntry>> {
    let path = data_dir.join(format!("{name}.json"));
    match std::fs::read(&path) {
        Ok(data) => match parse_bulk_pricing(&data) {
            Ok(entries) => {
                tracing::debug!(service = name, entries = entries.len(), "loaded pricing");
                Some(entries)
            }
            Err(e) => {
                tracing::warn!(service = name, error = %e, "failed to parse pricing file");
                None
            }
        },
        Err(_) => None,
    }
}
