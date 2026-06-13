//! AWS service plugin implementations for yevice.

pub mod cfn;
pub mod common;
pub mod connection_rules;
#[macro_use]
pub mod macros;
pub mod plugin;
pub mod pricing_adapter;
pub mod pricing_provider;
pub mod quotas;
pub mod services;
pub mod tf;

pub use connection_rules::{
    AwsDataFlowRule, AwsEventSourceRule, AwsInvocationRule, AwsNotificationRule,
    aws_connection_rules,
};
pub use plugin::AwsPlugin;
pub use pricing_adapter::AwsPricingCatalog;
pub use quotas::AwsQuotaProvider;

/// Register all AWS services, CFN adapters, TF adapters, and connection rules.
///
/// The service/adapter table below is the single source of truth for adding a
/// new AWS service: one row registers the service, its CFN adapter, and
/// (optionally) its TF adapter. The `register_aws_services!` macro expands
/// this table into the underlying `register` calls.
///
/// Invariants enforced by `tests/registration_consistency.rs`:
/// - Every row's service appears in `EXPECTED_SERVICE_TO_CFN`.
/// - Every row's CFN adapter has its resource type listed there.
/// - Rows with `tf = ...` appear in `EXPECTED_SERVICE_TO_TF`; rows without
///   `tf = ...` appear in `SERVICES_WITHOUT_TF_ADAPTER`.
/// - Use `=> shared` for services that piggyback on a CFN/TF adapter already
///   registered by an earlier row (e.g. `EcsCfnAdapter` covers both
///   `aws.ecs_fargate` and `aws.ecs_ec2`).
pub fn register(
    catalog: &mut yevice_service_api::ServiceCatalog,
    cfn: &mut yevice_service_api::CfnAdapterRegistry,
    tf: &mut yevice_service_api::TfAdapterRegistry,
) {
    // Connection rules (binding derivation)
    catalog.register_connection_rules(aws_connection_rules());

    // Quota provider
    catalog.register_quota_provider(Box::new(AwsQuotaProvider));

    register_aws_services! {
        catalog, cfn, tf;

        // --- Services with TF adapters ---
        services::lambda::LambdaService
            => cfn = cfn::lambda::LambdaCfnAdapter, tf = tf::lambda::LambdaTfAdapter;
        services::dynamodb::DynamoDbService
            => cfn = cfn::dynamodb::DynamoDbCfnAdapter, tf = tf::dynamodb::DynamoDbTfAdapter;
        services::kinesis::KinesisService
            => cfn = cfn::kinesis::KinesisCfnAdapter, tf = tf::kinesis::KinesisTfAdapter;
        services::s3::S3Service
            => cfn = cfn::s3::S3CfnAdapter, tf = tf::s3::S3TfAdapter;
        services::sqs::SqsService
            => cfn = cfn::sqs::SqsCfnAdapter, tf = tf::sqs::SqsTfAdapter;
        services::ec2::Ec2Service
            => cfn = cfn::ec2::Ec2CfnAdapter, tf = tf::ec2::Ec2TfAdapter;
        services::ecs_fargate::EcsFargateService
            => cfn = cfn::ecs::EcsCfnAdapter, tf = tf::ecs::EcsTfAdapter;
        // EcsCfnAdapter / EcsTfAdapter already registered above for ecs_fargate.
        services::ecs_ec2::EcsEc2Service => shared;
        services::ecr::EcrService
            => cfn = cfn::ecr::EcrCfnAdapter, tf = tf::ecr::EcrTfAdapter;
        services::rds::RdsService
            => cfn = cfn::rds::RdsCfnAdapter, tf = tf::rds::RdsTfAdapter;
        services::opensearch_serverless::OpenSearchServerlessService
            => cfn = cfn::opensearch_serverless::OpenSearchServerlessCfnAdapter,
               tf = tf::opensearch_serverless::OpenSearchServerlessTfAdapter;
        services::cloudwatch_logs::CloudWatchLogsService
            => cfn = cfn::cloudwatch_logs::CloudWatchLogsCfnAdapter,
               tf = tf::cloudwatch_logs::CloudWatchLogsTfAdapter;
        services::api_gateway::ApiGatewayService
            => cfn = cfn::api_gateway::ApiGatewayCfnAdapter,
               tf = tf::api_gateway::ApiGatewayTfAdapter;
        services::nat_gateway::NatGatewayService
            => cfn = cfn::nat_gateway::NatGatewayCfnAdapter,
               tf = tf::nat_gateway::NatGatewayTfAdapter;
        services::cloudfront::CloudFrontService
            => cfn = cfn::cloudfront::CloudFrontCfnAdapter,
               tf = tf::cloudfront::CloudFrontTfAdapter;
        services::elasticache::ElastiCacheService
            => cfn = cfn::elasticache::ElastiCacheCfnAdapter,
               tf = tf::elasticache::ElastiCacheTfAdapter;
        services::step_functions::StepFunctionsService
            => cfn = cfn::step_functions::StepFunctionsCfnAdapter,
               tf = tf::step_functions::StepFunctionsTfAdapter;
        services::eventbridge_scheduler::EventBridgeSchedulerService
            => cfn = cfn::eventbridge_scheduler::EventBridgeSchedulerCfnAdapter,
               tf = tf::eventbridge_scheduler::EventBridgeSchedulerTfAdapter;
        services::batch::BatchService
            => cfn = cfn::batch::BatchCfnAdapter, tf = tf::batch::BatchTfAdapter;

        // --- Services without TF adapters (see registration_consistency tests) ---
        services::bedrock::BedrockService     => cfn = cfn::bedrock::BedrockCfnAdapter;
        services::alb::AlbService             => cfn = cfn::alb::AlbCfnAdapter;
        services::sns::SnsService             => cfn = cfn::sns::SnsCfnAdapter;
        services::msk::MskService             => cfn = cfn::msk::MskCfnAdapter;
        services::eks::EksService             => cfn = cfn::eks::EksCfnAdapter;
        services::firehose::FirehoseService   => cfn = cfn::firehose::FirehoseCfnAdapter;
        services::secrets_manager::SecretsManagerService
            => cfn = cfn::secrets_manager::SecretsManagerCfnAdapter;
        services::waf::WafService             => cfn = cfn::waf::WafCfnAdapter;
        services::efs::EfsService             => cfn = cfn::efs::EfsCfnAdapter;
        services::eventbridge_rule::EventBridgeRuleService
            => cfn = cfn::eventbridge_rule::EventBridgeRuleCfnAdapter;
        services::athena::AthenaService       => cfn = cfn::athena::AthenaCfnAdapter;
        services::opensearch_service::OpenSearchServiceService
            => cfn = cfn::opensearch_service::OpenSearchServiceCfnAdapter;
        services::documentdb::DocumentDbService
            => cfn = cfn::documentdb::DocumentDbCfnAdapter;
        services::glue::GlueService           => cfn = cfn::glue::GlueCfnAdapter;
        services::appsync::AppSyncService     => cfn = cfn::appsync::AppSyncCfnAdapter;
        services::cognito::CognitoService     => cfn = cfn::cognito::CognitoCfnAdapter;
        services::redshift::RedshiftService   => cfn = cfn::redshift::RedshiftCfnAdapter;
        services::route53::Route53Service     => cfn = cfn::route53::Route53CfnAdapter;
        services::quicksight::QuickSightService
            => cfn = cfn::quicksight::QuickSightCfnAdapter;
        services::lightsail::LightsailService => cfn = cfn::lightsail::LightsailCfnAdapter;
        services::container_insights::ContainerInsightsService
            => cfn = cfn::container_insights::EcsClusterCfnAdapter;
        services::ebs::EbsService             => cfn = cfn::ebs::EbsCfnAdapter;
        services::vpn::VpnService             => cfn = cfn::vpn::VpnCfnAdapter;
        services::kendra::KendraService       => cfn = cfn::kendra::KendraCfnAdapter;
        services::transcribe::TranscribeService
            => cfn = cfn::transcribe::TranscribeCfnAdapter;
        services::fsx_windows::FsxWindowsService
            => cfn = cfn::fsx_windows::FsxWindowsCfnAdapter;
        services::directory_service::DirectoryServiceService
            => cfn = cfn::directory_service::DirectoryServiceCfnAdapter;
        services::cloudwatch::CloudWatchService
            => cfn = cfn::cloudwatch::CloudWatchCfnAdapter;
        services::guardduty::GuardDutyService => cfn = cfn::guardduty::GuardDutyCfnAdapter;
        services::cloudtrail::CloudTrailService
            => cfn = cfn::cloudtrail::CloudTrailCfnAdapter;
        services::backup::BackupService       => cfn = cfn::backup::BackupCfnAdapter;
        services::data_transfer::DataTransferService
            => cfn = cfn::data_transfer::DataTransferCfnAdapter;
    }
}
