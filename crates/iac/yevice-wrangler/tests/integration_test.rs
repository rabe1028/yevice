//! Integration tests for the wrangler.toml parser and Cloudflare service plugins.

use std::path::PathBuf;

use yevice_pricing::{PriceCatalog, PriceRecord, Sku, error::PricingError};
use yevice_service_api::ServiceCatalog;
use yevice_wrangler::{parser, services::CloudflareWorkerSpec};

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

/// `Cloudflare` services compute prices inline (the catalog is unused), so the
/// test catalog only needs to satisfy the trait.
struct EmptyCatalog;

impl PriceCatalog for EmptyCatalog {
    fn region(&self) -> &'static str {
        "global"
    }

    fn lookup(&self, sku: &Sku) -> Result<PriceRecord, PricingError> {
        Err(PricingError::NotFound {
            service: sku.as_str().to_string(),
            region: "global".to_string(),
        })
    }
}

fn cf_catalog() -> ServiceCatalog {
    let mut catalog = ServiceCatalog::new();
    yevice_wrangler::register(&mut catalog);
    catalog
}

#[test]
fn parses_simple_worker_only() {
    let path = fixtures_dir().join("wrangler_simple.toml");
    let arch = parser::parse_wrangler(&path).unwrap();

    assert_eq!(arch.name, "simple-worker");
    assert_eq!(arch.region.as_str(), "global");
    assert_eq!(arch.resources.len(), 1);
    assert_eq!(arch.resources[0].shell.service_id, "cloudflare.worker");
}

#[test]
fn parses_full_wrangler_with_all_resource_kinds() {
    let path = fixtures_dir().join("wrangler_full.toml");
    let arch = parser::parse_wrangler(&path).unwrap();

    assert_eq!(arch.name, "my-app");

    let count_by_type = |kind: &str| {
        arch.resources
            .iter()
            .filter(|r| r.resource_type.as_str() == kind)
            .count()
    };

    assert_eq!(count_by_type("cloudflare_worker"), 1);
    assert_eq!(count_by_type("cloudflare_workers_kv_namespace"), 2); // SESSIONS + CACHE
    assert_eq!(count_by_type("cloudflare_r2_bucket"), 1);
    assert_eq!(count_by_type("cloudflare_d1_database"), 1);
    assert_eq!(count_by_type("cloudflare_queue"), 1); // deduped: job-queue
    assert_eq!(count_by_type("cloudflare_durable_object"), 2); // ChatRoom + UserSession
}

#[test]
fn worker_usage_model_decodes_as_standard() {
    let path = fixtures_dir().join("wrangler_full.toml");
    let arch = parser::parse_wrangler(&path).unwrap();

    let worker = arch
        .resources
        .iter()
        .find(|r| r.shell.service_id == "cloudflare.worker")
        .expect("worker resource present");

    let spec: CloudflareWorkerSpec = worker.shell.decode().unwrap();
    assert!(matches!(
        spec.usage_model,
        yevice_wrangler::services::CloudflareUsageModel::Standard
    ));
}

#[test]
fn build_cost_model_yields_one_cost_per_resource() {
    let path = fixtures_dir().join("wrangler_full.toml");
    let arch = parser::parse_wrangler(&path).unwrap();
    let catalog = cf_catalog();
    let pricing = EmptyCatalog;

    let cost_model = catalog.build_cost_model(&arch, &pricing, true).unwrap();

    // 1 Worker + 2 KV + 1 R2 + 1 D1 + 1 Queue + 2 DO = 8 resources
    assert_eq!(cost_model.resources.len(), 8);

    let labels: Vec<&str> = cost_model
        .resources
        .iter()
        .map(|c| c.label.as_str())
        .collect();
    assert!(labels.iter().any(|l| l.contains("Workers")));
    assert!(labels.iter().any(|l| l.contains("R2")));
    assert!(labels.iter().any(|l| l.contains("D1")));
}

#[test]
fn worker_cost_declares_required_variables() {
    let path = fixtures_dir().join("wrangler_simple.toml");
    let arch = parser::parse_wrangler(&path).unwrap();
    let catalog = cf_catalog();
    let pricing = EmptyCatalog;

    let cost_model = catalog.build_cost_model(&arch, &pricing, true).unwrap();
    let worker_cost = &cost_model.resources[0];

    let var_names: Vec<&str> = worker_cost
        .required_variables
        .iter()
        .map(|v| v.name.as_str())
        .collect();

    assert!(var_names.iter().any(|n| n.contains("monthly_requests")));
    assert!(var_names.iter().any(|n| n.contains("avg_cpu_ms")));
}

#[test]
fn queue_producer_and_consumer_dedupe_to_single_resource() {
    let content = r#"
name = "test"
main = "src/index.ts"
compatibility_date = "2024-01-01"

[queues]

[[queues.producers]]
binding = "P"
queue = "my-queue"

[[queues.consumers]]
queue = "my-queue"
"#;
    let arch = parser::parse_wrangler_str(content, "test").unwrap();
    let queues: Vec<_> = arch
        .resources
        .iter()
        .filter(|r| r.resource_type.as_str() == "cloudflare_queue")
        .collect();
    assert_eq!(queues.len(), 1);
}
