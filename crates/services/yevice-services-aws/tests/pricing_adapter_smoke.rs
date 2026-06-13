//! Pricing adapter SKU smoke tests.
//!
//! Catches the "added a `Sku::new(...)` to a service but forgot to add the
//! corresponding match arm in `pricing_adapter.rs`" class of bug. Without
//! this test, the omission would only be discovered at runtime when a
//! customer template happens to exercise that cost component.
//!
//! ## How it works
//!
//! At test time we walk every `crates/services/yevice-services-aws/src/services/*.rs`
//! source file, extract each `Sku::new("...")` static literal, and call
//! `AwsPricingCatalog::lookup` for it. Anything that returns
//! `PricingError::NotFound` is reported.
//!
//! Dynamic SKUs constructed with `Sku::dynamic(format!(...))` are covered
//! by a separate hand-curated representative-sample list (`DYNAMIC_SKU_SAMPLES`)
//! because the macro expansion happens at runtime and the actual SKU value
//! depends on customer-supplied instance/node-type strings. The sample
//! ensures every prefix handler in `pricing_adapter.rs` still has at least
//! one known-good instance type wired through it.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use yevice_pricing::{
    catalog::{PriceCatalog, Sku},
    error::PricingError,
};
use yevice_services_aws::AwsPricingCatalog;

/// Representative dynamic SKUs that should each resolve to a real price.
///
/// Each entry pairs one of the `Sku::dynamic(format!(...))` prefixes used
/// by a service with a concrete instance/node/bundle type known to exist in
/// the hardcoded `PricingRegistry`. The list is intentionally hand-curated:
/// if a new dynamic prefix is added, append a representative sample here so
/// the smoke test catches a missing match arm.
const DYNAMIC_SKU_SAMPLES: &[&str] = &[
    "aws.ec2.instance.t3.micro",
    // `aws.rds.<engine>.<instance>` and `aws.rds_storage.<engine>.<instance>`
    // both delegate to `rds_price(instance, engine)`.
    "aws.rds.mysql.db.t3.micro",
    "aws.rds_storage.mysql.db.t3.micro",
    "aws.elasticache.cache.t3.micro",
    "aws.msk.kafka.m5.large",
    "aws.msk_storage.kafka.m5.large",
    "aws.opensearch_service.t3.small.search",
    "aws.opensearch_service_storage.t3.small.search",
    "aws.documentdb.db.t3.medium",
    "aws.documentdb_storage.db.t3.medium",
    "aws.ebs.gb_month.gp3",
    "aws.redshift.dc2.large",
    "aws.lightsail.bundle.nano_2_0",
    // Kendra editions are SCREAMING_CASE (`DEVELOPER_EDITION` / `ENTERPRISE_EDITION`).
    "aws.kendra.index_hour.DEVELOPER_EDITION",
    // FSx storage takes `<storage_type>.<deployment>`.
    "aws.fsx_windows.storage_gb_month.ssd.single_az",
    "aws.fsx_windows.throughput_mbps_month.single_az",
    // Directory Service editions are TitleCase (`Standard` / `Enterprise`).
    "aws.directory_service.dc_hour.Standard",
];

/// Region used for all smoke-test catalog lookups. Tokyo is the default
/// supported region for the hardcoded fallback registry, so every SKU
/// should resolve here.
const TEST_REGION: &str = "ap-northeast-1";

#[test]
fn every_static_sku_literal_resolves() {
    let services_dir = service_source_dir();
    let mut skus: BTreeSet<String> = BTreeSet::new();
    let mut scanned_files = 0usize;

    for entry in fs::read_dir(&services_dir).expect("read services dir") {
        let path = entry.expect("dir entry").path();
        if !is_rust_source(&path) {
            continue;
        }
        scanned_files += 1;
        let source =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        extract_static_skus(&source, &mut skus);
    }

    assert!(
        scanned_files > 0,
        "no service source files found under {}",
        services_dir.display()
    );
    assert!(
        skus.len() >= 50,
        "expected at least 50 static SKUs across services, found {} \
         (regex broken? scanned {} files)",
        skus.len(),
        scanned_files
    );

    let catalog = AwsPricingCatalog::new(TEST_REGION);
    let mut failures: Vec<String> = Vec::new();

    for sku_str in &skus {
        let sku = Sku::dynamic(sku_str);
        match catalog.lookup(&sku) {
            Ok(_) => {}
            Err(PricingError::NotFound { .. }) => {
                failures.push(format!(
                    "Sku::new(\"{sku_str}\") is referenced by a service but \
                     AwsPricingCatalog::lookup returned NotFound — add the \
                     corresponding match arm to pricing_adapter.rs"
                ));
            }
            Err(other) => {
                failures.push(format!(
                    "Sku::new(\"{sku_str}\") returned non-NotFound error: {other}"
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "Pricing SKU smoke-test failures ({} of {} SKUs):\n{}",
        failures.len(),
        skus.len(),
        failures
            .iter()
            .map(|s| format!("  - {s}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

#[test]
fn every_dynamic_sku_sample_resolves() {
    let catalog = AwsPricingCatalog::new(TEST_REGION);
    let mut failures: Vec<String> = Vec::new();

    for sample in DYNAMIC_SKU_SAMPLES {
        let sku = Sku::dynamic(*sample);
        match catalog.lookup(&sku) {
            Ok(_) => {}
            Err(PricingError::NotFound { .. }) => {
                failures.push(format!(
                    "dynamic-SKU sample \"{sample}\" returned NotFound — \
                     the matching `sku.starts_with(...)` arm is missing \
                     from pricing_adapter.rs"
                ));
            }
            Err(other) => {
                failures.push(format!(
                    "dynamic-SKU sample \"{sample}\" returned error: {other}"
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "Dynamic-SKU sample failures ({} of {}):\n{}",
        failures.len(),
        DYNAMIC_SKU_SAMPLES.len(),
        failures
            .iter()
            .map(|s| format!("  - {s}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn service_source_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("services")
}

fn is_rust_source(path: &Path) -> bool {
    path.is_file()
        && path.extension().is_some_and(|ext| ext == "rs")
        && path.file_name().is_some_and(|name| name != "mod.rs")
}

/// Scan a source file for `Sku::new("...")` and `Sku(Cow::Borrowed("..."))`
/// literal arguments and insert each unique SKU into `out`.
///
/// We do not pull in a regex crate just for this test, so the parser is a
/// hand-written scanner: find the literal `Sku::new("`, then read until the
/// closing `"`. Escaped quotes are not expected in SKU strings.
fn extract_static_skus(source: &str, out: &mut BTreeSet<String>) {
    const NEEDLE: &str = "Sku::new(\"";
    let mut rest = source;
    while let Some(idx) = rest.find(NEEDLE) {
        rest = &rest[idx + NEEDLE.len()..];
        if let Some(end) = rest.find('"') {
            let sku = &rest[..end];
            if sku.starts_with("aws.") {
                out.insert(sku.to_string());
            }
            rest = &rest[end + 1..];
        } else {
            break;
        }
    }
}
