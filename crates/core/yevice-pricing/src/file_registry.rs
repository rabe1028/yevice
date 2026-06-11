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
                .unwrap_or_else(|| {
                    tracing::warn!(
                        service = "lambda",
                        group = "AWS-Lambda-Requests",
                        "pricing group not found in downloaded file; using hardcoded fallback"
                    );
                    crate::registry::PricingRegistry::LAMBDA_REQUEST_PRICE
                });

            let gb_second_price = find_entries(entries, &[("group", "AWS-Lambda-Duration")])
                .first()
                .and_then(|e| first_price(e))
                .unwrap_or_else(|| {
                    tracing::warn!(
                        service = "lambda",
                        group = "AWS-Lambda-Duration",
                        "pricing group not found in downloaded file; using hardcoded fallback"
                    );
                    crate::registry::PricingRegistry::LAMBDA_GB_SECOND_PRICE
                });

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
                .unwrap_or_else(|| {
                    tracing::warn!(
                        service = "dynamodb",
                        group = "DDB-WriteUnits",
                        "pricing group not found in downloaded file; using hardcoded fallback"
                    );
                    crate::registry::PricingRegistry::DYNAMODB_WRITE_REQUEST_PRICE
                });

            let read_price = find_entries(entries, &[("group", "DDB-ReadUnits")])
                .first()
                .and_then(|e| first_price(e))
                .unwrap_or_else(|| {
                    tracing::warn!(
                        service = "dynamodb",
                        group = "DDB-ReadUnits",
                        "pricing group not found in downloaded file; using hardcoded fallback"
                    );
                    crate::registry::PricingRegistry::DYNAMODB_READ_REQUEST_PRICE
                });

            let storage_price = find_entries(entries, &[("group", "DDB-Storage")])
                .first()
                .and_then(|e| first_price(e))
                .unwrap_or_else(|| {
                    tracing::warn!(
                        service = "dynamodb",
                        group = "DDB-Storage",
                        "pricing group not found in downloaded file; using hardcoded fallback"
                    );
                    crate::registry::PricingRegistry::DYNAMODB_STORAGE_PRICE
                });

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
                .unwrap_or_else(|| {
                    tracing::warn!(
                        service = "kinesis",
                        group = "Kinesis Streams",
                        "pricing group not found in downloaded file; using hardcoded fallback"
                    );
                    crate::registry::PricingRegistry::KINESIS_SHARD_HOUR_PRICE
                });

            let put_price =
                find_entries(entries, &[("group", "Kinesis Streams PUT Payload Units")])
                    .first()
                    .and_then(|e| first_price(e))
                    .unwrap_or_else(|| {
                        tracing::warn!(
                            service = "kinesis",
                            group = "Kinesis Streams PUT Payload Units",
                            "pricing group not found in downloaded file; using hardcoded fallback"
                        );
                        crate::registry::PricingRegistry::KINESIS_PUT_PAYLOAD_UNIT_PRICE
                    });

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
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!(service = name, "pricing file not found; skipping");
            None
        }
        Err(e) => {
            tracing::warn!(service = name, error = %e, "failed to read pricing file");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal Bulk Pricing JSON fixture with a single SKU whose
    /// `group` attribute is deliberately set to `WRONG_GROUP_NAME`.  When the
    /// registry looks up the real group name, `find_entries` returns empty,
    /// which triggers the per-group fallback path.
    fn minimal_bulk_json_with_wrong_group(offer_code: &str) -> String {
        // Assemble as concatenated parts so we never need to escape braces.
        [
            r#"{"offerCode":""#,
            offer_code,
            // The closing brace count (right-to-left):
            // pricePerUnit  dim-entry  priceDimensions  offerTerm  SKU001-on-demand  OnDemand  terms  outer
            r#"","products":{"SKU001":{"sku":"SKU001","productFamily":"Compute","attributes":{"group":"WRONG_GROUP_NAME"}}},"terms":{"OnDemand":{"SKU001":{"SKU001.JRTCKXETXF":{"sku":"SKU001","priceDimensions":{"SKU001.JRTCKXETXF.6YS6EN2CT7":{"description":"per request","beginRange":"0","endRange":"Inf","unit":"Requests","pricePerUnit":{"USD":"0.000001"}}}}}}}}}}"#,
        ]
        .concat()
    }

    /// Create a uniquely-named temporary directory (no external crate needed).
    fn make_temp_dir(suffix: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "yevice_pricing_test_{}_{suffix}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    /// When a downloaded Lambda pricing file loads successfully but has no
    /// `AWS-Lambda-Requests` group, the fallback must equal the canonical constant.
    #[test]
    fn lambda_missing_group_returns_canonical_fallback() {
        let dir = make_temp_dir("lambda");
        std::fs::write(
            dir.join("lambda.json"),
            minimal_bulk_json_with_wrong_group("AmazonLambda"),
        )
        .unwrap();

        let reg = FilePricingRegistry::load("ap-northeast-1", &dir);
        let price = reg.lambda_price();

        assert_eq!(
            price.request_price,
            crate::registry::PricingRegistry::LAMBDA_REQUEST_PRICE,
            "lambda request_price must fall back to canonical constant"
        );
        assert_eq!(
            price.gb_second_price,
            crate::registry::PricingRegistry::LAMBDA_GB_SECOND_PRICE,
            "lambda gb_second_price must fall back to canonical constant"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// When a downloaded DynamoDB pricing file loads successfully but has no
    /// `DDB-WriteUnits` group, prices must equal the canonical constants.
    #[test]
    fn dynamodb_missing_group_returns_canonical_fallback() {
        let dir = make_temp_dir("dynamodb");
        std::fs::write(
            dir.join("dynamodb.json"),
            minimal_bulk_json_with_wrong_group("AmazonDynamoDB"),
        )
        .unwrap();

        let reg = FilePricingRegistry::load("ap-northeast-1", &dir);
        let price = reg.dynamodb_price();

        assert_eq!(
            price.write_request_price,
            crate::registry::PricingRegistry::DYNAMODB_WRITE_REQUEST_PRICE,
            "dynamodb write_request_price must fall back to canonical constant"
        );
        assert_eq!(
            price.read_request_price,
            crate::registry::PricingRegistry::DYNAMODB_READ_REQUEST_PRICE,
            "dynamodb read_request_price must fall back to canonical constant"
        );
        assert_eq!(
            price.storage_price_per_gb,
            crate::registry::PricingRegistry::DYNAMODB_STORAGE_PRICE,
            "dynamodb storage_price_per_gb must fall back to canonical constant"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// When a downloaded Kinesis pricing file loads successfully but has no
    /// `Kinesis Streams` group, prices must equal the canonical constants.
    #[test]
    fn kinesis_missing_group_returns_canonical_fallback() {
        let dir = make_temp_dir("kinesis");
        std::fs::write(
            dir.join("kinesis.json"),
            minimal_bulk_json_with_wrong_group("AmazonKinesis"),
        )
        .unwrap();

        let reg = FilePricingRegistry::load("ap-northeast-1", &dir);
        let price = reg.kinesis_price();

        assert_eq!(
            price.shard_hour_price,
            crate::registry::PricingRegistry::KINESIS_SHARD_HOUR_PRICE,
            "kinesis shard_hour_price must fall back to canonical constant"
        );
        assert_eq!(
            price.put_payload_unit_price,
            crate::registry::PricingRegistry::KINESIS_PUT_PAYLOAD_UNIT_PRICE,
            "kinesis put_payload_unit_price must fall back to canonical constant"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
