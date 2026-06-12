//! Tests for structured-property connection edge extraction in the CFn converter.
//!
//! Covers:
//!   s3-notification.yml  — S3 NotificationConfiguration -> Lambda/SQS/SNS
//!   sam-events.yml       — SAM Function Events (SQS/Kinesis/DynamoDB types)
//!   events-rule.yml      — AWS::Events::Rule Targets -> Lambda/SQS
//!   sns-subscription.yml — SNS Topic inline Subscription + standalone Subscription resource

mod common;
use common::load_fixture;

use std::collections::{BTreeMap, HashMap};

use yevice_cfn::{convert, parser};
use yevice_core::resource::ConnectionType;
use yevice_service_api::{CfnAdapterRegistry, TfAdapterRegistry};
use yevice_services_aws::register;

const REGION: &str = "ap-northeast-1";

fn build_arch_connections(
    name: &str,
    resources: &BTreeMap<String, parser::ResolvedResource>,
) -> yevice_core::resource::Architecture {
    let tmpl = parser::CfnTemplate {
        parameters: HashMap::new(),
        mappings: HashMap::new(),
        conditions: HashMap::new(),
        resources: resources.clone(),
    };
    let mut cfn = CfnAdapterRegistry::new();
    let mut tf = TfAdapterRegistry::new();
    let mut catalog = yevice_service_api::ServiceCatalog::new();
    register(&mut catalog, &mut cfn, &mut tf);
    convert::build_architecture(name, REGION, &tmpl, &cfn)
}

// ---------------------------------------------------------------------------
// S3 NotificationConfiguration tests
// ---------------------------------------------------------------------------

#[test]
fn test_s3_notification_lambda_edge() {
    let resources = load_fixture("s3-notification.yml");
    let arch = build_arch_connections("s3-notification", &resources);

    let edge = arch.connections.iter().find(|c| {
        c.source.as_str() == "InputBucket"
            && c.target.as_str() == "ProcessorFunction"
            && matches!(c.connection_type, ConnectionType::Notification)
    });
    assert!(
        edge.is_some(),
        "expected Notification edge InputBucket -> ProcessorFunction, connections: {:?}",
        arch.connections
            .iter()
            .map(|c| format!("{}->{:?}", c.source, c.target))
            .collect::<Vec<_>>()
    );
}

#[test]
fn test_s3_notification_sqs_edge() {
    let resources = load_fixture("s3-notification.yml");
    let arch = build_arch_connections("s3-notification", &resources);

    let edge = arch.connections.iter().find(|c| {
        c.source.as_str() == "InputBucket"
            && c.target.as_str() == "NotifyQueue"
            && matches!(c.connection_type, ConnectionType::Notification)
    });
    assert!(
        edge.is_some(),
        "expected Notification edge InputBucket -> NotifyQueue"
    );
}

#[test]
fn test_s3_notification_sns_edge() {
    let resources = load_fixture("s3-notification.yml");
    let arch = build_arch_connections("s3-notification", &resources);

    let edge = arch.connections.iter().find(|c| {
        c.source.as_str() == "InputBucket"
            && c.target.as_str() == "NotifyTopic"
            && matches!(c.connection_type, ConnectionType::Notification)
    });
    assert!(
        edge.is_some(),
        "expected Notification edge InputBucket -> NotifyTopic"
    );
}

#[test]
fn test_s3_notification_no_duplicate_edges() {
    let resources = load_fixture("s3-notification.yml");
    let arch = build_arch_connections("s3-notification", &resources);

    let count = arch
        .connections
        .iter()
        .filter(|c| {
            c.source.as_str() == "InputBucket"
                && c.target.as_str() == "ProcessorFunction"
                && matches!(c.connection_type, ConnectionType::Notification)
        })
        .count();
    assert_eq!(
        count, 1,
        "InputBucket -> ProcessorFunction Notification edge must appear exactly once, got {count}"
    );
}

// ---------------------------------------------------------------------------
// SAM Function Events tests
// ---------------------------------------------------------------------------

#[test]
fn test_sam_sqs_event_source_edge() {
    let resources = load_fixture("sam-events.yml");
    let arch = build_arch_connections("sam-events", &resources);

    let edge = arch.connections.iter().find(|c| {
        c.source.as_str() == "MyQueue"
            && c.target.as_str() == "SqsFunction"
            && matches!(c.connection_type, ConnectionType::EventSource)
    });
    assert!(
        edge.is_some(),
        "expected EventSource edge MyQueue -> SqsFunction, connections: {:?}",
        arch.connections
            .iter()
            .map(|c| format!("{}->{:?}", c.source, c.target))
            .collect::<Vec<_>>()
    );

    // batch_size should be 10 as specified
    let edge = edge.unwrap();
    assert_eq!(edge.batch_size, Some(10.0), "batch_size should be 10");
}

#[test]
fn test_sam_kinesis_event_source_edge() {
    let resources = load_fixture("sam-events.yml");
    let arch = build_arch_connections("sam-events", &resources);

    let edge = arch.connections.iter().find(|c| {
        c.source.as_str() == "MyStream"
            && c.target.as_str() == "KinesisFunction"
            && matches!(c.connection_type, ConnectionType::EventSource)
    });
    assert!(
        edge.is_some(),
        "expected EventSource edge MyStream -> KinesisFunction"
    );

    let edge = edge.unwrap();
    assert_eq!(edge.batch_size, Some(100.0), "batch_size should be 100");
    assert_eq!(
        edge.parallelization_factor,
        Some(2.0),
        "parallelization_factor should be 2"
    );
}

#[test]
fn test_sam_dynamodb_event_source_edge() {
    let resources = load_fixture("sam-events.yml");
    let arch = build_arch_connections("sam-events", &resources);

    let edge = arch.connections.iter().find(|c| {
        c.source.as_str() == "MyTable"
            && c.target.as_str() == "DynamoFunction"
            && matches!(c.connection_type, ConnectionType::EventSource)
    });
    assert!(
        edge.is_some(),
        "expected EventSource edge MyTable -> DynamoFunction"
    );
}

#[test]
fn test_sam_events_no_duplicate_edges() {
    // If both an EventSourceMapping and SAM Events produce the same (source, target, type),
    // only one edge should appear.
    let template_str = r#"
AWSTemplateFormatVersion: "2010-09-09"
Transform: AWS::Serverless-2016-10-31
Resources:
  MyQueue:
    Type: AWS::SQS::Queue
    Properties:
      VisibilityTimeout: 30

  MyFunction:
    Type: AWS::Serverless::Function
    Properties:
      FunctionName: dedup-test
      MemorySize: 128
      Timeout: 30
      Events:
        QueueEvent:
          Type: SQS
          Properties:
            Queue: !GetAtt MyQueue.Arn
            BatchSize: 5

  # Explicit EventSourceMapping for the same queue → should NOT create a second edge
  MyESM:
    Type: AWS::Lambda::EventSourceMapping
    Properties:
      BatchSize: 5
      EventSourceArn: !GetAtt MyQueue.Arn
      FunctionName: !GetAtt MyFunction.Arn
"#;
    let tmpl = parser::parse_template_str(template_str).unwrap();
    let resources = parser::resolve_template(&tmpl, &HashMap::new(), &HashMap::new()).unwrap();
    let arch = build_arch_connections("dedup-test", &resources);

    let count = arch
        .connections
        .iter()
        .filter(|c| {
            c.source.as_str() == "MyQueue"
                && c.target.as_str() == "MyFunction"
                && matches!(c.connection_type, ConnectionType::EventSource)
        })
        .count();
    assert_eq!(
        count, 1,
        "MyQueue -> MyFunction EventSource edge must appear exactly once (dedup), got {count}"
    );
}

// ---------------------------------------------------------------------------
// AWS::Events::Rule Targets tests
// ---------------------------------------------------------------------------

#[test]
fn test_events_rule_lambda_invocation_edge() {
    let resources = load_fixture("events-rule.yml");
    let arch = build_arch_connections("events-rule", &resources);

    let edge = arch.connections.iter().find(|c| {
        c.source.as_str() == "ScheduleRule"
            && c.target.as_str() == "HandlerFunction"
            && matches!(c.connection_type, ConnectionType::Invocation)
    });
    assert!(
        edge.is_some(),
        "expected Invocation edge ScheduleRule -> HandlerFunction, connections: {:?}",
        arch.connections
            .iter()
            .map(|c| format!("{}->{:?}", c.source, c.target))
            .collect::<Vec<_>>()
    );
}

#[test]
fn test_events_rule_sqs_invocation_edge() {
    let resources = load_fixture("events-rule.yml");
    let arch = build_arch_connections("events-rule", &resources);

    let edge = arch.connections.iter().find(|c| {
        c.source.as_str() == "ScheduleRule"
            && c.target.as_str() == "HandlerQueue"
            && matches!(c.connection_type, ConnectionType::Invocation)
    });
    assert!(
        edge.is_some(),
        "expected Invocation edge ScheduleRule -> HandlerQueue"
    );
}

// ---------------------------------------------------------------------------
// SNS Subscription tests
// ---------------------------------------------------------------------------

#[test]
fn test_sns_topic_inline_subscription_edge() {
    let resources = load_fixture("sns-subscription.yml");
    let arch = build_arch_connections("sns-subscription", &resources);

    // MyTopic has inline Subscription -> SubscriberFunction
    let edge = arch.connections.iter().find(|c| {
        c.source.as_str() == "MyTopic"
            && c.target.as_str() == "SubscriberFunction"
            && matches!(c.connection_type, ConnectionType::Notification)
    });
    assert!(
        edge.is_some(),
        "expected Notification edge MyTopic -> SubscriberFunction, connections: {:?}",
        arch.connections
            .iter()
            .map(|c| format!("{}->{:?}", c.source, c.target))
            .collect::<Vec<_>>()
    );
}

#[test]
fn test_sns_subscription_resource_edge() {
    let resources = load_fixture("sns-subscription.yml");
    let arch = build_arch_connections("sns-subscription", &resources);

    // QueueSubscription (AWS::SNS::Subscription) -> AnotherTopic -> SubscriberQueue
    let edge = arch.connections.iter().find(|c| {
        c.source.as_str() == "AnotherTopic"
            && c.target.as_str() == "SubscriberQueue"
            && matches!(c.connection_type, ConnectionType::Notification)
    });
    assert!(
        edge.is_some(),
        "expected Notification edge AnotherTopic -> SubscriberQueue (via standalone Subscription resource)"
    );
}

// ---------------------------------------------------------------------------
// DependsOn parsing test
// ---------------------------------------------------------------------------

#[test]
fn test_depends_on_parsed_but_no_edge_created() {
    let template_str = r#"
AWSTemplateFormatVersion: "2010-09-09"
Resources:
  BucketA:
    Type: AWS::S3::Bucket
    Properties:
      BucketName: bucket-a
  BucketB:
    Type: AWS::S3::Bucket
    DependsOn: BucketA
    Properties:
      BucketName: bucket-b
  BucketC:
    Type: AWS::S3::Bucket
    DependsOn:
      - BucketA
      - BucketB
    Properties:
      BucketName: bucket-c
"#;
    let tmpl = parser::parse_template_str(template_str).unwrap();
    // DependsOn is stored but does NOT produce connection edges.
    assert_eq!(tmpl.resources.len(), 3);
    let resources = parser::resolve_template(&tmpl, &HashMap::new(), &HashMap::new()).unwrap();
    let arch = build_arch_connections("depends-on-test", &resources);
    // No connection edges — DependsOn is metadata only.
    assert_eq!(
        arch.connections.len(),
        0,
        "DependsOn must not produce connection edges, got {:?}",
        arch.connections
    );
}
