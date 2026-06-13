//! Integration tests for event-driven / async architectures.
//!
//! Architectures covered:
//!   sqs-fanout.yml         - SQS Standard + DLQ + Lambda + SNS + LogGroup
//!   async-api.yml          - SQS FIFO + Lambda + SNS + LogGroup
//!   step-functions-workflow.yml - Step Functions Standard + 2 Lambda + 2 LogGroup
//!   express-workflow.yml   - Step Functions Express + Lambda + LogGroup

mod common;
use common::{build_arch, load_fixture, p};

use yevice_core::evaluate::evaluate_architecture;

// ---------------------------------------------------------------------------
// 1. test_sqs_fanout_resource_count
// ---------------------------------------------------------------------------
#[test]
fn test_sqs_fanout_resource_count() {
    let resources = load_fixture("sqs-fanout.yml");
    let arch = build_arch("sqs-fanout", &resources, false);
    // InputQueue, InputDLQ, ProcessorFunction, NotificationTopic, ProcessorLogs
    assert_eq!(
        arch.resources.len(),
        5,
        "sqs-fanout.yml should produce exactly 5 costed resources, got {}",
        arch.resources.len()
    );
}

// ---------------------------------------------------------------------------
// 2. test_sqs_standard_and_fifo_distinguished
// ---------------------------------------------------------------------------
#[test]
fn test_sqs_standard_and_fifo_distinguished() {
    // async-api.yml has one FIFO queue; sqs-fanout.yml has two Standard queues.
    let fanout_resources = load_fixture("sqs-fanout.yml");
    let fanout_arch = build_arch("sqs-fanout", &fanout_resources, false);

    let standard_queues: Vec<_> = fanout_arch
        .resources
        .iter()
        .filter(|r| r.label.starts_with("SQS Standard"))
        .collect();
    assert_eq!(
        standard_queues.len(),
        2,
        "sqs-fanout.yml should have 2 Standard queues"
    );

    let async_resources = load_fixture("async-api.yml");
    let async_arch = build_arch("async-api", &async_resources, false);

    let fifo_queues: Vec<_> = async_arch
        .resources
        .iter()
        .filter(|r| r.label.starts_with("SQS FIFO"))
        .collect();
    assert_eq!(
        fifo_queues.len(),
        1,
        "async-api.yml should have 1 FIFO queue"
    );
}

// ---------------------------------------------------------------------------
// 3. test_sns_topic_has_deliveries_variable
// ---------------------------------------------------------------------------
#[test]
fn test_sns_topic_has_deliveries_variable() {
    let resources = load_fixture("sqs-fanout.yml");
    let arch = build_arch("sqs-fanout", &resources, false);

    let sns_resource = arch
        .resources
        .iter()
        .find(|r| r.label.contains("SNS"))
        .expect("should find an SNS resource in sqs-fanout.yml");

    let has_deliveries = sns_resource
        .required_variables
        .iter()
        .any(|v| v.name.as_str().ends_with("deliveries"));

    assert!(
        has_deliveries,
        "SNS resource required_variables should include a 'deliveries' variable, got: {:?}",
        sns_resource
            .required_variables
            .iter()
            .map(|v| v.name.as_str())
            .collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// 4. test_sqs_cost_scales_with_requests
// ---------------------------------------------------------------------------
#[test]
fn test_sqs_cost_scales_with_requests() {
    let resources = load_fixture("sqs-fanout.yml");
    let arch = build_arch("sqs-fanout", &resources, false);

    // Provide all required variables. Focus on InputQueue.
    let low_params = p(&[
        ("InputQueue_requests", 1_000_000.0),
        ("InputDLQ_requests", 0.0),
        ("ProcessorFunction_requests", 1_000_000.0),
        ("ProcessorFunction_avg_duration_ms", 100.0),
        ("ProcessorFunction_data_transfer_out_gb", 0.0),
        ("NotificationTopic_deliveries", 0.0),
        ("ProcessorLogs_ingestion_gb", 0.0),
        ("ProcessorLogs_storage_gb", 0.0),
    ]);

    let high_params = p(&[
        ("InputQueue_requests", 10_000_000.0),
        ("InputDLQ_requests", 0.0),
        ("ProcessorFunction_requests", 1_000_000.0),
        ("ProcessorFunction_avg_duration_ms", 100.0),
        ("ProcessorFunction_data_transfer_out_gb", 0.0),
        ("NotificationTopic_deliveries", 0.0),
        ("ProcessorLogs_ingestion_gb", 0.0),
        ("ProcessorLogs_storage_gb", 0.0),
    ]);

    let low_result = evaluate_architecture(&arch, &low_params).unwrap();
    let high_result = evaluate_architecture(&arch, &high_params).unwrap();

    assert!(
        high_result.naive_total() > low_result.naive_total(),
        "10M SQS requests (${:.4}) should cost more than 1M requests (${:.4})",
        high_result.naive_total(),
        low_result.naive_total()
    );
}

// ---------------------------------------------------------------------------
// 5. test_fifo_costs_more_than_standard
// ---------------------------------------------------------------------------
#[test]
fn test_fifo_costs_more_than_standard() {
    // Standard: $0.0000004/req after 1M free
    // FIFO:     $0.0000005/req after 1M free
    // At 5M requests, Standard: 4M * 0.0000004 = $1.60; FIFO: 4M * 0.0000005 = $2.00

    let fanout_resources = load_fixture("sqs-fanout.yml");
    let fanout_arch = build_arch("sqs-fanout", &fanout_resources, false);

    let async_resources = load_fixture("async-api.yml");
    let async_arch = build_arch("async-api", &async_resources, false);

    let standard_params = p(&[
        ("InputQueue_requests", 5_000_000.0),
        ("InputDLQ_requests", 0.0),
        ("ProcessorFunction_requests", 0.0),
        ("ProcessorFunction_avg_duration_ms", 0.0),
        ("ProcessorFunction_data_transfer_out_gb", 0.0),
        ("NotificationTopic_deliveries", 0.0),
        ("ProcessorLogs_ingestion_gb", 0.0),
        ("ProcessorLogs_storage_gb", 0.0),
    ]);

    let fifo_params = p(&[
        ("ApiQueue_requests", 5_000_000.0),
        ("WorkerFunction_requests", 0.0),
        ("WorkerFunction_avg_duration_ms", 0.0),
        ("WorkerFunction_data_transfer_out_gb", 0.0),
        ("ResultTopic_deliveries", 0.0),
        ("WorkerLogs_ingestion_gb", 0.0),
        ("WorkerLogs_storage_gb", 0.0),
    ]);

    let standard_result = evaluate_architecture(&fanout_arch, &standard_params).unwrap();
    let fifo_result = evaluate_architecture(&async_arch, &fifo_params).unwrap();

    // Extract just the InputQueue cost from standard
    let standard_queue_cost = standard_result
        .resources
        .iter()
        .find(|r| r.logical_id == "InputQueue")
        .unwrap()
        .monthly_cost
        .value;

    // Extract just the ApiQueue cost from fifo
    let fifo_queue_cost = fifo_result
        .resources
        .iter()
        .find(|r| r.logical_id == "ApiQueue")
        .unwrap()
        .monthly_cost
        .value;

    assert!(
        fifo_queue_cost > standard_queue_cost,
        "FIFO queue (${fifo_queue_cost:.4}) should cost more than Standard queue (${standard_queue_cost:.4}) at 5M requests"
    );

    // Verify the exact pricing difference: (5M - 1M) * ($0.0000005 - $0.0000004) = 4M * $0.0000001 = $0.40
    let expected_diff = 4_000_000.0 * (0.0000005 - 0.0000004);
    let actual_diff = fifo_queue_cost - standard_queue_cost;
    assert!(
        (actual_diff - expected_diff).abs() < 0.01,
        "FIFO vs Standard cost diff should be ~${expected_diff:.2}, got ${actual_diff:.2}"
    );
}

// ---------------------------------------------------------------------------
// 6. test_step_functions_standard_resource_count
// ---------------------------------------------------------------------------
#[test]
fn test_step_functions_standard_resource_count() {
    let resources = load_fixture("step-functions-workflow.yml");
    let arch = build_arch("step-functions-workflow", &resources, false);
    // OrderWorkflow, ValidateFunction, ProcessFunction, ValidateLogs, ProcessLogs
    assert_eq!(
        arch.resources.len(),
        5,
        "step-functions-workflow.yml should produce 5 costed resources, got {}",
        arch.resources.len()
    );
}

// ---------------------------------------------------------------------------
// 7. test_step_functions_standard_free_tier
// ---------------------------------------------------------------------------
#[test]
fn test_step_functions_standard_free_tier() {
    // Free tier: 4,000 transitions/month => cost = $0
    let resources = load_fixture("step-functions-workflow.yml");
    let arch = build_arch("step-functions-workflow", &resources, false);

    let params = p(&[
        ("OrderWorkflow_transitions", 3_000.0),
        ("ValidateFunction_requests", 0.0),
        ("ValidateFunction_avg_duration_ms", 0.0),
        ("ValidateFunction_data_transfer_out_gb", 0.0),
        ("ProcessFunction_requests", 0.0),
        ("ProcessFunction_avg_duration_ms", 0.0),
        ("ProcessFunction_data_transfer_out_gb", 0.0),
        ("ValidateLogs_ingestion_gb", 0.0),
        ("ValidateLogs_storage_gb", 0.0),
        ("ProcessLogs_ingestion_gb", 0.0),
        ("ProcessLogs_storage_gb", 0.0),
    ]);

    let result = evaluate_architecture(&arch, &params).unwrap();

    let sfn_cost = result
        .resources
        .iter()
        .find(|r| r.logical_id == "OrderWorkflow")
        .unwrap()
        .monthly_cost
        .value;

    assert!(
        sfn_cost.abs() < 0.001,
        "Step Functions Standard at 3000 transitions (under 4000 free) should cost $0, got ${sfn_cost:.4}"
    );
}

// ---------------------------------------------------------------------------
// 8. test_step_functions_standard_above_free_tier
// ---------------------------------------------------------------------------
#[test]
fn test_step_functions_standard_above_free_tier() {
    // 100,000 transitions: (100,000 - 4,000) * $0.000025 = 96,000 * $0.000025 = $2.40
    let resources = load_fixture("step-functions-workflow.yml");
    let arch = build_arch("step-functions-workflow", &resources, false);

    let params = p(&[
        ("OrderWorkflow_transitions", 100_000.0),
        ("ValidateFunction_requests", 0.0),
        ("ValidateFunction_avg_duration_ms", 0.0),
        ("ValidateFunction_data_transfer_out_gb", 0.0),
        ("ProcessFunction_requests", 0.0),
        ("ProcessFunction_avg_duration_ms", 0.0),
        ("ProcessFunction_data_transfer_out_gb", 0.0),
        ("ValidateLogs_ingestion_gb", 0.0),
        ("ValidateLogs_storage_gb", 0.0),
        ("ProcessLogs_ingestion_gb", 0.0),
        ("ProcessLogs_storage_gb", 0.0),
    ]);

    let result = evaluate_architecture(&arch, &params).unwrap();

    let sfn_cost = result
        .resources
        .iter()
        .find(|r| r.logical_id == "OrderWorkflow")
        .unwrap()
        .monthly_cost
        .value;

    let expected = (100_000.0 - 4_000.0) * 0.000025; // $2.40
    assert!(
        (sfn_cost - expected).abs() < 0.01,
        "Step Functions Standard at 100K transitions should cost ~${expected:.2}, got ${sfn_cost:.4}"
    );
}

// ---------------------------------------------------------------------------
// 9. test_express_workflow_scales_with_requests
// ---------------------------------------------------------------------------
#[test]
fn test_express_workflow_scales_with_requests() {
    let resources = load_fixture("express-workflow.yml");
    let arch = build_arch("express-workflow", &resources, false);

    let low_params = p(&[
        ("EventWorkflow_requests", 100_000.0),
        ("EventWorkflow_duration_gb_seconds", 100.0),
        ("HandlerFunction_requests", 0.0),
        ("HandlerFunction_avg_duration_ms", 0.0),
        ("HandlerFunction_data_transfer_out_gb", 0.0),
        ("HandlerLogs_ingestion_gb", 0.0),
        ("HandlerLogs_storage_gb", 0.0),
    ]);

    let high_params = p(&[
        ("EventWorkflow_requests", 10_000_000.0),
        ("EventWorkflow_duration_gb_seconds", 10_000.0),
        ("HandlerFunction_requests", 0.0),
        ("HandlerFunction_avg_duration_ms", 0.0),
        ("HandlerFunction_data_transfer_out_gb", 0.0),
        ("HandlerLogs_ingestion_gb", 0.0),
        ("HandlerLogs_storage_gb", 0.0),
    ]);

    let low_result = evaluate_architecture(&arch, &low_params).unwrap();
    let high_result = evaluate_architecture(&arch, &high_params).unwrap();

    assert!(
        high_result.naive_total() > low_result.naive_total(),
        "Express workflow at 10M requests (${:.4}) should cost more than 100K requests (${:.4})",
        high_result.naive_total(),
        low_result.naive_total()
    );
}

// ---------------------------------------------------------------------------
// 10. test_sns_free_tier_applies
// ---------------------------------------------------------------------------
#[test]
fn test_sns_free_tier_applies() {
    // SNS free tier: first 1,000,000 deliveries = $0
    let resources = load_fixture("sqs-fanout.yml");
    let arch = build_arch("sqs-fanout", &resources, false);

    let params = p(&[
        ("InputQueue_requests", 0.0),
        ("InputDLQ_requests", 0.0),
        ("ProcessorFunction_requests", 0.0),
        ("ProcessorFunction_avg_duration_ms", 0.0),
        ("ProcessorFunction_data_transfer_out_gb", 0.0),
        ("NotificationTopic_deliveries", 500_000.0),
        ("ProcessorLogs_ingestion_gb", 0.0),
        ("ProcessorLogs_storage_gb", 0.0),
    ]);

    let result = evaluate_architecture(&arch, &params).unwrap();

    let sns_cost = result
        .resources
        .iter()
        .find(|r| r.logical_id == "NotificationTopic")
        .unwrap()
        .monthly_cost
        .value;

    assert!(
        sns_cost.abs() < 0.001,
        "SNS at 500K deliveries (under 1M free tier) should cost $0, got ${sns_cost:.4}"
    );
}

// ---------------------------------------------------------------------------
// 11. test_sqs_fifo_with_async_api_total
// ---------------------------------------------------------------------------
#[test]
fn test_sqs_fifo_with_async_api_total() {
    // Realistic params for async-api.yml: total cost should be positive.
    let resources = load_fixture("async-api.yml");
    let arch = build_arch("async-api", &resources, false);

    let params = p(&[
        // 5M FIFO requests: (5M - 1M) * $0.0000005 = $2.00
        ("ApiQueue_requests", 5_000_000.0),
        // 2M Lambda invocations at 512MB, 100ms each
        ("WorkerFunction_requests", 2_000_000.0),
        ("WorkerFunction_avg_duration_ms", 100.0),
        ("WorkerFunction_data_transfer_out_gb", 1.0),
        // 3M SNS deliveries: (3M - 1M) * $0.0000005 = $1.00
        ("ResultTopic_deliveries", 3_000_000.0),
        // 10 GB logs ingested
        ("WorkerLogs_ingestion_gb", 10.0),
        ("WorkerLogs_storage_gb", 5.0),
    ]);

    let result = evaluate_architecture(&arch, &params).unwrap();

    assert!(
        result.naive_total() > 0.0,
        "async-api total cost should be positive with realistic params, got ${:.4}",
        result.naive_total()
    );
}

// ---------------------------------------------------------------------------
// 12. test_standard_vs_express_workflow_cost_comparison
// ---------------------------------------------------------------------------
#[test]
fn test_standard_vs_express_workflow_cost_comparison() {
    // At 1M invocations, compare cost structures of Standard vs Express.
    // Standard: (1M - 4K free) * $0.000025 = $24.90
    // Express: 1M * $0.000001 = $1.00 (requests only, ignoring duration for comparison)

    let std_resources = load_fixture("step-functions-workflow.yml");
    let std_arch = build_arch("step-functions-standard", &std_resources, false);

    let exp_resources = load_fixture("express-workflow.yml");
    let exp_arch = build_arch("express-workflow", &exp_resources, false);

    let std_params = p(&[
        ("OrderWorkflow_transitions", 1_000_000.0),
        ("ValidateFunction_requests", 0.0),
        ("ValidateFunction_avg_duration_ms", 0.0),
        ("ValidateFunction_data_transfer_out_gb", 0.0),
        ("ProcessFunction_requests", 0.0),
        ("ProcessFunction_avg_duration_ms", 0.0),
        ("ProcessFunction_data_transfer_out_gb", 0.0),
        ("ValidateLogs_ingestion_gb", 0.0),
        ("ValidateLogs_storage_gb", 0.0),
        ("ProcessLogs_ingestion_gb", 0.0),
        ("ProcessLogs_storage_gb", 0.0),
    ]);

    let exp_params = p(&[
        ("EventWorkflow_requests", 1_000_000.0),
        // Zero duration to isolate request cost
        ("EventWorkflow_duration_gb_seconds", 0.0),
        ("HandlerFunction_requests", 0.0),
        ("HandlerFunction_avg_duration_ms", 0.0),
        ("HandlerFunction_data_transfer_out_gb", 0.0),
        ("HandlerLogs_ingestion_gb", 0.0),
        ("HandlerLogs_storage_gb", 0.0),
    ]);

    let std_result = evaluate_architecture(&std_arch, &std_params).unwrap();
    let exp_result = evaluate_architecture(&exp_arch, &exp_params).unwrap();

    let std_sfn_cost = std_result
        .resources
        .iter()
        .find(|r| r.logical_id == "OrderWorkflow")
        .unwrap()
        .monthly_cost
        .value;

    let exp_sfn_cost = exp_result
        .resources
        .iter()
        .find(|r| r.logical_id == "EventWorkflow")
        .unwrap()
        .monthly_cost
        .value;

    // Standard at 1M transitions: (1M - 4K) * $0.000025 = $24.90
    let expected_standard = (1_000_000.0 - 4_000.0) * 0.000025;
    assert!(
        (std_sfn_cost - expected_standard).abs() < 0.01,
        "Standard workflow at 1M transitions should cost ~${expected_standard:.2}, got ${std_sfn_cost:.4}"
    );

    // Express at 1M requests: 1M * $0.000001 = $1.00
    let expected_express = 1_000_000.0 * 0.000001;
    assert!(
        (exp_sfn_cost - expected_express).abs() < 0.01,
        "Express workflow at 1M requests (0 duration) should cost ~${expected_express:.2}, got ${exp_sfn_cost:.4}"
    );

    // Standard should cost significantly more than Express at this volume
    assert!(
        std_sfn_cost > exp_sfn_cost,
        "Standard (${std_sfn_cost:.2}) should cost more than Express (${exp_sfn_cost:.2}) at 1M invocations"
    );
}
