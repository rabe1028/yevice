//! AWS service plugin implementations for yevice.

pub mod cfn;
pub mod connection_rules;
pub mod plugin;
pub mod pricing_adapter;
pub mod quotas;
pub mod services;
pub mod tf;

pub use connection_rules::{
    aws_connection_rules, AwsDataFlowRule, AwsEventSourceRule, AwsInvocationRule,
    AwsNotificationRule,
};
pub use plugin::AwsPlugin;
pub use pricing_adapter::AwsPricingCatalog;
pub use quotas::AwsQuotaProvider;

/// Register all AWS services, CFN adapters, TF adapters, and connection rules.
pub fn register(
    catalog: &mut yevice_service_api::ServiceCatalog,
    cfn: &mut yevice_service_api::CfnAdapterRegistry,
    tf: &mut yevice_service_api::TfAdapterRegistry,
) {
    // Connection rules (binding derivation)
    catalog.register_connection_rules(aws_connection_rules());

    // Quota provider
    catalog.register_quota_provider(Box::new(AwsQuotaProvider));

    // Services
    catalog.register(services::lambda::LambdaService);
    catalog.register(services::dynamodb::DynamoDbService);
    catalog.register(services::kinesis::KinesisService);
    catalog.register(services::s3::S3Service);
    catalog.register(services::sqs::SqsService);
    catalog.register(services::ec2::Ec2Service);
    catalog.register(services::ecs_fargate::EcsFargateService);
    catalog.register(services::ecs_ec2::EcsEc2Service);
    catalog.register(services::ecr::EcrService);
    catalog.register(services::rds::RdsService);
    catalog.register(services::opensearch_serverless::OpenSearchServerlessService);
    catalog.register(services::cloudwatch_logs::CloudWatchLogsService);
    catalog.register(services::api_gateway::ApiGatewayService);
    catalog.register(services::nat_gateway::NatGatewayService);
    catalog.register(services::cloudfront::CloudFrontService);
    catalog.register(services::elasticache::ElastiCacheService);
    catalog.register(services::step_functions::StepFunctionsService);
    catalog.register(services::eventbridge_scheduler::EventBridgeSchedulerService);
    catalog.register(services::batch::BatchService);
    catalog.register(services::bedrock::BedrockService);
    catalog.register(services::alb::AlbService);
    catalog.register(services::sns::SnsService);
    catalog.register(services::msk::MskService);
    catalog.register(services::eks::EksService);
    catalog.register(services::firehose::FirehoseService);
    catalog.register(services::secrets_manager::SecretsManagerService);
    catalog.register(services::waf::WafService);
    catalog.register(services::efs::EfsService);
    catalog.register(services::eventbridge_rule::EventBridgeRuleService);
    catalog.register(services::athena::AthenaService);
    catalog.register(services::opensearch_service::OpenSearchServiceService);
    catalog.register(services::documentdb::DocumentDbService);
    catalog.register(services::glue::GlueService);
    catalog.register(services::appsync::AppSyncService);
    catalog.register(services::cognito::CognitoService);
    catalog.register(services::redshift::RedshiftService);
    catalog.register(services::route53::Route53Service);
    catalog.register(services::quicksight::QuickSightService);
    catalog.register(services::lightsail::LightsailService);
    catalog.register(services::container_insights::ContainerInsightsService);
    catalog.register(services::ebs::EbsService);
    catalog.register(services::vpn::VpnService);
    catalog.register(services::kendra::KendraService);
    catalog.register(services::transcribe::TranscribeService);
    catalog.register(services::fsx_windows::FsxWindowsService);
    catalog.register(services::directory_service::DirectoryServiceService);
    catalog.register(services::cloudwatch::CloudWatchService);
    catalog.register(services::guardduty::GuardDutyService);
    catalog.register(services::cloudtrail::CloudTrailService);
    catalog.register(services::backup::BackupService);
    catalog.register(services::data_transfer::DataTransferService);

    // CFN adapters
    cfn.register(cfn::lambda::LambdaCfnAdapter);
    cfn.register(cfn::dynamodb::DynamoDbCfnAdapter);
    cfn.register(cfn::kinesis::KinesisCfnAdapter);
    cfn.register(cfn::s3::S3CfnAdapter);
    cfn.register(cfn::sqs::SqsCfnAdapter);
    cfn.register(cfn::ec2::Ec2CfnAdapter);
    cfn.register(cfn::ecs::EcsCfnAdapter);
    cfn.register(cfn::ecr::EcrCfnAdapter);
    cfn.register(cfn::rds::RdsCfnAdapter);
    cfn.register(cfn::opensearch_serverless::OpenSearchServerlessCfnAdapter);
    cfn.register(cfn::cloudwatch_logs::CloudWatchLogsCfnAdapter);
    cfn.register(cfn::api_gateway::ApiGatewayCfnAdapter);
    cfn.register(cfn::nat_gateway::NatGatewayCfnAdapter);
    cfn.register(cfn::cloudfront::CloudFrontCfnAdapter);
    cfn.register(cfn::elasticache::ElastiCacheCfnAdapter);
    cfn.register(cfn::step_functions::StepFunctionsCfnAdapter);
    cfn.register(cfn::eventbridge_scheduler::EventBridgeSchedulerCfnAdapter);
    cfn.register(cfn::batch::BatchCfnAdapter);
    cfn.register(cfn::bedrock::BedrockCfnAdapter);
    cfn.register(cfn::alb::AlbCfnAdapter);
    cfn.register(cfn::sns::SnsCfnAdapter);
    cfn.register(cfn::msk::MskCfnAdapter);
    cfn.register(cfn::eks::EksCfnAdapter);
    cfn.register(cfn::firehose::FirehoseCfnAdapter);
    cfn.register(cfn::secrets_manager::SecretsManagerCfnAdapter);
    cfn.register(cfn::waf::WafCfnAdapter);
    cfn.register(cfn::efs::EfsCfnAdapter);
    cfn.register(cfn::eventbridge_rule::EventBridgeRuleCfnAdapter);
    cfn.register(cfn::athena::AthenaCfnAdapter);
    cfn.register(cfn::opensearch_service::OpenSearchServiceCfnAdapter);
    cfn.register(cfn::documentdb::DocumentDbCfnAdapter);
    cfn.register(cfn::glue::GlueCfnAdapter);
    cfn.register(cfn::appsync::AppSyncCfnAdapter);
    cfn.register(cfn::cognito::CognitoCfnAdapter);
    cfn.register(cfn::redshift::RedshiftCfnAdapter);
    cfn.register(cfn::route53::Route53CfnAdapter);
    cfn.register(cfn::quicksight::QuickSightCfnAdapter);
    cfn.register(cfn::lightsail::LightsailCfnAdapter);
    cfn.register(cfn::container_insights::EcsClusterCfnAdapter);
    cfn.register(cfn::ebs::EbsCfnAdapter);
    cfn.register(cfn::vpn::VpnCfnAdapter);
    cfn.register(cfn::kendra::KendraCfnAdapter);
    cfn.register(cfn::transcribe::TranscribeCfnAdapter);
    cfn.register(cfn::fsx_windows::FsxWindowsCfnAdapter);
    cfn.register(cfn::directory_service::DirectoryServiceCfnAdapter);
    cfn.register(cfn::cloudwatch::CloudWatchCfnAdapter);
    cfn.register(cfn::guardduty::GuardDutyCfnAdapter);
    cfn.register(cfn::cloudtrail::CloudTrailCfnAdapter);
    cfn.register(cfn::backup::BackupCfnAdapter);
    cfn.register(cfn::data_transfer::DataTransferCfnAdapter);

    // TF adapters (subset — adapters with no corresponding tf/ file are omitted)
    tf.register(tf::lambda::LambdaTfAdapter);
    tf.register(tf::dynamodb::DynamoDbTfAdapter);
    tf.register(tf::kinesis::KinesisTfAdapter);
    tf.register(tf::s3::S3TfAdapter);
    tf.register(tf::sqs::SqsTfAdapter);
    tf.register(tf::ec2::Ec2TfAdapter);
    tf.register(tf::ecs::EcsTfAdapter);
    tf.register(tf::ecr::EcrTfAdapter);
    tf.register(tf::rds::RdsTfAdapter);
    tf.register(tf::opensearch_serverless::OpenSearchServerlessTfAdapter);
    tf.register(tf::cloudwatch_logs::CloudWatchLogsTfAdapter);
    tf.register(tf::api_gateway::ApiGatewayTfAdapter);
    tf.register(tf::nat_gateway::NatGatewayTfAdapter);
    tf.register(tf::cloudfront::CloudFrontTfAdapter);
    tf.register(tf::elasticache::ElastiCacheTfAdapter);
    tf.register(tf::step_functions::StepFunctionsTfAdapter);
    tf.register(tf::eventbridge_scheduler::EventBridgeSchedulerTfAdapter);
    tf.register(tf::batch::BatchTfAdapter);
}
