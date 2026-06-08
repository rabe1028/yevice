//! Trait abstracting pricing data access.

use crate::error::PricingError;
use crate::model::*;

/// Provides pricing data for AWS resources.
pub trait PricingProvider {
    fn region(&self) -> &str;
    fn lambda_price(&self) -> LambdaPrice;
    fn ec2_price(&self, instance_type: &str) -> Result<Ec2Price, PricingError>;
    fn rds_price(&self, instance_type: &str, engine: &str) -> Result<RdsPrice, PricingError>;
    fn s3_price(&self) -> S3Price;
    fn dynamodb_price(&self) -> DynamoDbPrice;
    fn fargate_price(&self) -> FargatePrice;
    fn opensearch_serverless_price(&self) -> OpenSearchServerlessPrice;
    fn kinesis_price(&self) -> KinesisPrice;
    fn sqs_price(&self) -> SqsPrice;
    fn cloudwatch_logs_price(&self) -> CloudWatchLogsPrice;
    fn api_gateway_price(&self) -> ApiGatewayPrice;
    fn nat_gateway_price(&self) -> NatGatewayPrice;
    fn cloudfront_price(&self) -> CloudFrontPrice;
    fn elasticache_price(&self, node_type: &str) -> Result<ElastiCachePrice, PricingError>;
    fn step_functions_price(&self) -> StepFunctionsPrice;
    fn eventbridge_scheduler_price(&self) -> EventBridgeSchedulerPrice;
    fn batch_price(&self) -> BatchPrice;
    fn data_transfer_price(&self) -> DataTransferPrice;
}

macro_rules! impl_pricing_provider {
    ($ty:ty) => {
        impl PricingProvider for $ty {
            fn region(&self) -> &str {
                &self.region
            }
            fn lambda_price(&self) -> LambdaPrice {
                self.lambda_price()
            }
            fn ec2_price(&self, it: &str) -> Result<Ec2Price, PricingError> {
                self.ec2_price(it)
            }
            fn rds_price(&self, it: &str, e: &str) -> Result<RdsPrice, PricingError> {
                self.rds_price(it, e)
            }
            fn s3_price(&self) -> S3Price {
                self.s3_price()
            }
            fn dynamodb_price(&self) -> DynamoDbPrice {
                self.dynamodb_price()
            }
            fn fargate_price(&self) -> FargatePrice {
                self.fargate_price()
            }
            fn opensearch_serverless_price(&self) -> OpenSearchServerlessPrice {
                self.opensearch_serverless_price()
            }
            fn kinesis_price(&self) -> KinesisPrice {
                self.kinesis_price()
            }
            fn sqs_price(&self) -> SqsPrice {
                self.sqs_price()
            }
            fn cloudwatch_logs_price(&self) -> CloudWatchLogsPrice {
                self.cloudwatch_logs_price()
            }
            fn api_gateway_price(&self) -> ApiGatewayPrice {
                self.api_gateway_price()
            }
            fn nat_gateway_price(&self) -> NatGatewayPrice {
                self.nat_gateway_price()
            }
            fn cloudfront_price(&self) -> CloudFrontPrice {
                self.cloudfront_price()
            }
            fn elasticache_price(&self, nt: &str) -> Result<ElastiCachePrice, PricingError> {
                self.elasticache_price(nt)
            }
            fn step_functions_price(&self) -> StepFunctionsPrice {
                self.step_functions_price()
            }
            fn eventbridge_scheduler_price(&self) -> EventBridgeSchedulerPrice {
                self.eventbridge_scheduler_price()
            }
            fn batch_price(&self) -> BatchPrice {
                self.batch_price()
            }
            fn data_transfer_price(&self) -> DataTransferPrice {
                self.data_transfer_price()
            }
        }
    };
}

impl_pricing_provider!(crate::registry::PricingRegistry);
impl_pricing_provider!(crate::file_registry::FilePricingRegistry);
