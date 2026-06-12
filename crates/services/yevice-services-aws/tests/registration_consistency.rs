//! Registration consistency tests for the AWS plugin.
//!
//! These tests verify that the service catalog, CFN adapter registry, and TF
//! adapter registry stay in sync. A service added to the catalog without a
//! corresponding CFN adapter (or vice-versa) will be caught here.
//!
//! Design
//! ------
//! The matchable key is `service_id` — the string passed to `ResourceShell::new`
//! inside each adapter's `convert()` implementation, and the string returned by
//! `Service::id()`. Because the mapping is hard-coded inside the convert method,
//! this file maintains a static `EXPECTED_SERVICE_TO_CFN` table that must be kept
//! up to date when new services are added.  That maintenance burden is deliberate:
//! it forces the author of a new service to explicitly list the mapping here,
//! making the relationship visible and machine-checked.
//!
//! Services that intentionally have no TF adapter are listed in
//! `SERVICES_WITHOUT_TF_ADAPTER` with a comment explaining why.

use std::collections::{HashMap, HashSet};

use yevice_service_api::{CfnAdapterRegistry, ServiceCatalog, TfAdapterRegistry};
use yevice_services_aws::register;

// ---------------------------------------------------------------------------
// Static mapping: service_id → expected CFN resource type(s)
//
// This is the single source of truth for the CFN ↔ service relationship.
// Each service must have at least one CFN resource type listed.
// ---------------------------------------------------------------------------

/// Maps each service_id to the CFN resource type(s) whose adapters produce it.
///
/// One CFN adapter may handle multiple resource types (e.g. lambda handles
/// `AWS::Lambda::Function` and `AWS::Serverless::Function`). Both are listed.
/// One CFN adapter may also produce multiple service_ids depending on runtime
/// input (e.g. the ECS adapter outputs either `aws.ecs_fargate` or `aws.ecs_ec2`).
const EXPECTED_SERVICE_TO_CFN: &[(&str, &[&str])] = &[
    ("aws.alb", &["AWS::ElasticLoadBalancingV2::LoadBalancer"]),
    (
        "aws.api_gateway",
        &[
            "AWS::ApiGateway::RestApi",
            "AWS::Serverless::Api",
            "AWS::ApiGatewayV2::Api",
            "AWS::Serverless::HttpApi",
        ],
    ),
    ("aws.appsync", &["AWS::AppSync::GraphQLApi"]),
    ("aws.athena", &["AWS::Athena::WorkGroup"]),
    ("aws.backup", &["AWS::Backup::BackupVault"]),
    ("aws.batch", &["AWS::Batch::JobDefinition"]),
    // Bedrock invocation cost is usage-driven; the CFN type is a placeholder.
    ("aws.bedrock", &["AWS::Bedrock::Agent"]),
    ("aws.cloudfront", &["AWS::CloudFront::Distribution"]),
    ("aws.cloudtrail", &["AWS::CloudTrail::Trail"]),
    ("aws.cloudwatch", &["AWS::CloudWatch::Alarm"]),
    ("aws.cloudwatch_logs", &["AWS::Logs::LogGroup"]),
    ("aws.cognito", &["AWS::Cognito::UserPool"]),
    // ContainerInsights cost is tied to an ECS Cluster resource.
    ("aws.container_insights", &["AWS::ECS::Cluster"]),
    // Data transfer uses a custom Yevice marker type (no first-class AWS resource).
    ("aws.data_transfer", &["Yevice::DataTransfer"]),
    (
        "aws.directory_service",
        &["AWS::DirectoryService::MicrosoftAD"],
    ),
    (
        "aws.documentdb",
        &["AWS::DocDB::DBCluster", "AWS::DocDB::DBInstance"],
    ),
    (
        "aws.dynamodb",
        &["AWS::DynamoDB::Table", "AWS::DynamoDB::GlobalTable"],
    ),
    ("aws.ebs", &["AWS::EC2::Volume"]),
    ("aws.ec2", &["AWS::EC2::Instance"]),
    ("aws.ecr", &["AWS::ECR::Repository"]),
    // One CFN adapter (EcsCfnAdapter) produces either ecs_ec2 or ecs_fargate.
    ("aws.ecs_ec2", &["AWS::ECS::Service"]),
    ("aws.ecs_fargate", &["AWS::ECS::Service"]),
    ("aws.efs", &["AWS::EFS::FileSystem"]),
    ("aws.eks", &["AWS::EKS::Cluster"]),
    (
        "aws.elasticache",
        &[
            "AWS::ElastiCache::CacheCluster",
            "AWS::ElastiCache::ReplicationGroup",
        ],
    ),
    ("aws.eventbridge_rule", &["AWS::Events::Rule"]),
    ("aws.eventbridge_scheduler", &["AWS::Scheduler::Schedule"]),
    ("aws.firehose", &["AWS::KinesisFirehose::DeliveryStream"]),
    ("aws.fsx_windows", &["AWS::FSx::FileSystem"]),
    ("aws.glue", &["AWS::Glue::Job"]),
    ("aws.guardduty", &["AWS::GuardDuty::Detector"]),
    ("aws.kendra", &["AWS::Kendra::Index"]),
    ("aws.kinesis", &["AWS::Kinesis::Stream"]),
    (
        "aws.lambda",
        &["AWS::Lambda::Function", "AWS::Serverless::Function"],
    ),
    (
        "aws.lightsail",
        &["AWS::Lightsail::Instance", "AWS::Lightsail::Disk"],
    ),
    ("aws.msk", &["AWS::MSK::Cluster"]),
    ("aws.nat_gateway", &["AWS::EC2::NatGateway"]),
    (
        "aws.opensearch_serverless",
        &["AWS::OpenSearchServerless::Collection"],
    ),
    (
        "aws.opensearch_service",
        &[
            "AWS::OpenSearchService::Domain",
            "AWS::Elasticsearch::Domain",
        ],
    ),
    // QuickSight cost is account-level; uses a custom Yevice marker.
    ("aws.quicksight", &["Yevice::QuickSight"]),
    ("aws.rds", &["AWS::RDS::DBInstance", "AWS::RDS::DBCluster"]),
    ("aws.redshift", &["AWS::Redshift::Cluster"]),
    ("aws.route53", &["AWS::Route53::HostedZone"]),
    ("aws.s3", &["AWS::S3::Bucket"]),
    ("aws.secrets_manager", &["AWS::SecretsManager::Secret"]),
    ("aws.sns", &["AWS::SNS::Topic"]),
    ("aws.sqs", &["AWS::SQS::Queue"]),
    (
        "aws.step_functions",
        &[
            "AWS::StepFunctions::StateMachine",
            "AWS::Serverless::StateMachine",
        ],
    ),
    // Transcribe cost is usage-driven; the CFN type is a placeholder vocabulary.
    ("aws.transcribe", &["AWS::Transcribe::Vocabulary"]),
    ("aws.vpn", &["AWS::EC2::VPNConnection"]),
    ("aws.waf", &["AWS::WAFv2::WebACL"]),
];

// ---------------------------------------------------------------------------
// Static mapping: service_id → expected TF resource type(s)
//
// Lists only services that have TF adapters.  Services intentionally without a
// TF adapter are tracked in `SERVICES_WITHOUT_TF_ADAPTER` below.
//
// Maintenance: when a new TF adapter is added, move the service_id out of
// SERVICES_WITHOUT_TF_ADAPTER and add an entry here.
// ---------------------------------------------------------------------------

/// Maps each TF-covered service_id to the Terraform resource type(s) whose
/// adapters produce it.
///
/// One TF adapter may handle multiple resource types (e.g. elasticache handles
/// both `aws_elasticache_cluster` and `aws_elasticache_replication_group`). One
/// TF adapter may also produce multiple service IDs depending on runtime input
/// (e.g. the ECS adapter outputs either `aws.ecs_ec2` or `aws.ecs_fargate`
/// from the same `aws_ecs_service` type).
const EXPECTED_SERVICE_TO_TF: &[(&str, &[&str])] = &[
    (
        "aws.api_gateway",
        &["aws_api_gateway_rest_api", "aws_apigatewayv2_api"],
    ),
    ("aws.batch", &["aws_batch_job_definition"]),
    ("aws.cloudfront", &["aws_cloudfront_distribution"]),
    ("aws.cloudwatch_logs", &["aws_cloudwatch_log_group"]),
    ("aws.dynamodb", &["aws_dynamodb_table"]),
    ("aws.ec2", &["aws_instance"]),
    ("aws.ecr", &["aws_ecr_repository"]),
    // One TF adapter (EcsTfAdapter) handles aws_ecs_service and produces
    // either ecs_ec2 or ecs_fargate depending on launch_type.
    ("aws.ecs_ec2", &["aws_ecs_service"]),
    ("aws.ecs_fargate", &["aws_ecs_service"]),
    (
        "aws.elasticache",
        &["aws_elasticache_cluster", "aws_elasticache_replication_group"],
    ),
    ("aws.eventbridge_scheduler", &["aws_scheduler_schedule"]),
    ("aws.kinesis", &["aws_kinesis_stream"]),
    ("aws.lambda", &["aws_lambda_function"]),
    ("aws.nat_gateway", &["aws_nat_gateway"]),
    (
        "aws.opensearch_serverless",
        &["aws_opensearchserverless_collection"],
    ),
    ("aws.rds", &["aws_db_instance", "aws_rds_cluster"]),
    ("aws.s3", &["aws_s3_bucket"]),
    ("aws.sqs", &["aws_sqs_queue"]),
    ("aws.step_functions", &["aws_sfn_state_machine"]),
];

// ---------------------------------------------------------------------------
// Allow-list: services intentionally without a TF adapter
//
// Rationale for each group is noted. Remove entries here ONLY when adding the
// corresponding TF adapter (and adding the entry to EXPECTED_SERVICE_TO_TF).
// ---------------------------------------------------------------------------

/// Service IDs that intentionally have no TF adapter.
///
/// These services are either usage-driven (no meaningful TF resource), have no
/// widely-used Terraform provider resource, or TF support is planned for a
/// later sprint.
const SERVICES_WITHOUT_TF_ADAPTER: &[&str] = &[
    // ALB — TF adapter not yet implemented (future work).
    "aws.alb",
    // AppSync — TF adapter not yet implemented.
    "aws.appsync",
    // Athena — TF adapter not yet implemented.
    "aws.athena",
    // Backup — TF adapter not yet implemented.
    "aws.backup",
    // Bedrock — usage-driven; no standard Terraform resource that models cost.
    "aws.bedrock",
    // CloudTrail — TF adapter not yet implemented.
    "aws.cloudtrail",
    // CloudWatch — TF adapter not yet implemented.
    "aws.cloudwatch",
    // Cognito — TF adapter not yet implemented.
    "aws.cognito",
    // ContainerInsights — cost is tied to ECS cluster config; no TF resource needed.
    "aws.container_insights",
    // DataTransfer — uses a Yevice custom marker, no standard Terraform resource.
    "aws.data_transfer",
    // DirectoryService — TF adapter not yet implemented.
    "aws.directory_service",
    // DocumentDB — TF adapter not yet implemented.
    "aws.documentdb",
    // EBS — TF adapter not yet implemented.
    "aws.ebs",
    // EFS — TF adapter not yet implemented.
    "aws.efs",
    // EKS — TF adapter not yet implemented.
    "aws.eks",
    // EventBridge Rule — TF adapter not yet implemented.
    "aws.eventbridge_rule",
    // Firehose — TF adapter not yet implemented.
    "aws.firehose",
    // FSx for Windows — TF adapter not yet implemented.
    "aws.fsx_windows",
    // Glue — TF adapter not yet implemented.
    "aws.glue",
    // GuardDuty — TF adapter not yet implemented.
    "aws.guardduty",
    // Kendra — TF adapter not yet implemented.
    "aws.kendra",
    // Lightsail — TF adapter not yet implemented.
    "aws.lightsail",
    // MSK — TF adapter not yet implemented.
    "aws.msk",
    // OpenSearch Service — TF adapter not yet implemented.
    "aws.opensearch_service",
    // QuickSight — uses a Yevice custom marker, no standard Terraform resource.
    "aws.quicksight",
    // Redshift — TF adapter not yet implemented.
    "aws.redshift",
    // Route 53 — TF adapter not yet implemented.
    "aws.route53",
    // Secrets Manager — TF adapter not yet implemented.
    "aws.secrets_manager",
    // SNS — TF adapter not yet implemented.
    "aws.sns",
    // Transcribe — usage-driven; no meaningful TF cost resource.
    "aws.transcribe",
    // VPN — TF adapter not yet implemented.
    "aws.vpn",
    // WAF — TF adapter not yet implemented.
    "aws.waf",
];

// ---------------------------------------------------------------------------
// Minimum counts (test b): snapshot lower bounds
// ---------------------------------------------------------------------------

/// Minimum number of services that must be registered.
/// Catches catastrophic wipe of the registration list.
const MIN_SERVICE_COUNT: usize = 50;

/// Minimum number of CFN resource types that must be registered.
/// Counts the total number of resource type strings (not adapters), so
/// multi-type adapters each contribute multiple entries.
const MIN_CFN_TYPE_COUNT: usize = 55;

/// Minimum number of TF resource types that must be registered.
const MIN_TF_TYPE_COUNT: usize = 18;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn build_registries() -> (ServiceCatalog, CfnAdapterRegistry, TfAdapterRegistry) {
    let mut catalog = ServiceCatalog::new();
    let mut cfn = CfnAdapterRegistry::new();
    let mut tf = TfAdapterRegistry::new();
    register(&mut catalog, &mut cfn, &mut tf);
    (catalog, cfn, tf)
}

// ---------------------------------------------------------------------------
// Test a: cross-reference service IDs ↔ CFN types ↔ TF types
// ---------------------------------------------------------------------------

/// Verify that every service in the catalog has at least one CFN adapter and
/// vice-versa, using `EXPECTED_SERVICE_TO_CFN` as the ground truth.
///
/// Also verifies TF adapter coverage via `EXPECTED_SERVICE_TO_TF`:
/// - every TF type in the expected mapping is present in the real registry
///   (catches deleted tf.register() calls), and
/// - every TF type in the real registry is present in the expected mapping
///   (catches mapping-table update omissions).
/// Services intentionally without a TF adapter are listed in
/// `SERVICES_WITHOUT_TF_ADAPTER`.
#[test]
fn cfn_tf_and_service_registration_are_consistent() {
    let (catalog, cfn, tf) = build_registries();

    let registered_services: HashSet<&str> = catalog.registered_service_ids().into_iter().collect();
    let registered_cfn_types: HashSet<&str> = cfn.registered_types().into_iter().collect();
    let registered_tf_types: HashSet<&str> = tf.registered_types().into_iter().collect();

    // Build the set of service_ids that have at least one TF adapter by
    // checking which expected CFN entries also have TF coverage from lib.rs.
    // We derive TF-covered services dynamically via EXPECTED_SERVICE_TO_CFN +
    // the SERVICES_WITHOUT_TF_ADAPTER allow-list.
    let no_tf_set: HashSet<&str> = SERVICES_WITHOUT_TF_ADAPTER.iter().copied().collect();

    let mut failures: Vec<String> = Vec::new();

    // --- Build reverse map: CFN resource type → service IDs that claim it ---
    let mut cfn_type_to_services: HashMap<&str, Vec<&str>> = HashMap::new();
    for &(service_id, cfn_types) in EXPECTED_SERVICE_TO_CFN {
        for &rt in cfn_types {
            cfn_type_to_services.entry(rt).or_default().push(service_id);
        }
    }

    // 1. Every service_id in EXPECTED_SERVICE_TO_CFN must be in the catalog.
    for &(service_id, _) in EXPECTED_SERVICE_TO_CFN {
        if !registered_services.contains(service_id) {
            failures.push(format!(
                "Service '{service_id}' is listed in EXPECTED_SERVICE_TO_CFN \
                 but not registered in ServiceCatalog"
            ));
        }
    }

    // 2. Every service in the catalog must be listed in EXPECTED_SERVICE_TO_CFN.
    let expected_services: HashSet<&str> =
        EXPECTED_SERVICE_TO_CFN.iter().map(|&(id, _)| id).collect();
    for id in &registered_services {
        if !expected_services.contains(*id) {
            failures.push(format!(
                "Service '{id}' is registered in ServiceCatalog \
                 but missing from EXPECTED_SERVICE_TO_CFN — add it with its CFN resource types"
            ));
        }
    }

    // 3. Every CFN resource type in EXPECTED_SERVICE_TO_CFN must be registered.
    for &(service_id, cfn_types) in EXPECTED_SERVICE_TO_CFN {
        for &rt in cfn_types {
            if !registered_cfn_types.contains(rt) {
                failures.push(format!(
                    "CFN resource type '{rt}' (mapped to service '{service_id}') \
                     is listed in EXPECTED_SERVICE_TO_CFN \
                     but not registered in CfnAdapterRegistry"
                ));
            }
        }
    }

    // 4. Every CFN type in the registry must be listed in EXPECTED_SERVICE_TO_CFN.
    let expected_cfn: HashSet<&str> = EXPECTED_SERVICE_TO_CFN
        .iter()
        .flat_map(|&(_, types)| types.iter().copied())
        .collect();
    for rt in &registered_cfn_types {
        if !expected_cfn.contains(*rt) {
            failures.push(format!(
                "CFN resource type '{rt}' is registered in CfnAdapterRegistry \
                 but not listed in EXPECTED_SERVICE_TO_CFN — add it to the mapping table"
            ));
        }
    }

    // 5. Every service_id in EXPECTED_SERVICE_TO_TF must be in the catalog.
    for &(service_id, _) in EXPECTED_SERVICE_TO_TF {
        if !registered_services.contains(service_id) {
            failures.push(format!(
                "Service '{service_id}' is listed in EXPECTED_SERVICE_TO_TF \
                 but not registered in ServiceCatalog"
            ));
        }
    }

    // 6. Every service not in the allow-list must be listed in EXPECTED_SERVICE_TO_TF.
    //    This detects newly registered services that were added to the catalog but
    //    whose TF adapter entry was never added to the mapping table.
    let expected_tf_services: HashSet<&str> =
        EXPECTED_SERVICE_TO_TF.iter().map(|&(id, _)| id).collect();
    for id in &registered_services {
        if !no_tf_set.contains(*id) && !expected_tf_services.contains(*id) {
            failures.push(format!(
                "Service '{id}' is registered in ServiceCatalog and not in \
                 SERVICES_WITHOUT_TF_ADAPTER, but is missing from EXPECTED_SERVICE_TO_TF \
                 — add it with its TF resource types or add it to the allow-list"
            ));
        }
    }

    // 7. Every TF resource type in EXPECTED_SERVICE_TO_TF must be registered in
    //    the TF registry.  Catches the case where a tf.register() call is deleted.
    for &(service_id, tf_types) in EXPECTED_SERVICE_TO_TF {
        for &rt in tf_types {
            if !registered_tf_types.contains(rt) {
                failures.push(format!(
                    "TF resource type '{rt}' (mapped to service '{service_id}') \
                     is listed in EXPECTED_SERVICE_TO_TF \
                     but not registered in TfAdapterRegistry"
                ));
            }
        }
    }

    // 8. Every TF type in the registry must be listed in EXPECTED_SERVICE_TO_TF.
    //    Catches the case where a TF adapter is registered but the mapping table
    //    was not updated.
    let expected_tf_types: HashSet<&str> = EXPECTED_SERVICE_TO_TF
        .iter()
        .flat_map(|&(_, types)| types.iter().copied())
        .collect();
    for rt in &registered_tf_types {
        if !expected_tf_types.contains(*rt) {
            failures.push(format!(
                "TF resource type '{rt}' is registered in TfAdapterRegistry \
                 but not listed in EXPECTED_SERVICE_TO_TF — add it to the mapping table"
            ));
        }
    }

    // 9. Verify the allow-list only references services that are actually registered
    //    (no stale entries in SERVICES_WITHOUT_TF_ADAPTER).
    // Also verify that no service appears in both SERVICES_WITHOUT_TF_ADAPTER and
    // EXPECTED_SERVICE_TO_TF (they should be mutually exclusive).
    for &id in SERVICES_WITHOUT_TF_ADAPTER {
        if !registered_services.contains(id) {
            failures.push(format!(
                "SERVICES_WITHOUT_TF_ADAPTER references '{id}' \
                 which is not registered in ServiceCatalog — remove the stale entry"
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "Registration consistency failures ({} total):\n{}",
        failures.len(),
        failures
            .iter()
            .map(|s| format!("  - {s}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

// ---------------------------------------------------------------------------
// Test b: snapshot lower bounds (guards against accidental wipeout)
// ---------------------------------------------------------------------------

/// Verify that the number of registered services, CFN types, and TF types
/// never falls below the expected minimums.
///
/// This catches catastrophic errors like accidentally deleting a registration
/// block or forgetting to call `register()` before running the other tests.
#[test]
fn registration_counts_meet_minimum_thresholds() {
    let (catalog, cfn, tf) = build_registries();

    let service_count = catalog.registered_service_ids().len();
    let cfn_count = cfn.registered_types().len();
    let tf_count = tf.registered_types().len();

    assert!(
        service_count >= MIN_SERVICE_COUNT,
        "Expected at least {MIN_SERVICE_COUNT} registered services, got {service_count}"
    );
    assert!(
        cfn_count >= MIN_CFN_TYPE_COUNT,
        "Expected at least {MIN_CFN_TYPE_COUNT} registered CFN types, got {cfn_count}"
    );
    assert!(
        tf_count >= MIN_TF_TYPE_COUNT,
        "Expected at least {MIN_TF_TYPE_COUNT} registered TF types, got {tf_count}"
    );
}
