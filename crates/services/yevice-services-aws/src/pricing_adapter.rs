//! Adapter that wraps `PricingRegistry` and implements `PriceCatalog`.
//!
//! Maps SKU strings (e.g. `"aws.lambda.gb_second"`) to the corresponding
//! method on `PricingRegistry`.
//!
//! When constructed with `with_data_dir` or `auto`, looks up Lambda/EC2/RDS/S3/
//! DynamoDB/ECS/Kinesis/SQS/CloudWatch prices from downloaded Bulk-API JSON
//! files (`pricing-data/*.json`); other services use the hardcoded fallback.

use std::path::{Path, PathBuf};

use yevice_core::resource::Provider;
use yevice_pricing::{
    catalog::{PriceCatalog, PricedValue, Sku},
    error::PricingError,
    file_registry::FilePricingRegistry,
    registry::PricingRegistry,
};
use yevice_service_api::PriceCatalogResolver;

use crate::pricing_provider::PricingProvider;

pub struct AwsPricingCatalog {
    /// Always populated; used as the fallback or as the sole source.
    memory: PricingRegistry,
    /// Optional file-backed registry (used for the services it knows about).
    file: Option<FilePricingRegistry>,
    /// When `true`, promotional AWS Free Tier allowances (`*free_tier*` SKUs)
    /// resolve to `0`, so costs reflect list prices. Product-included
    /// allocations (e.g. QuickSight `free_spice_gb`, gp3 baseline IOPS) are
    /// kept regardless. Mirrors how AWS's own CDP estimates ignore the
    /// promotional Free Tier.
    list_price: bool,
}

impl AwsPricingCatalog {
    /// Use hardcoded prices only.
    pub fn new(region: impl Into<String>) -> Self {
        let region = region.into();
        Self {
            memory: PricingRegistry::new(&region),
            file: None,
            list_price: false,
        }
    }

    /// Use downloaded pricing data from `data_dir` for supported services,
    /// falling back to hardcoded prices for everything else.
    pub fn with_data_dir(region: impl Into<String>, data_dir: impl Into<PathBuf>) -> Self {
        let region = region.into();
        Self {
            memory: PricingRegistry::new(&region),
            file: Some(FilePricingRegistry::load(&region, data_dir)),
            list_price: false,
        }
    }

    /// Enable list-price mode: zero out promotional AWS Free Tier allowances.
    #[must_use]
    pub fn with_list_price(mut self, list_price: bool) -> Self {
        self.list_price = list_price;
        self
    }

    /// Auto-select: use `pricing-data/` directory if present, otherwise hardcoded.
    pub fn auto(region: impl Into<String>) -> Self {
        let data_dir = Path::new("pricing-data");
        if data_dir.is_dir() {
            tracing::info!("using pricing data from {}", data_dir.display());
            Self::with_data_dir(region, data_dir)
        } else {
            Self::new(region)
        }
    }

    /// Returns the file-backed provider when available, otherwise the memory
    /// registry. Used for trait-defined price methods so downloaded data wins.
    fn provider(&self) -> &dyn PricingProvider {
        match &self.file {
            Some(f) => f,
            None => &self.memory,
        }
    }

    /// RDS gp3 storage price per GB-month, routed through the file registry so
    /// non-Tokyo regions emit a fallback warning consistent with other RDS paths.
    fn rds_gp3_storage_price(&self) -> f64 {
        match &self.file {
            Some(f) => f.rds_gp3_storage_price(),
            None => self.memory.rds_gp3_storage_price(),
        }
    }

    /// RDS gp3 excess IOPS price per IOPS-month, routed through the file
    /// registry so non-Tokyo regions emit a fallback warning consistent with
    /// other RDS paths.
    fn rds_gp3_iops_price(&self) -> f64 {
        match &self.file {
            Some(f) => f.rds_gp3_iops_price(),
            None => self.memory.rds_gp3_iops_price(),
        }
    }
}

impl PriceCatalog for AwsPricingCatalog {
    fn region(&self) -> &str {
        &self.memory.region
    }

    #[allow(clippy::too_many_lines)]
    fn lookup(&self, sku: &Sku) -> Result<PricedValue, PricingError> {
        // List-price mode: zero out promotional AWS Free Tier allowances.
        // Only `*free_tier*` SKUs are masked; product-included allocations such
        // as QuickSight `free_spice_gb` or Batch gp3 baseline (`*_free*`) are
        // intentionally kept.
        if self.list_price && sku.as_str().contains("free_tier") {
            return Ok(PricedValue::scalar(0.0, "USD"));
        }
        let record: Result<PricedValue, PricingError> = match sku.as_str() {
            // Lambda
            "aws.lambda.request_price" => Ok(PricedValue::scalar(
                self.provider().lambda_price().request_price,
                "USD",
            )),
            "aws.lambda.gb_second" => Ok(PricedValue::scalar(
                self.provider().lambda_price().gb_second_price,
                "USD",
            )),
            "aws.lambda.http_stream_gb" => Ok(PricedValue::scalar(
                self.memory.lambda_http_stream_gb_price(),
                "USD",
            )),
            "aws.lambda.free_tier_requests" => Ok(PricedValue::scalar(
                self.provider().lambda_price().free_tier_requests,
                "USD",
            )),
            "aws.lambda.free_tier_gb_seconds" => Ok(PricedValue::scalar(
                self.provider().lambda_price().free_tier_gb_seconds,
                "USD",
            )),

            // DynamoDB
            "aws.dynamodb.write_request_price" => Ok(PricedValue::scalar(
                self.provider().dynamodb_price().write_request_price,
                "USD",
            )),
            "aws.dynamodb.read_request_price" => Ok(PricedValue::scalar(
                self.provider().dynamodb_price().read_request_price,
                "USD",
            )),
            "aws.dynamodb.wcu_hour_price" => Ok(PricedValue::scalar(
                self.provider().dynamodb_price().wcu_hour_price,
                "USD",
            )),
            "aws.dynamodb.rcu_hour_price" => Ok(PricedValue::scalar(
                self.provider().dynamodb_price().rcu_hour_price,
                "USD",
            )),
            "aws.dynamodb.storage_price_per_gb" => Ok(PricedValue::scalar(
                self.provider().dynamodb_price().storage_price_per_gb,
                "USD",
            )),
            "aws.dynamodb.free_tier_wru" => Ok(PricedValue::scalar(
                self.provider().dynamodb_price().free_tier_wru,
                "USD",
            )),
            "aws.dynamodb.free_tier_rru" => Ok(PricedValue::scalar(
                self.provider().dynamodb_price().free_tier_rru,
                "USD",
            )),
            "aws.dynamodb.free_tier_storage_gb" => Ok(PricedValue::scalar(
                self.provider().dynamodb_price().free_tier_storage_gb,
                "USD",
            )),

            // Kinesis
            "aws.kinesis.shard_hour_price" => Ok(PricedValue::scalar(
                self.provider().kinesis_price().shard_hour_price,
                "USD",
            )),
            "aws.kinesis.put_payload_unit_price" => Ok(PricedValue::scalar(
                self.provider().kinesis_price().put_payload_unit_price,
                "USD",
            )),
            "aws.kinesis.on_demand_ingestion_price_per_gb" => Ok(PricedValue::scalar(
                self.provider()
                    .kinesis_price()
                    .on_demand_ingestion_price_per_gb,
                "USD",
            )),
            "aws.kinesis.on_demand_retrieval_price_per_gb" => Ok(PricedValue::scalar(
                self.provider()
                    .kinesis_price()
                    .on_demand_retrieval_price_per_gb,
                "USD",
            )),
            "aws.kinesis.on_demand_stream_hour_price" => Ok(PricedValue::scalar(
                self.provider().kinesis_price().on_demand_stream_hour_price,
                "USD",
            )),

            // S3
            "aws.s3.put_request_price" => Ok(PricedValue::scalar(
                self.provider().s3_price().put_request_price,
                "USD",
            )),
            "aws.s3.get_request_price" => Ok(PricedValue::scalar(
                self.provider().s3_price().get_request_price,
                "USD",
            )),
            "aws.s3.storage_tiers" => {
                let price = self.provider().s3_price();
                let tiers = price
                    .storage_tiers
                    .iter()
                    .map(|t| yevice_pricing::catalog::PricedTier {
                        upper_limit: t.upper_limit_gb,
                        unit_price: t.price_per_gb,
                    })
                    .collect();
                Ok(PricedValue::tiered(tiers, "USD"))
            }

            // SQS
            "aws.sqs.standard_request_price" => Ok(PricedValue::scalar(
                self.provider().sqs_price().standard_request_price,
                "USD",
            )),
            "aws.sqs.fifo_request_price" => Ok(PricedValue::scalar(
                self.provider().sqs_price().fifo_request_price,
                "USD",
            )),
            "aws.sqs.free_tier_requests" => Ok(PricedValue::scalar(
                self.provider().sqs_price().free_tier_requests,
                "USD",
            )),

            // Fargate (ECS/Batch)
            "aws.fargate.vcpu_hour_price" => Ok(PricedValue::scalar(
                self.provider().fargate_price().vcpu_hour_price,
                "USD",
            )),
            "aws.fargate.memory_gb_hour_price" => Ok(PricedValue::scalar(
                self.provider().fargate_price().memory_gb_hour_price,
                "USD",
            )),

            // CloudWatch Logs
            "aws.cloudwatch_logs.ingestion_price_per_gb" => Ok(PricedValue::scalar(
                self.provider()
                    .cloudwatch_logs_price()
                    .ingestion_price_per_gb,
                "USD",
            )),
            "aws.cloudwatch_logs.storage_price_per_gb" => Ok(PricedValue::scalar(
                self.provider().cloudwatch_logs_price().storage_price_per_gb,
                "USD",
            )),
            "aws.cloudwatch_logs.free_tier_ingestion_gb" => Ok(PricedValue::scalar(
                self.provider()
                    .cloudwatch_logs_price()
                    .free_tier_ingestion_gb,
                "USD",
            )),
            "aws.cloudwatch_logs.free_tier_storage_gb" => Ok(PricedValue::scalar(
                self.provider().cloudwatch_logs_price().free_tier_storage_gb,
                "USD",
            )),

            // CloudWatch custom metrics (Container Insights)
            "aws.cloudwatch.custom_metric_month_price" => Ok(PricedValue::scalar(
                self.memory.cloudwatch_custom_metric_month_price(),
                "USD",
            )),

            // API Gateway
            "aws.api_gateway.rest_api_request_price" => Ok(PricedValue::scalar(
                self.provider().api_gateway_price().rest_api_request_price,
                "USD",
            )),
            "aws.api_gateway.http_api_request_price" => Ok(PricedValue::scalar(
                self.provider().api_gateway_price().http_api_request_price,
                "USD",
            )),
            "aws.api_gateway.free_tier_requests" => Ok(PricedValue::scalar(
                self.provider().api_gateway_price().free_tier_requests,
                "USD",
            )),

            // NAT Gateway
            "aws.nat_gateway.hourly_price" => Ok(PricedValue::scalar(
                self.provider().nat_gateway_price().hourly_price,
                "USD",
            )),
            "aws.nat_gateway.data_processing_price_per_gb" => Ok(PricedValue::scalar(
                self.provider()
                    .nat_gateway_price()
                    .data_processing_price_per_gb,
                "USD",
            )),

            // CloudFront
            "aws.cloudfront.request_price_per_10k" => Ok(PricedValue::scalar(
                self.provider().cloudfront_price().request_price_per_10k,
                "USD",
            )),
            "aws.cloudfront.data_transfer_price_per_gb" => Ok(PricedValue::scalar(
                self.provider()
                    .cloudfront_price()
                    .data_transfer_price_per_gb,
                "USD",
            )),
            "aws.cloudfront.free_tier_data_transfer_gb" => Ok(PricedValue::scalar(
                self.provider()
                    .cloudfront_price()
                    .free_tier_data_transfer_gb,
                "USD",
            )),

            // Step Functions
            "aws.step_functions.standard_transition_price" => Ok(PricedValue::scalar(
                self.provider()
                    .step_functions_price()
                    .standard_transition_price,
                "USD",
            )),
            "aws.step_functions.express_request_price" => Ok(PricedValue::scalar(
                self.provider().step_functions_price().express_request_price,
                "USD",
            )),
            "aws.step_functions.express_duration_price_per_gb_second" => Ok(PricedValue::scalar(
                self.provider()
                    .step_functions_price()
                    .express_duration_price_per_gb_second,
                "USD",
            )),
            "aws.step_functions.free_tier_transitions" => Ok(PricedValue::scalar(
                self.provider().step_functions_price().free_tier_transitions,
                "USD",
            )),

            // EventBridge Scheduler
            "aws.eventbridge_scheduler.invocation_price" => Ok(PricedValue::scalar(
                self.provider()
                    .eventbridge_scheduler_price()
                    .invocation_price,
                "USD",
            )),
            "aws.eventbridge_scheduler.free_tier_invocations" => Ok(PricedValue::scalar(
                self.provider()
                    .eventbridge_scheduler_price()
                    .free_tier_invocations,
                "USD",
            )),

            // EventBridge Rule
            "aws.eventbridge_rule.custom_event_price_per_million" => Ok(PricedValue::scalar(
                self.memory
                    .eventbridge_price()
                    .custom_event_price_per_million,
                "USD",
            )),

            // Data transfer (egress)
            "aws.data_transfer.egress_tiers" => {
                let price = self.provider().data_transfer_price();
                let tiers = price
                    .egress_tiers
                    .iter()
                    .map(|t| yevice_pricing::catalog::PricedTier {
                        upper_limit: t.upper_limit_gb,
                        unit_price: t.price_per_gb,
                    })
                    .collect();
                Ok(PricedValue::tiered(tiers, "USD"))
            }

            // ALB
            "aws.alb.alb_hour_price" => Ok(PricedValue::scalar(
                self.memory.alb_price().alb_hour_price,
                "USD",
            )),
            "aws.alb.lcu_hour_price" => Ok(PricedValue::scalar(
                self.memory.alb_price().lcu_hour_price,
                "USD",
            )),

            // SNS
            "aws.sns.delivery_price_per_million" => Ok(PricedValue::scalar(
                self.memory.sns_price().delivery_price_per_million,
                "USD",
            )),
            "aws.sns.free_tier_deliveries" => Ok(PricedValue::scalar(
                self.memory.sns_price().free_tier_deliveries,
                "USD",
            )),

            // EKS
            "aws.eks.cluster_hour_price" => Ok(PricedValue::scalar(
                self.memory.eks_price().cluster_hour_price,
                "USD",
            )),

            // Firehose
            "aws.firehose.ingestion_price_per_gb" => Ok(PricedValue::scalar(
                self.memory.firehose_price().ingestion_price_per_gb,
                "USD",
            )),

            // Secrets Manager
            "aws.secrets_manager.secret_month_price" => Ok(PricedValue::scalar(
                self.memory.secrets_manager_price().secret_month_price,
                "USD",
            )),
            "aws.secrets_manager.api_call_price_per_10k" => Ok(PricedValue::scalar(
                self.memory.secrets_manager_price().api_call_price_per_10k,
                "USD",
            )),

            // WAF
            "aws.waf.web_acl_month_price" => Ok(PricedValue::scalar(
                self.memory.waf_price().web_acl_month_price,
                "USD",
            )),
            "aws.waf.rule_month_price" => Ok(PricedValue::scalar(
                self.memory.waf_price().rule_month_price,
                "USD",
            )),
            "aws.waf.request_price_per_million" => Ok(PricedValue::scalar(
                self.memory.waf_price().request_price_per_million,
                "USD",
            )),

            // EFS
            "aws.efs.standard_gb_month_price" => Ok(PricedValue::scalar(
                self.memory.efs_price().standard_gb_month_price,
                "USD",
            )),
            "aws.efs.ia_gb_month_price" => Ok(PricedValue::scalar(
                self.memory.efs_price().ia_gb_month_price,
                "USD",
            )),
            "aws.efs.ia_access_price_per_gb" => Ok(PricedValue::scalar(
                self.memory.efs_price().ia_access_price_per_gb,
                "USD",
            )),

            // Athena
            "aws.athena.scan_price_per_tb" => Ok(PricedValue::scalar(
                self.memory.athena_price().scan_price_per_tb,
                "USD",
            )),

            // Bedrock (foundation-model token pricing)
            "aws.bedrock.input_token_price_per_1k" => Ok(PricedValue::scalar(
                self.memory.bedrock_input_token_price_per_1k(),
                "USD",
            )),
            "aws.bedrock.output_token_price_per_1k" => Ok(PricedValue::scalar(
                self.memory.bedrock_output_token_price_per_1k(),
                "USD",
            )),

            // ECR
            "aws.ecr.private_storage_gb_month" => Ok(PricedValue::scalar(
                self.memory.ecr_price().private_storage_gb_month,
                "USD",
            )),

            // Batch
            "aws.batch.fargate_vcpu_hour_price" => Ok(PricedValue::scalar(
                self.provider().batch_price().fargate_vcpu_hour_price,
                "USD",
            )),
            "aws.batch.fargate_memory_gb_hour_price" => Ok(PricedValue::scalar(
                self.provider().batch_price().fargate_memory_gb_hour_price,
                "USD",
            )),
            "aws.batch.fargate_ephemeral_storage_gb_hour_price" => Ok(PricedValue::scalar(
                self.provider()
                    .batch_price()
                    .fargate_ephemeral_storage_gb_hour_price,
                "USD",
            )),
            "aws.batch.fargate_ephemeral_free_gb" => Ok(PricedValue::scalar(
                self.provider().batch_price().fargate_ephemeral_free_gb,
                "USD",
            )),
            "aws.batch.ebs_gp3_gb_month_price" => Ok(PricedValue::scalar(
                self.provider().batch_price().ebs_gp3_gb_month_price,
                "USD",
            )),
            "aws.batch.ebs_gp3_iops_month_price" => Ok(PricedValue::scalar(
                self.provider().batch_price().ebs_gp3_iops_month_price,
                "USD",
            )),
            "aws.batch.ebs_gp3_iops_free" => Ok(PricedValue::scalar(
                self.provider().batch_price().ebs_gp3_iops_free,
                "USD",
            )),
            "aws.batch.ebs_gp3_throughput_mibps_month_price" => Ok(PricedValue::scalar(
                self.provider()
                    .batch_price()
                    .ebs_gp3_throughput_mibps_month_price,
                "USD",
            )),
            "aws.batch.ebs_gp3_throughput_free_mibps" => Ok(PricedValue::scalar(
                self.provider().batch_price().ebs_gp3_throughput_free_mibps,
                "USD",
            )),

            // AppSync
            "aws.appsync.operation_price_per_million" => Ok(PricedValue::scalar(
                self.memory.appsync_price().operation_price_per_million,
                "USD",
            )),
            "aws.appsync.free_tier_operations" => Ok(PricedValue::scalar(
                self.memory.appsync_price().free_tier_operations,
                "USD",
            )),

            // Cognito
            "aws.cognito.free_tier_mau" => Ok(PricedValue::scalar(
                self.memory.cognito_price().free_tier_mau,
                "USD",
            )),
            "aws.cognito.tier1_price" => Ok(PricedValue::scalar(
                self.memory.cognito_price().tier1_price,
                "USD",
            )),
            "aws.cognito.tier2_price" => Ok(PricedValue::scalar(
                self.memory.cognito_price().tier2_price,
                "USD",
            )),
            "aws.cognito.tier3_price" => Ok(PricedValue::scalar(
                self.memory.cognito_price().tier3_price,
                "USD",
            )),

            // Route53
            "aws.route53.hosted_zone_month_price" => Ok(PricedValue::scalar(
                self.memory.route53_price().hosted_zone_month_price,
                "USD",
            )),
            "aws.route53.query_price_per_million" => Ok(PricedValue::scalar(
                self.memory.route53_price().query_price_per_million,
                "USD",
            )),

            // OpenSearch Serverless
            "aws.opensearch_serverless.ocu_hour_price" => Ok(PricedValue::scalar(
                self.provider().opensearch_serverless_price().ocu_hour_price,
                "USD",
            )),
            "aws.opensearch_serverless.storage_price_per_gb" => Ok(PricedValue::scalar(
                self.provider()
                    .opensearch_serverless_price()
                    .storage_price_per_gb,
                "USD",
            )),

            // Glue
            "aws.glue.standard_dpu_hour_price" => Ok(PricedValue::scalar(
                self.memory.glue_price().standard_dpu_hour_price,
                "USD",
            )),
            "aws.glue.flex_dpu_hour_price" => Ok(PricedValue::scalar(
                self.memory.glue_price().flex_dpu_hour_price,
                "USD",
            )),

            // Instance-type-specific SKUs (passed dynamically)
            // Windows arm must precede the generic Linux instance arm.
            sku if sku.starts_with("aws.ec2.os.windows.") => {
                let itype = sku.strip_prefix("aws.ec2.os.windows.").unwrap_or("");
                Ok(PricedValue::scalar(
                    self.memory.ec2_windows_hourly_price(itype)?,
                    "USD",
                ))
            }
            sku if sku.starts_with("aws.ec2.instance.") => {
                let itype = sku.strip_prefix("aws.ec2.instance.").unwrap_or("");
                Ok(PricedValue::scalar(
                    self.provider().ec2_price(itype)?.hourly_price,
                    "USD",
                ))
            }
            // RDS gp3 storage and excess-IOPS unit prices must be matched
            // before the generic `aws.rds.*` prefix guard below, as Rust
            // evaluates match arms in order and the prefix guard would shadow
            // these exact-string arms.
            // Route through the file registry so that non-Tokyo regions emit a
            // fallback warning consistent with other RDS paths (via
            // FilePricingRegistry::warn_fallback_once).
            "aws.rds.gp3_storage_gb_month" => {
                Ok(PricedValue::scalar(self.rds_gp3_storage_price(), "USD"))
            }
            "aws.rds.gp3_iops_month" => Ok(PricedValue::scalar(self.rds_gp3_iops_price(), "USD")),
            sku if sku.starts_with("aws.rds.") => {
                // Format: aws.rds.<engine>.<instance_type>
                let rest = sku.strip_prefix("aws.rds.").unwrap_or("");
                let mut parts = rest.splitn(2, '.');
                let engine = parts.next().unwrap_or("mysql");
                let itype = parts.next().unwrap_or("db.t3.micro");
                let price = self.provider().rds_price(itype, engine)?;
                Ok(PricedValue::scalar(price.hourly_price, "USD"))
            }
            sku if sku.starts_with("aws.rds_storage.") => {
                let rest = sku.strip_prefix("aws.rds_storage.").unwrap_or("");
                let mut parts = rest.splitn(2, '.');
                let engine = parts.next().unwrap_or("mysql");
                let itype = parts.next().unwrap_or("db.t3.micro");
                let price = self.provider().rds_price(itype, engine)?;
                Ok(PricedValue::scalar(price.storage_price_per_gb, "USD"))
            }
            sku if sku.starts_with("aws.elasticache.") => {
                let node_type = sku.strip_prefix("aws.elasticache.").unwrap_or("");
                Ok(PricedValue::scalar(
                    self.provider().elasticache_price(node_type)?.hourly_price,
                    "USD",
                ))
            }
            sku if sku.starts_with("aws.msk.") => {
                let itype = sku.strip_prefix("aws.msk.").unwrap_or("");
                Ok(PricedValue::scalar(
                    self.memory.msk_broker_price(itype)?.hourly_price,
                    "USD",
                ))
            }
            sku if sku.starts_with("aws.msk_storage.") => {
                let itype = sku.strip_prefix("aws.msk_storage.").unwrap_or("");
                Ok(PricedValue::scalar(
                    self.memory.msk_broker_price(itype)?.storage_gb_month_price,
                    "USD",
                ))
            }
            sku if sku.starts_with("aws.opensearch_service.") => {
                let itype = sku.strip_prefix("aws.opensearch_service.").unwrap_or("");
                Ok(PricedValue::scalar(
                    self.memory
                        .opensearch_service_price(itype)?
                        .instance_hour_price,
                    "USD",
                ))
            }
            sku if sku.starts_with("aws.opensearch_service_storage.") => {
                let itype = sku
                    .strip_prefix("aws.opensearch_service_storage.")
                    .unwrap_or("");
                Ok(PricedValue::scalar(
                    self.memory
                        .opensearch_service_price(itype)?
                        .gp2_storage_gb_month_price,
                    "USD",
                ))
            }
            sku if sku.starts_with("aws.documentdb.") => {
                let itype = sku.strip_prefix("aws.documentdb.").unwrap_or("");
                Ok(PricedValue::scalar(
                    self.memory.documentdb_price(itype)?.instance_hour_price,
                    "USD",
                ))
            }
            sku if sku.starts_with("aws.documentdb_storage.") => Ok(PricedValue::scalar(
                self.memory.documentdb_storage_price(),
                "USD",
            )),
            // EBS (standalone volumes + snapshots) and Site-to-Site VPN
            "aws.ebs.snapshot_gb_month" => Ok(PricedValue::scalar(
                self.memory.ebs_snapshot_gb_month_price(),
                "USD",
            )),
            sku if sku.starts_with("aws.ebs.gb_month.") => {
                let vtype = sku.strip_prefix("aws.ebs.gb_month.").unwrap_or("");
                Ok(PricedValue::scalar(
                    self.memory.ebs_gb_month_price(vtype)?,
                    "USD",
                ))
            }
            "aws.vpn.connection_hour" => Ok(PricedValue::scalar(
                self.memory.site_to_site_vpn_connection_hour_price(),
                "USD",
            )),

            // Redshift managed storage + Spectrum (exact arms must precede the
            // generic `aws.redshift.<node_type>` prefix match below).
            "aws.redshift.storage_gb_month" => Ok(PricedValue::scalar(
                self.memory.redshift_storage_gb_month_price(),
                "USD",
            )),
            "aws.redshift.spectrum_tb_scan" => Ok(PricedValue::scalar(
                self.memory.redshift_spectrum_tb_scan_price(),
                "USD",
            )),
            sku if sku.starts_with("aws.redshift.") => {
                let node_type = sku.strip_prefix("aws.redshift.").unwrap_or("");
                Ok(PricedValue::scalar(
                    self.memory.redshift_price(node_type)?.node_hour_price,
                    "USD",
                ))
            }

            // Lightsail
            sku if sku.starts_with("aws.lightsail.bundle.") => {
                let bundle = sku.strip_prefix("aws.lightsail.bundle.").unwrap_or("");
                Ok(PricedValue::scalar(
                    self.memory.lightsail_bundle_month_price(bundle)?,
                    "USD",
                ))
            }
            "aws.lightsail.bundle_month_price" => Ok(PricedValue::scalar(
                self.memory.lightsail_price().instance_bundle_month_price,
                "USD",
            )),
            "aws.lightsail.disk_gb_month_price" => Ok(PricedValue::scalar(
                self.memory.lightsail_price().disk_gb_month_price,
                "USD",
            )),

            // QuickSight
            "aws.quicksight.creator_month_price" => Ok(PricedValue::scalar(
                self.memory.quicksight_price().creator_month_price,
                "USD",
            )),
            "aws.quicksight.viewer_session_price" => Ok(PricedValue::scalar(
                self.memory.quicksight_price().viewer_session_price,
                "USD",
            )),
            "aws.quicksight.viewer_max_month_price" => Ok(PricedValue::scalar(
                self.memory.quicksight_price().viewer_max_month_price,
                "USD",
            )),
            "aws.quicksight.spice_gb_month_price" => Ok(PricedValue::scalar(
                self.memory.quicksight_price().spice_gb_month_price,
                "USD",
            )),
            "aws.quicksight.free_spice_gb" => Ok(PricedValue::scalar(
                self.memory.quicksight_price().free_spice_gb,
                "USD",
            )),

            // ----- aws.kendra -----
            // Kendra index (per-edition hourly rate)
            sku if sku.starts_with("aws.kendra.index_hour.") => {
                let edition = sku.strip_prefix("aws.kendra.index_hour.").unwrap_or("");
                Ok(PricedValue::scalar(
                    self.memory.kendra_index_hour_price(edition)?,
                    "USD",
                ))
            }
            "aws.kendra.connector_scan_document_price" => Ok(PricedValue::scalar(
                self.memory.kendra_connector_scan_document_price(),
                "USD",
            )),
            "aws.kendra.connector_scan_hour_price" => Ok(PricedValue::scalar(
                self.memory.kendra_connector_scan_hour_price(),
                "USD",
            )),

            // ----- aws.transcribe -----
            // Transcribe
            "aws.transcribe.standard_batch_price_per_minute" => Ok(PricedValue::scalar(
                self.memory.transcribe_standard_batch_price_per_minute(),
                "USD",
            )),

            // ----- aws.fsx_windows -----
            // FSx for Windows File Server
            "aws.fsx_windows.backup_gb_month" => Ok(PricedValue::scalar(
                self.memory.fsx_windows_backup_gb_month_price(),
                "USD",
            )),
            sku if sku.starts_with("aws.fsx_windows.storage_gb_month.") => {
                // Format: aws.fsx_windows.storage_gb_month.<storage_type>.<deployment>
                let rest = sku
                    .strip_prefix("aws.fsx_windows.storage_gb_month.")
                    .unwrap_or("");
                let mut parts = rest.splitn(2, '.');
                let storage_type = parts.next().unwrap_or("ssd");
                let deployment = parts.next().unwrap_or("single_az");
                Ok(PricedValue::scalar(
                    self.memory
                        .fsx_windows_storage_gb_month_price(storage_type, deployment)?,
                    "USD",
                ))
            }
            sku if sku.starts_with("aws.fsx_windows.throughput_mbps_month.") => {
                let deployment = sku
                    .strip_prefix("aws.fsx_windows.throughput_mbps_month.")
                    .unwrap_or("single_az");
                Ok(PricedValue::scalar(
                    self.memory
                        .fsx_windows_throughput_mbps_month_price(deployment)?,
                    "USD",
                ))
            }

            // ----- aws.directory_service -----
            // AWS Directory Service — Managed Microsoft AD (per domain-controller-hour, by edition)
            sku if sku.starts_with("aws.directory_service.dc_hour.") => {
                let edition = sku
                    .strip_prefix("aws.directory_service.dc_hour.")
                    .unwrap_or("");
                Ok(PricedValue::scalar(
                    self.memory.directory_service_dc_hour_price(edition)?,
                    "USD",
                ))
            }

            // ----- aws.cloudwatch -----
            // CloudWatch standard alarms
            "aws.cloudwatch.alarm_month_price" => Ok(PricedValue::scalar(
                self.memory.cloudwatch_alarm_month_price(),
                "USD",
            )),

            // ----- aws.guardduty -----
            // GuardDuty
            "aws.guardduty.cloudtrail_event_price" => Ok(PricedValue::scalar(
                self.memory.guardduty_price().cloudtrail_event_price,
                "USD",
            )),
            "aws.guardduty.flowlog_dns_gb_tiers" => {
                let price = self.memory.guardduty_price();
                let tiers = price
                    .flowlog_dns_gb_tiers
                    .iter()
                    .map(|t| yevice_pricing::catalog::PricedTier {
                        upper_limit: t.upper_limit_gb,
                        unit_price: t.price_per_gb,
                    })
                    .collect();
                Ok(PricedValue::tiered(tiers, "USD"))
            }

            // ----- aws.cloudtrail -----
            // CloudTrail
            "aws.cloudtrail.data_event_price_per_100k" => Ok(PricedValue::scalar(
                self.memory.cloudtrail_data_event_price_per_100k(),
                "USD",
            )),
            "aws.cloudtrail.management_event_copy_price_per_100k" => Ok(PricedValue::scalar(
                self.memory
                    .cloudtrail_management_event_copy_price_per_100k(),
                "USD",
            )),

            // ----- aws.backup -----
            // AWS Backup (warm / backup storage, per protected-resource engine)
            sku if sku.starts_with("aws.backup.warm_storage_gb_month.") => {
                let engine = sku
                    .strip_prefix("aws.backup.warm_storage_gb_month.")
                    .unwrap_or("");
                Ok(PricedValue::scalar(
                    self.memory.backup_warm_storage_gb_month_price(engine)?,
                    "USD",
                ))
            }
            "aws.data_transfer.inter_region_price_per_gb" => Ok(PricedValue::scalar(
                self.memory.data_transfer_inter_region_price_per_gb(),
                "USD",
            )),

            _ => Err(PricingError::NotFound {
                service: sku.to_string(),
                region: self.memory.region.clone(),
            }),
        };

        // List-price mode also drops the leading free (unit_price == 0) tiers of
        // tiered records, e.g. the internet data-transfer "first 1 GB free"
        // allowance, which is encoded in the tier structure rather than a
        // `free_tier_*` SKU. Non-free leading tiers (e.g. S3 storage) are kept.
        match (self.list_price, record?) {
            (true, PricedValue::Tiered { tiers, currency }) => {
                let stripped: Vec<_> = tiers
                    .into_iter()
                    .skip_while(|t| t.unit_price == 0.0)
                    .collect();
                Ok(PricedValue::tiered(stripped, currency))
            }
            (_, record) => Ok(record),
        }
    }
}

impl PriceCatalogResolver for AwsPricingCatalog {
    fn resolve(&self, provider: Provider) -> Option<&dyn PriceCatalog> {
        (provider == Provider::Aws).then_some(self as &dyn PriceCatalog)
    }
}

// -----------------------------------------------------------------------------
// TypedPricingProvider<USD> — ADR-0001 Tier B trait.
//
// AwsPricingCatalog is statically USD-tagged. Bulk-API files that declare a
// non-USD currency are rejected at the SKU boundary with
// `PricingError::CurrencyMismatch`. The `PriceCatalog::lookup` impl above
// already returns USD-tagged `PricedValue` values; this trait surface simply
// re-exposes the typed (`Currency<f64, USD>`) form for callers that want
// compile-time currency safety inside this crate.
// -----------------------------------------------------------------------------

use yevice_core::currency::{Currency, USD};
use yevice_pricing::catalog::{TypedPriceRecord, TypedPricingProvider, TypedTier};

impl TypedPricingProvider<USD> for AwsPricingCatalog {
    fn lookup(&self, sku: &Sku) -> Result<TypedPriceRecord<USD>, PricingError> {
        // Validate Bulk-API declared currency once at the typed boundary
        // (currently a no-op for hardcoded SKUs, but in place for when more
        // SKUs are routed via FilePricingRegistry metadata).
        if let Some(file) = self.file.as_ref() {
            for meta in file.all_metadata() {
                if meta.currency != <USD as yevice_core::CurrencyCode>::CODE {
                    return Err(PricingError::CurrencyMismatch {
                        expected: <USD as yevice_core::CurrencyCode>::CODE.to_string(),
                        actual: meta.currency.clone(),
                        sku: sku.clone(),
                    });
                }
            }
        }

        // Delegate to the erased path and re-promote into the typed enum.
        match <Self as PriceCatalog>::lookup(self, sku)? {
            PricedValue::Scalar { value, .. } => {
                Ok(TypedPriceRecord::Scalar(Currency::<f64, USD>::new(value)))
            }
            PricedValue::Tiered { tiers, .. } => Ok(TypedPriceRecord::Tiered(
                tiers
                    .into_iter()
                    .map(|t| TypedTier::new(t.upper_limit, Currency::<f64, USD>::new(t.unit_price)))
                    .collect(),
            )),
        }
    }
}
