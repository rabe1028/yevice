use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Pricing data for a single AWS service in a specific region.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServicePricing {
    pub service: String,
    pub region: String,
    pub products: Vec<Product>,
}

/// A single product (e.g., a specific EC2 instance type).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Product {
    pub sku: String,
    pub attributes: HashMap<String, String>,
    pub prices: Vec<PriceEntry>,
}

/// A price entry for a product.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceEntry {
    pub unit: String,
    pub price_per_unit: f64,
    pub description: String,
    /// For tiered pricing, the begin/end range.
    pub begin_range: Option<f64>,
    pub end_range: Option<f64>,
}

/// EC2 instance pricing lookup result.
#[derive(Debug, Clone)]
pub struct Ec2Price {
    pub instance_type: String,
    pub hourly_price: f64,
}

/// Lambda pricing lookup result.
#[derive(Debug, Clone)]
pub struct LambdaPrice {
    pub request_price: f64,
    pub gb_second_price: f64,
    pub free_tier_requests: f64,
    pub free_tier_gb_seconds: f64,
}

/// RDS instance pricing lookup result.
#[derive(Debug, Clone)]
pub struct RdsPrice {
    pub instance_type: String,
    pub hourly_price: f64,
    pub storage_price_per_gb: f64,
}

/// S3 storage pricing tiers.
#[derive(Debug, Clone)]
pub struct S3Price {
    pub storage_tiers: Vec<S3StorageTier>,
    pub put_request_price: f64,
    pub get_request_price: f64,
}

#[derive(Debug, Clone)]
pub struct S3StorageTier {
    pub upper_limit_gb: Option<f64>,
    pub price_per_gb: f64,
}

/// `DynamoDB` pricing.
#[derive(Debug, Clone)]
pub struct DynamoDbPrice {
    // On-Demand (PAY_PER_REQUEST)
    pub write_request_price: f64,
    pub read_request_price: f64,
    // Provisioned
    pub wcu_hour_price: f64,
    pub rcu_hour_price: f64,
    // Common
    pub storage_price_per_gb: f64,
    pub free_tier_wru: f64,
    pub free_tier_rru: f64,
    pub free_tier_storage_gb: f64,
}

/// ECS Fargate pricing.
#[derive(Debug, Clone)]
pub struct FargatePrice {
    pub vcpu_hour_price: f64,
    pub memory_gb_hour_price: f64,
}

/// `OpenSearch` Serverless pricing.
#[derive(Debug, Clone)]
pub struct OpenSearchServerlessPrice {
    pub ocu_hour_price: f64,
    pub storage_price_per_gb: f64,
}

/// Kinesis Data Streams pricing.
#[derive(Debug, Clone)]
pub struct KinesisPrice {
    // Provisioned mode
    pub shard_hour_price: f64,
    pub put_payload_unit_price: f64,
    // On-Demand mode
    pub on_demand_ingestion_price_per_gb: f64,
    pub on_demand_retrieval_price_per_gb: f64,
    pub on_demand_stream_hour_price: f64,
}

/// SQS pricing.
#[derive(Debug, Clone)]
pub struct SqsPrice {
    pub standard_request_price: f64,
    pub fifo_request_price: f64,
    pub free_tier_requests: f64,
}

/// `CloudWatch` Logs pricing.
#[derive(Debug, Clone)]
pub struct CloudWatchLogsPrice {
    pub ingestion_price_per_gb: f64,
    pub storage_price_per_gb: f64,
    pub free_tier_ingestion_gb: f64,
    pub free_tier_storage_gb: f64,
}

/// API Gateway (REST/HTTP) pricing.
#[derive(Debug, Clone)]
pub struct ApiGatewayPrice {
    pub rest_api_request_price: f64,
    pub http_api_request_price: f64,
    pub free_tier_requests: f64,
}

/// NAT Gateway pricing.
#[derive(Debug, Clone)]
pub struct NatGatewayPrice {
    pub hourly_price: f64,
    pub data_processing_price_per_gb: f64,
}

/// `CloudFront` pricing.
#[derive(Debug, Clone)]
pub struct CloudFrontPrice {
    pub request_price_per_10k: f64,
    pub data_transfer_price_per_gb: f64,
    pub free_tier_data_transfer_gb: f64,
}

/// `ElastiCache` pricing.
#[derive(Debug, Clone)]
pub struct ElastiCachePrice {
    pub node_type: String,
    pub hourly_price: f64,
}

/// Step Functions pricing.
#[derive(Debug, Clone)]
pub struct StepFunctionsPrice {
    pub standard_transition_price: f64,
    pub express_request_price: f64,
    pub express_duration_price_per_gb_second: f64,
    pub free_tier_transitions: f64,
}

/// `EventBridge` Scheduler pricing.
#[derive(Debug, Clone)]
pub struct EventBridgeSchedulerPrice {
    pub invocation_price: f64,
    pub free_tier_invocations: f64,
}

/// Internet egress (data transfer out) pricing with tiered structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataTransferPrice {
    /// Tiered pricing for internet egress (GB)
    pub egress_tiers: Vec<DataTransferTier>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataTransferTier {
    pub upper_limit_gb: Option<f64>,
    pub price_per_gb: f64,
}

/// AWS Batch pricing (uses Fargate or EC2 pricing underneath).
#[derive(Debug, Clone)]
pub struct BatchPrice {
    // Fargate pricing (same as ECS Fargate)
    pub fargate_vcpu_hour_price: f64,
    pub fargate_memory_gb_hour_price: f64,
    pub fargate_ephemeral_storage_gb_hour_price: f64,
    pub fargate_ephemeral_free_gb: f64,
    // EBS gp3 pricing
    pub ebs_gp3_gb_month_price: f64,
    pub ebs_gp3_iops_month_price: f64,
    pub ebs_gp3_iops_free: f64,
    pub ebs_gp3_throughput_mibps_month_price: f64,
    pub ebs_gp3_throughput_free_mibps: f64,
}

#[derive(Debug, Clone)]
pub struct AlbPrice {
    pub alb_hour_price: f64,
    pub lcu_hour_price: f64,
}

#[derive(Debug, Clone)]
pub struct SnsPrice {
    pub delivery_price_per_million: f64,
    pub free_tier_deliveries: f64,
}

#[derive(Debug, Clone)]
pub struct EksPrice {
    pub cluster_hour_price: f64,
}

#[derive(Debug, Clone)]
pub struct FirehosePrice {
    pub ingestion_price_per_gb: f64,
}

#[derive(Debug, Clone)]
pub struct SecretsManagerPrice {
    pub secret_month_price: f64,
    pub api_call_price_per_10k: f64,
}

#[derive(Debug, Clone)]
pub struct WafPrice {
    pub web_acl_month_price: f64,
    pub rule_month_price: f64,
    pub request_price_per_million: f64,
}

#[derive(Debug, Clone)]
pub struct EfsPrice {
    pub standard_gb_month_price: f64,
    pub ia_gb_month_price: f64,
    pub ia_access_price_per_gb: f64,
}

#[derive(Debug, Clone)]
pub struct EventBridgePrice {
    pub custom_event_price_per_million: f64,
}

#[derive(Debug, Clone)]
pub struct AthenaPrice {
    pub scan_price_per_tb: f64,
}

#[derive(Debug, Clone)]
pub struct EcrPrice {
    pub private_storage_gb_month: f64,
}

#[derive(Debug, Clone)]
pub struct AppSyncPrice {
    pub operation_price_per_million: f64,
    pub free_tier_operations: f64,
}

#[derive(Debug, Clone)]
pub struct CognitoPrice {
    pub free_tier_mau: f64,
    pub tier1_price: f64,
    pub tier2_price: f64,
    pub tier3_price: f64,
}

#[derive(Debug, Clone)]
pub struct Route53Price {
    pub hosted_zone_month_price: f64,
    pub query_price_per_million: f64,
}

#[derive(Debug, Clone)]
pub struct GluePrice {
    pub standard_dpu_hour_price: f64,
    pub flex_dpu_hour_price: f64,
}

#[derive(Debug, Clone)]
pub struct MskBrokerPrice {
    pub hourly_price: f64,
    pub storage_gb_month_price: f64,
}

#[derive(Debug, Clone)]
pub struct OpenSearchServicePrice {
    pub instance_hour_price: f64,
    pub gp2_storage_gb_month_price: f64,
}

#[derive(Debug, Clone)]
pub struct DocumentDbPrice {
    pub instance_hour_price: f64,
    pub storage_gb_month_price: f64,
}

#[derive(Debug, Clone)]
pub struct RedshiftPrice {
    pub node_hour_price: f64,
}

/// `Lightsail` pricing.
pub struct LightsailPrice {
    /// Instance bundle price per month (e.g., $3.43 for nano_2_0)
    pub instance_bundle_month_price: f64,
    /// EBS disk price per GB per month
    pub disk_gb_month_price: f64,
}

/// `QuickSight` pricing.
pub struct QuickSightPrice {
    /// Creator monthly price per user
    pub creator_month_price: f64,
    /// Creator annual price per user (discounted)
    pub creator_annual_month_price: f64,
    /// Viewer on-demand session price per session
    pub viewer_session_price: f64,
    /// Viewer maximum monthly price per user
    pub viewer_max_month_price: f64,
    /// SPICE capacity price per GB per month
    pub spice_gb_month_price: f64,
    /// Free SPICE allocation per creator
    pub free_spice_gb: f64,
}

/// Amazon GuardDuty pricing.
#[derive(Debug, Clone)]
pub struct GuardDutyPrice {
    /// CloudTrail management-event analysis price per individual event.
    pub cloudtrail_event_price: f64,
    /// VPC Flow Logs + DNS query log analysis, volume-tiered per GB-month.
    pub flowlog_dns_gb_tiers: Vec<GuardDutyTier>,
}

#[derive(Debug, Clone)]
pub struct GuardDutyTier {
    pub upper_limit_gb: Option<f64>,
    pub price_per_gb: f64,
}
