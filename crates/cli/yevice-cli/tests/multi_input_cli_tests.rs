use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use yevice_core::cost::ArchitectureCost;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn make_temp_dir(label: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("yevice-cli-{label}-{unique}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn run_yevice(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_yevice"))
        .args(args)
        .output()
        .unwrap()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn read_cost_model(path: &Path) -> ArchitectureCost {
    let content = fs::read_to_string(path).unwrap();
    serde_json::from_str(&content).unwrap()
}

#[test]
fn generate_supports_gcp_terraform_directory_input() {
    let fixture = fixtures_dir().join("gcp-analytics-tf");
    let temp_dir = make_temp_dir("gcp-generate");
    let output_path = temp_dir.join("gcp-cost-model.json");

    let output = run_yevice(&[
        "--region",
        "asia-northeast1",
        "generate",
        "--template",
        fixture.to_str().unwrap(),
        "--name",
        "gcp-analytics",
        "--output",
        output_path.to_str().unwrap(),
    ]);

    assert_success(&output);

    let cost_model = read_cost_model(&output_path);
    assert_eq!(cost_model.name.as_str(), "gcp-analytics");
    assert_eq!(cost_model.region.as_str(), "asia-northeast1");
    assert!(
        cost_model
            .resources
            .iter()
            .any(|resource| resource.resource_type == "google_storage_bucket")
    );
    assert!(
        cost_model
            .resources
            .iter()
            .any(|resource| resource.resource_type == "google_pubsub_topic")
    );

    fs::remove_dir_all(temp_dir).unwrap();
}

#[test]
fn validate_supports_tfvars_input() {
    let tfvars_path = fixtures_dir()
        .join("gcp-analytics-tf")
        .join("terraform.tfvars");
    let usage_path = fixtures_dir().join("usage.yaml");

    let output = run_yevice(&[
        "--region",
        "asia-northeast1",
        "validate",
        "--template",
        tfvars_path.to_str().unwrap(),
        "--params",
        usage_path.to_str().unwrap(),
    ]);

    assert_success(&output);
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("All capacity constraints satisfied.")
    );
}

#[test]
fn generate_supports_wrangler_directory_input() {
    let fixture = fixtures_dir().join("wrangler-basic");
    let temp_dir = make_temp_dir("wrangler-generate");
    let output_path = temp_dir.join("wrangler-cost-model.json");

    let output = run_yevice(&[
        "generate",
        "--template",
        fixture.to_str().unwrap(),
        "--output",
        output_path.to_str().unwrap(),
    ]);

    assert_success(&output);

    let cost_model = read_cost_model(&output_path);
    assert_eq!(cost_model.name.as_str(), "edge-worker");
    assert_eq!(cost_model.region.as_str(), "global");
    assert!(
        cost_model
            .resources
            .iter()
            .any(|resource| resource.resource_type == "cloudflare_worker")
    );
    assert!(
        cost_model
            .resources
            .iter()
            .any(|resource| resource.resource_type == "cloudflare_workers_kv_namespace")
    );
    assert!(
        cost_model
            .resources
            .iter()
            .any(|resource| resource.resource_type == "cloudflare_r2_bucket")
    );

    fs::remove_dir_all(temp_dir).unwrap();
}

#[test]
fn validate_supports_wrangler_directory_input() {
    let fixture = fixtures_dir().join("wrangler-basic");
    let usage_path = fixtures_dir().join("usage.yaml");

    let output = run_yevice(&[
        "validate",
        "--template",
        fixture.to_str().unwrap(),
        "--params",
        usage_path.to_str().unwrap(),
    ]);

    assert_success(&output);
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("All capacity constraints satisfied.")
    );
}

// -----------------------------------------------------------------------
// ADR-0003 / Issue #38 — ParsePolicy CLI surface
// -----------------------------------------------------------------------

const CFN_TEMPLATE_REQUIRES_PARAM: &str = r#"
AWSTemplateFormatVersion: "2010-09-09"
Parameters:
  TableName:
    Type: String
Resources:
  Table:
    Type: AWS::DynamoDB::Table
    Properties:
      TableName: !Ref TableName
      BillingMode: PAY_PER_REQUEST
      AttributeDefinitions:
        - AttributeName: pk
          AttributeType: S
      KeySchema:
        - AttributeName: pk
          KeyType: HASH
"#;

/// Default (Lenient) generate of a CFN template missing a required parameter
/// must succeed AND record a `missing_parameter` diagnostic in the emitted
/// cost_model.json.
#[test]
fn lenient_default_emits_diagnostic_in_cost_model() {
    let temp_dir = make_temp_dir("policy-lenient");
    let template_path = temp_dir.join("template.yaml");
    fs::write(&template_path, CFN_TEMPLATE_REQUIRES_PARAM).unwrap();
    let output_path = temp_dir.join("model.json");

    let output = run_yevice(&[
        "generate",
        "--template",
        template_path.to_str().unwrap(),
        "--output",
        output_path.to_str().unwrap(),
    ]);
    assert_success(&output);

    let cost_model = read_cost_model(&output_path);
    assert!(
        cost_model
            .diagnostics
            .iter()
            .any(|d| d.code == "missing_parameter"),
        "lenient generate must record missing_parameter diagnostic in cost_model.json; got {:?}",
        cost_model.diagnostics
    );

    fs::remove_dir_all(temp_dir).unwrap();
}

/// The same template under top-level `--strict` must exit non-zero.
#[test]
fn strict_flag_exits_nonzero_on_missing_parameter() {
    let temp_dir = make_temp_dir("policy-strict");
    let template_path = temp_dir.join("template.yaml");
    fs::write(&template_path, CFN_TEMPLATE_REQUIRES_PARAM).unwrap();
    let output_path = temp_dir.join("model.json");

    let output = run_yevice(&[
        "--strict",
        "generate",
        "--template",
        template_path.to_str().unwrap(),
        "--output",
        output_path.to_str().unwrap(),
    ]);
    assert!(
        !output.status.success(),
        "expected non-zero exit under --strict; stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    fs::remove_dir_all(temp_dir).unwrap();
}
