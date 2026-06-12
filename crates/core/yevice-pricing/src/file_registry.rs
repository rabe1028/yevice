//! Pricing registry backed by downloaded Bulk API JSON files.
//! Falls back to hardcoded values if files are not available.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::bulk_api::{
    PricingEntry, find_entries, find_entries_by_family, first_price, parse_bulk_pricing,
};
use crate::error::PricingError;
use crate::model::*;

/// The region used by the hardcoded fallback values.  When the requested
/// region matches this constant, no warning is emitted.
const HARDCODED_REGION: &str = "ap-northeast-1";

/// Pricing registry that loads from downloaded JSON files.
pub struct FilePricingRegistry {
    pub region: String,
    #[allow(dead_code)]
    data_dir: PathBuf,
    pub fallback: crate::registry::PricingRegistry,
    /// Map from service key (e.g. `"lambda"`, `"ec2"`) to parsed pricing entries.
    services: HashMap<String, Vec<PricingEntry>>,
    /// Tracks which services have already emitted a fallback warning so each
    /// service only warns once per registry instance.
    warned_services: Mutex<HashSet<String>>,
}

impl FilePricingRegistry {
    pub fn load(region: impl Into<String>, data_dir: impl Into<PathBuf>) -> Self {
        let data_dir = data_dir.into();
        let region = region.into();

        let fallback = crate::registry::PricingRegistry::new(&region);

        let service_names = [
            "lambda",
            "ec2",
            "rds",
            "s3",
            "dynamodb",
            "ecs",
            "kinesis",
            "sqs",
            "cloudwatch",
        ];

        let mut services: HashMap<String, Vec<PricingEntry>> = HashMap::new();
        for name in &service_names {
            if let Some(entries) = load_service(&data_dir, name) {
                services.insert(name.to_string(), entries);
            }
        }

        let loaded_count = services.len();
        tracing::info!(
            data_dir = %data_dir.display(),
            loaded_count,
            "loaded pricing data from files"
        );

        Self {
            region,
            data_dir,
            fallback,
            services,
            warned_services: Mutex::new(HashSet::new()),
        }
    }

    /// Emit a `tracing::warn!` the first time `service_key` falls back to
    /// hardcoded pricing for a non-ap-northeast-1 region.
    fn warn_fallback_once(&self, service_key: &str) {
        if self.region == HARDCODED_REGION {
            return;
        }
        // Only warn once per service per registry instance.
        let mut warned = self
            .warned_services
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if warned.insert(service_key.to_string()) {
            tracing::warn!(
                service = service_key,
                requested_region = %self.region,
                fallback_region = HARDCODED_REGION,
                "no Bulk API pricing data for this service/region; \
                 using hardcoded {} prices",
                HARDCODED_REGION,
            );
        }
    }

    /// Returns loaded entries for `service_key`, or `None` if that file was
    /// not loaded.
    fn entries(&self, service_key: &str) -> Option<&Vec<PricingEntry>> {
        self.services.get(service_key)
    }

    pub fn lambda_price(&self) -> LambdaPrice {
        if let Some(entries) = self.entries("lambda") {
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
        self.warn_fallback_once("lambda");
        self.fallback.lambda_price()
    }

    pub fn ec2_price(&self, instance_type: &str) -> Result<Ec2Price, PricingError> {
        if let Some(entries) = self.entries("ec2") {
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
        self.warn_fallback_once("ec2");
        self.fallback.ec2_price(instance_type)
    }

    pub fn rds_price(&self, instance_type: &str, engine: &str) -> Result<RdsPrice, PricingError> {
        if let Some(entries) = self.entries("rds") {
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

        self.warn_fallback_once("rds");
        self.fallback.rds_price(instance_type, engine)
    }

    /// RDS gp3 storage price per GB-month.
    ///
    /// Looks for a `Database Storage` entry with `volumeType = "General Purpose-GP3"` in
    /// the downloaded `rds.json` file.  Falls back to the hardcoded ap-northeast-1 constant
    /// when the file is absent or the SKU is not found, and emits a region-fallback warning
    /// (suppressed for ap-northeast-1) using a dedicated dedup key so that it fires
    /// independently of the RDS instance-pricing fallback.
    pub fn rds_gp3_storage_price(&self) -> f64 {
        if let Some(entries) = self.entries("rds") {
            let matches = find_entries_by_family(
                entries,
                "Database Storage",
                &[
                    ("volumeType", "General Purpose-GP3"),
                    ("deploymentOption", "Single-AZ"),
                ],
            );
            if let Some(entry) = matches.first()
                && let Some(price) = first_price(entry)
            {
                return price;
            }
        }
        self.warn_fallback_once("rds_gp3_storage");
        self.fallback.rds_gp3_storage_price()
    }

    /// RDS gp3 excess IOPS price per IOPS-month.
    ///
    /// Looks for a `System Operation` entry with `group = "RDS-GP3-IOPS"` in the downloaded
    /// `rds.json` file.  Falls back to the hardcoded ap-northeast-1 constant when the file
    /// is absent or the SKU is not found, and emits a region-fallback warning (suppressed for
    /// ap-northeast-1) using a dedicated dedup key so that it fires independently of the
    /// RDS instance-pricing fallback.
    pub fn rds_gp3_iops_price(&self) -> f64 {
        if let Some(entries) = self.entries("rds") {
            let matches = find_entries(
                entries,
                &[("group", "RDS-GP3-IOPS"), ("deploymentOption", "Single-AZ")],
            );
            if let Some(entry) = matches.first()
                && let Some(price) = first_price(entry)
            {
                return price;
            }
        }
        self.warn_fallback_once("rds_gp3_iops");
        self.fallback.rds_gp3_iops_price()
    }

    pub fn s3_price(&self) -> S3Price {
        // S3 pricing has complex tiered structure, use hardcoded for now
        self.warn_fallback_once("s3");
        self.fallback.s3_price()
    }

    pub fn dynamodb_price(&self) -> DynamoDbPrice {
        if let Some(entries) = self.entries("dynamodb") {
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

        self.warn_fallback_once("dynamodb");
        self.fallback.dynamodb_price()
    }

    pub fn fargate_price(&self) -> FargatePrice {
        self.warn_fallback_once("fargate");
        self.fallback.fargate_price()
    }

    pub fn opensearch_serverless_price(&self) -> OpenSearchServerlessPrice {
        self.fallback.opensearch_serverless_price()
    }

    pub fn kinesis_price(&self) -> KinesisPrice {
        if let Some(entries) = self.entries("kinesis") {
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

        self.warn_fallback_once("kinesis");
        self.fallback.kinesis_price()
    }

    pub fn sqs_price(&self) -> SqsPrice {
        self.warn_fallback_once("sqs");
        self.fallback.sqs_price()
    }

    pub fn cloudwatch_logs_price(&self) -> CloudWatchLogsPrice {
        self.warn_fallback_once("cloudwatch_logs");
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

    /// Returns `true` when no Bulk API file was loaded for `service_key`.
    /// Useful for tests that need to verify fallback behavior.
    #[cfg(test)]
    pub fn is_fallback_for(&self, service_key: &str) -> bool {
        !self.services.contains_key(service_key)
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

    /// When a non-ap-northeast-1 region is requested with a file-backed
    /// registry that has no gp3 SKU in its rds.json, both gp3 price helpers
    /// must fall back to the hardcoded constant and record independent warning
    /// dedup entries (distinct from the "rds" instance-pricing key).
    #[test]
    fn rds_gp3_fallback_warning_fires_for_non_tokyo_region() {
        let dir = make_temp_dir("rds_gp3_fallback");
        // Write an rds.json with no gp3 entries so the lookup path returns None.
        std::fs::write(
            dir.join("rds.json"),
            minimal_bulk_json_with_wrong_group("AmazonRDS"),
        )
        .unwrap();

        let reg = FilePricingRegistry::load("us-east-1", &dir);

        // Both helpers must return the canonical hardcoded constants.
        assert_eq!(
            reg.rds_gp3_storage_price(),
            crate::registry::PricingRegistry::RDS_GP3_STORAGE_PRICE,
            "rds_gp3_storage_price must fall back to canonical constant"
        );
        assert_eq!(
            reg.rds_gp3_iops_price(),
            crate::registry::PricingRegistry::RDS_GP3_IOPS_PRICE,
            "rds_gp3_iops_price must fall back to canonical constant"
        );

        // Both fallback paths must have fired their dedup warnings.
        let warned = reg.warned_services.lock().unwrap();
        assert!(
            warned.contains("rds_gp3_storage"),
            "rds_gp3_storage should be in the warned set"
        );
        assert!(
            warned.contains("rds_gp3_iops"),
            "rds_gp3_iops should be in the warned set"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// gp3 fallback warning must be suppressed when the region is ap-northeast-1.
    #[test]
    fn rds_gp3_fallback_warning_suppressed_for_hardcoded_region() {
        let dir = make_temp_dir("rds_gp3_no_warn_tokyo");
        std::fs::write(
            dir.join("rds.json"),
            minimal_bulk_json_with_wrong_group("AmazonRDS"),
        )
        .unwrap();

        let reg = FilePricingRegistry::load("ap-northeast-1", &dir);

        let _ = reg.rds_gp3_storage_price();
        let _ = reg.rds_gp3_iops_price();

        let warned = reg.warned_services.lock().unwrap();
        assert!(
            !warned.contains("rds_gp3_storage"),
            "no gp3 storage warning should be emitted for ap-northeast-1"
        );
        assert!(
            !warned.contains("rds_gp3_iops"),
            "no gp3 iops warning should be emitted for ap-northeast-1"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// When a non-ap-northeast-1 region is requested and no Bulk API file is
    /// available for a service, the registry should use hardcoded prices and
    /// record that warning dedup has fired for that service.
    #[test]
    fn fallback_warning_dedup_fires_once_for_non_tokyo_region() {
        let dir = make_temp_dir("warn_dedup");
        // No lambda.json in dir -> full fallback path
        let reg = FilePricingRegistry::load("us-east-1", &dir);

        // Call lambda_price twice; the second call must not add a second entry.
        let _ = reg.lambda_price();
        let _ = reg.lambda_price();

        let warned = reg.warned_services.lock().unwrap();
        assert!(
            warned.contains("lambda"),
            "lambda should be in the warned set"
        );
        assert_eq!(warned.len(), 1, "only one service should have been warned");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// When the requested region equals ap-northeast-1, no warning is emitted
    /// even if no Bulk API file is present.
    #[test]
    fn fallback_warning_suppressed_for_hardcoded_region() {
        let dir = make_temp_dir("no_warn_tokyo");
        // No lambda.json in dir -> full fallback path, but region matches
        let reg = FilePricingRegistry::load("ap-northeast-1", &dir);

        let _ = reg.lambda_price();

        let warned = reg.warned_services.lock().unwrap();
        assert!(
            warned.is_empty(),
            "no warnings should be emitted for ap-northeast-1"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
