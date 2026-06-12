//! Registration consistency tests for the GCP plugin.
//!
//! GCP is a TF-only provider (no CloudFormation support), so these tests
//! verify that the service catalog and TF adapter registry stay in sync.
//!
//! Design
//! ------
//! The matchable key is `service_id`. This file maintains a static
//! `EXPECTED_SERVICE_TO_TF` table mapping each service_id to its expected
//! Terraform resource type(s).

use std::collections::HashSet;

use yevice_service_api::{ServiceCatalog, TfAdapterRegistry};
use yevice_services_gcp::register;

// ---------------------------------------------------------------------------
// Static mapping: service_id → expected TF resource type(s)
// ---------------------------------------------------------------------------

/// Maps each GCP service_id to the Terraform resource type(s) whose adapters
/// produce it.
const EXPECTED_SERVICE_TO_TF: &[(&str, &[&str])] = &[
    (
        "gcp.bigquery",
        &["google_bigquery_dataset", "google_bigquery_table"],
    ),
    (
        "gcp.cloud_function",
        &[
            "google_cloudfunctions_function",
            "google_cloudfunctions2_function",
        ],
    ),
    (
        "gcp.cloud_run",
        &["google_cloud_run_service", "google_cloud_run_v2_service"],
    ),
    ("gcp.cloud_sql", &["google_sql_database_instance"]),
    ("gcp.cloud_storage", &["google_storage_bucket"]),
    ("gcp.pubsub", &["google_pubsub_topic"]),
];

// ---------------------------------------------------------------------------
// Minimum counts (test b)
// ---------------------------------------------------------------------------

/// Minimum number of GCP services that must be registered.
const MIN_SERVICE_COUNT: usize = 6;

/// Minimum number of TF resource types that must be registered.
const MIN_TF_TYPE_COUNT: usize = 9;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn build_registries() -> (ServiceCatalog, TfAdapterRegistry) {
    let mut catalog = ServiceCatalog::new();
    let mut tf = TfAdapterRegistry::new();
    register(&mut catalog, &mut tf);
    (catalog, tf)
}

// ---------------------------------------------------------------------------
// Test a: cross-reference service IDs ↔ TF types
// ---------------------------------------------------------------------------

/// Verify that every GCP service in the catalog has at least one TF adapter
/// and vice-versa, using `EXPECTED_SERVICE_TO_TF` as the ground truth.
#[test]
fn tf_and_service_registration_are_consistent() {
    let (catalog, tf) = build_registries();

    let registered_services: HashSet<&str> = catalog.registered_service_ids().into_iter().collect();
    let registered_tf_types: HashSet<&str> = tf.registered_types().into_iter().collect();

    let mut failures: Vec<String> = Vec::new();

    // 1. Every service_id in EXPECTED_SERVICE_TO_TF must be in the catalog.
    for &(service_id, _) in EXPECTED_SERVICE_TO_TF {
        if !registered_services.contains(service_id) {
            failures.push(format!(
                "Service '{service_id}' is listed in EXPECTED_SERVICE_TO_TF \
                 but not registered in ServiceCatalog"
            ));
        }
    }

    // 2. Every service in the catalog must be listed in EXPECTED_SERVICE_TO_TF.
    let expected_services: HashSet<&str> =
        EXPECTED_SERVICE_TO_TF.iter().map(|&(id, _)| id).collect();
    for id in &registered_services {
        if !expected_services.contains(*id) {
            failures.push(format!(
                "Service '{id}' is registered in ServiceCatalog \
                 but missing from EXPECTED_SERVICE_TO_TF — add it with its TF resource types"
            ));
        }
    }

    // 3. Every TF resource type in EXPECTED_SERVICE_TO_TF must be registered.
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

    // 4. Every TF type in the registry must be listed in EXPECTED_SERVICE_TO_TF.
    let expected_tf: HashSet<&str> = EXPECTED_SERVICE_TO_TF
        .iter()
        .flat_map(|&(_, types)| types.iter().copied())
        .collect();
    for rt in &registered_tf_types {
        if !expected_tf.contains(*rt) {
            failures.push(format!(
                "TF resource type '{rt}' is registered in TfAdapterRegistry \
                 but not listed in EXPECTED_SERVICE_TO_TF — add it to the mapping table"
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "GCP registration consistency failures ({} total):\n{}",
        failures.len(),
        failures
            .iter()
            .map(|s| format!("  - {s}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

// ---------------------------------------------------------------------------
// Test b: snapshot lower bounds
// ---------------------------------------------------------------------------

/// Verify that the number of registered GCP services and TF types never falls
/// below the expected minimums.
#[test]
fn registration_counts_meet_minimum_thresholds() {
    let (catalog, tf) = build_registries();

    let service_count = catalog.registered_service_ids().len();
    let tf_count = tf.registered_types().len();

    assert!(
        service_count >= MIN_SERVICE_COUNT,
        "Expected at least {MIN_SERVICE_COUNT} registered GCP services, got {service_count}"
    );
    assert!(
        tf_count >= MIN_TF_TYPE_COUNT,
        "Expected at least {MIN_TF_TYPE_COUNT} registered GCP TF types, got {tf_count}"
    );
}
