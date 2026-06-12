//! Shared helpers for integration tests.
//!
//! Included via `mod common;` by several integration-test binaries; not every
//! binary uses every helper, so suppress dead-code warnings for the unused ones.
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use yevice_cfn::{convert, parser};
use yevice_core::cost::ArchitectureCost;
use yevice_service_api::{CfnAdapterRegistry, ServiceCatalog, TfAdapterRegistry};
use yevice_services_aws::AwsPricingCatalog;

pub use yevice_core::evaluate::Params;
pub use yevice_core::types::VariableName;

pub const REGION: &str = "ap-northeast-1";

pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

pub fn load_fixture(name: &str) -> BTreeMap<String, parser::ResolvedResource> {
    let path = fixtures_dir().join(name);
    let tmpl = parser::parse_template(path.as_ref()).unwrap();
    parser::resolve_template(&tmpl, &HashMap::new(), &HashMap::new()).unwrap()
}

pub fn build_arch(
    name: &str,
    resources: &BTreeMap<String, parser::ResolvedResource>,
    strict: bool,
) -> ArchitectureCost {
    let tmpl = parser::CfnTemplate {
        parameters: HashMap::new(),
        mappings: HashMap::new(),
        conditions: HashMap::new(),
        resources: resources.clone(),
    };
    let mut catalog = ServiceCatalog::new();
    let mut cfn = CfnAdapterRegistry::new();
    let mut tf = TfAdapterRegistry::new();
    yevice_services_aws::register(&mut catalog, &mut cfn, &mut tf);
    let arch = convert::build_architecture(name, REGION, &tmpl, &cfn);
    let pricing = AwsPricingCatalog::new(REGION);
    catalog.build_cost_model(&arch, &pricing, strict).unwrap()
}

// Not every test file uses this helper; suppress dead-code warnings on the
// integration test binaries that import the common module but never call `p`.
#[allow(dead_code)]
pub fn p(pairs: &[(&str, f64)]) -> Params {
    pairs
        .iter()
        .map(|(k, v)| (VariableName::new(*k), *v))
        .collect()
}
