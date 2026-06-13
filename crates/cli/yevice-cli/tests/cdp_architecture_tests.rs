//! Regression tests pinning the `examples/cdp-*` architectures to the cost
//! figures published on https://aws.amazon.com/jp/cdp/.
//!
//! Each test parses the example CloudFormation template, builds a cost model
//! (optionally in list-price mode), and asserts the monthly total matches the
//! AWS CDP reference page within one cent.
//!
//!   cdp-lightsail-basic.yml        -> $3.43    (migrate-lightsail)
//!   cdp-analytics-report-basic.yml -> $40.25   (analytics-report-basic)
//!   cdp-ec-container.yml           -> $1236.69 (ec-container, list price)

use std::collections::HashMap;
use std::path::PathBuf;

use yevice_cfn::{convert, parser};
use yevice_core::cost::ArchitectureCost;
use yevice_core::evaluate::{Params, evaluate_architecture};
use yevice_core::types::VariableName;
use yevice_service_api::{CfnAdapterRegistry, ServiceCatalog, TfAdapterRegistry};
use yevice_services_aws::AwsPricingCatalog;

const REGION: &str = "ap-northeast-1";

fn p(pairs: &[(&str, f64)]) -> Params {
    pairs
        .iter()
        .map(|(k, v)| (VariableName::new(*k), *v))
        .collect()
}

/// Repo-root `examples/` directory (three levels up: crates/cli/yevice-cli -> repo root).
fn example_template(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../examples")
        .join(name)
}

/// Build a cost model from an `examples/` template, with optional list-price mode.
fn build_cdp(name: &str, template_file: &str, list_price: bool) -> ArchitectureCost {
    let path = example_template(template_file);
    let tmpl = parser::parse_template(path.as_ref()).expect("parse template");
    let resources =
        parser::resolve_template(&tmpl, &HashMap::new(), &HashMap::new()).expect("resolve");
    let tmpl = parser::CfnTemplate {
        parameters: HashMap::new(),
        mappings: HashMap::new(),
        conditions: HashMap::new(),
        resources,
    };
    let mut catalog = ServiceCatalog::new();
    let mut cfn = CfnAdapterRegistry::new();
    let mut tf = TfAdapterRegistry::new();
    yevice_services_aws::register(&mut catalog, &mut cfn, &mut tf);
    let arch = convert::build_architecture(name, REGION, &tmpl, &cfn);
    let pricing = AwsPricingCatalog::new(REGION).with_list_price(list_price);
    catalog.build_cost_model(&arch, &pricing, false).unwrap()
}

fn total(arch: &ArchitectureCost, params: &[(&str, f64)]) -> f64 {
    let result = evaluate_architecture(arch, &p(params)).expect("evaluate");
    result.naive_total()
}

// =========================================================================
// CDP: Website (migrate-lightsail) -> $3.43
// =========================================================================
#[test]
fn cdp_lightsail_matches_published_total() {
    // The Lightsail instance bundle already includes its root SSD, so the
    // instance carries no disk variable; the bundle price alone is the cost.
    let params: [(&str, f64); 0] = [];
    // Lightsail has no Free Tier, so both modes match.
    for list_price in [false, true] {
        let arch = build_cdp("cdp-lightsail", "cdp-lightsail-basic.yml", list_price);
        let got = total(&arch, &params);
        assert!(
            (got - 3.43).abs() < 0.01,
            "lightsail (list_price={list_price}) expected $3.43, got ${got:.2}"
        );
    }
}

// =========================================================================
// CDP: Analytics report (analytics-report-basic) -> $40.25
// =========================================================================
#[test]
fn cdp_analytics_matches_published_total() {
    let params = [
        ("DataBucket_storage_gb", 10.0),
        ("DataBucket_put_requests", 300.0),
        ("DataBucket_get_requests", 300.0),
        // QuickSight account cost is billed once on the Yevice::QuickSight
        // marker; the Analysis/Dashboard are structural. 4 viewers x ~13.333
        // sessions = $16.00.
        ("QuickSightAccount_creators", 1.0),
        ("QuickSightAccount_viewer_users", 4.0),
        ("QuickSightAccount_sessions_per_user", 13.333333),
        ("QuickSightAccount_spice_gb", 10.0),
    ];
    // The 10GB SPICE allocation is product-included (not Free Tier), so the
    // total is $40.25 in both normal and list-price modes.
    for list_price in [false, true] {
        let arch = build_cdp(
            "cdp-analytics",
            "cdp-analytics-report-basic.yml",
            list_price,
        );
        let got = total(&arch, &params);
        assert!(
            (got - 40.25).abs() < 0.01,
            "analytics (list_price={list_price}) expected $40.25, got ${got:.2}"
        );
    }
}

// =========================================================================
// CDP: Analytics report 基本編 (analytics-report-advanced) -> $73.85
// =========================================================================
#[test]
fn cdp_analytics_advanced_matches_published_total() {
    let params = [
        ("DataBucket_storage_gb", 30.0),
        ("DataBucket_put_requests", 900.0),
        ("DataBucket_get_requests", 900.0),
        ("AthenaWorkGroup_scan_gb", 300.0),
        ("QuickSightAccount_creators", 1.0),
        ("QuickSightAccount_viewer_users", 10.0),
        ("QuickSightAccount_sessions_per_user", 13.333333),
        ("QuickSightAccount_spice_gb", 30.0),
    ];
    let arch = build_cdp("cdp-analytics-adv", "cdp-analytics-advanced.yml", false);
    let got = total(&arch, &params);
    assert!(
        (got - 73.85).abs() < 0.01,
        "analytics-advanced expected $73.85, got ${got:.2}"
    );
}

// =========================================================================
// CDP: Analytics report 応用編 (analytics-report-master) -> $518.93
// =========================================================================
#[test]
fn cdp_analytics_master_matches_published_total() {
    let params = [
        ("DataLakeBucket_storage_gb", 3072.0),
        ("DataLakeBucket_put_requests", 44681.0),
        ("DataLakeBucket_get_requests", 45000.0),
        ("Warehouse_storage_gb", 1024.0),
        ("Warehouse_spectrum_tb", 3.0),
        ("QuickSightAccount_creators", 1.0),
        ("QuickSightAccount_viewer_users", 30.0),
        ("QuickSightAccount_sessions_per_user", 15.833333),
        ("QuickSightAccount_spice_gb", 30.0),
    ];
    let arch = build_cdp("cdp-analytics-master", "cdp-analytics-master.yml", false);
    let got = total(&arch, &params);
    assert!(
        (got - 518.93).abs() < 0.01,
        "analytics-master expected $518.93, got ${got:.2}"
    );
}

// =========================================================================
// CDP: Container web service (ec-container) -> $1236.69 (list price)
// =========================================================================
#[test]
fn cdp_ec_container_matches_published_total_list_price() {
    let params = [
        ("AppRegistry_storage_gb", 2.0),
        ("FargateService_vcpu", 2.0),
        ("FargateService_memory_gb", 8.0),
        ("FargateService_data_transfer_out_gb", 0.0),
        ("FargateLogs_ingestion_gb", 30.0),
        ("FargateLogs_storage_gb", 30.0),
        ("EcsCluster_custom_metrics", 19.0),
        ("EcsCluster_log_gb", 0.225),
        ("Alb_lcu", 0.5),
        ("CloudFrontDistribution_data_transfer_gb", 1024.0),
        ("CloudFrontDistribution_http_requests", 10_000_000.0),
        ("NatGateway_data_processed_gb", 0.0),
        ("NatGateway2_data_processed_gb", 0.0),
    ];
    let arch = build_cdp("cdp-ec", "cdp-ec-container.yml", true);
    let got = total(&arch, &params);
    assert!(
        (got - 1236.69).abs() < 0.01,
        "ec-container (list price) expected $1236.69, got ${got:.2}"
    );
}

// =========================================================================
// CDP: Windows 業務アプリ移行 入門編 (windows-bizapp-basic) -> $1027.68
// EC2(Windows) + EBS gp2/snapshot + ALB + RDS for SQL Server + VPN.
// =========================================================================
#[test]
fn cdp_windows_bizapp_basic_matches_published_total_list_price() {
    let params = [
        ("AppServer_data_transfer_out_gb", 0.0),
        ("AppVolume_snapshot_gb", 200.0),
        ("Alb_lcu", 0.5),
    ];
    let arch = build_cdp("cdp-wbb", "cdp-windows-bizapp-basic.yml", true);
    let got = total(&arch, &params);
    assert!(
        (got - 1027.68).abs() < 0.01,
        "windows-bizapp-basic (list price) expected $1027.68, got ${got:.2}"
    );
}

// =========================================================================
// CDP: Windows 業務アプリ移行 基本編 (windows-bizapp-migration) -> $2034.70
// 2x EC2(Windows) + 2x EBS + ALB + RDS SQL Server Multi-AZ + 2x VPN.
// Validates Multi-AZ RDS storage doubling for SQL Server.
// =========================================================================
#[test]
fn cdp_windows_bizapp_migration_matches_published_total_list_price() {
    let params = [
        ("AppServer1_data_transfer_out_gb", 0.0),
        ("AppServer2_data_transfer_out_gb", 0.0),
        ("AppVolume1_snapshot_gb", 200.0),
        ("AppVolume2_snapshot_gb", 200.0),
        ("Alb_lcu", 0.5),
    ];
    let arch = build_cdp("cdp-wbm", "cdp-windows-bizapp-migration.yml", true);
    let got = total(&arch, &params);
    assert!(
        (got - 2034.70).abs() < 0.01,
        "windows-bizapp-migration (list price) expected $2034.70, got ${got:.2}"
    );
}

// =========================================================================
// CDP: Windows 業務アプリ移行 応用編 (windows-bizapp-master) -> $2059.63
// 基本編 + WAF + GuardDuty + CloudTrail + CloudWatch Logs/Alarms.
// Residual <$0.02 is CloudWatch Logs cent rounding.
// =========================================================================
#[test]
fn cdp_windows_bizapp_master_matches_published_total_list_price() {
    let params = [
        ("AppServer1_data_transfer_out_gb", 0.0),
        ("AppServer2_data_transfer_out_gb", 0.0),
        ("AppVolume1_snapshot_gb", 200.0),
        ("AppVolume2_snapshot_gb", 200.0),
        ("Alb_lcu", 0.5),
        ("Trail_data_events_100k", 0.0),
        ("Trail_management_event_copies_100k", 0.0),
        ("WebAcl_rule_count", 4.0),
        ("WebAcl_requests", 1_000_000.0),
        ("Detector_cloudtrail_events_millions", 2.0),
        ("Detector_flowlog_gb", 2.0),
        ("LogGroup_ingestion_gb", 2.0),
        ("LogGroup_storage_gb", 0.0),
        // Alarms: count fixed in the template (AlarmCount: 20), no usage input.
    ];
    let arch = build_cdp("cdp-wbmm", "cdp-windows-bizapp-master.yml", true);
    let got = total(&arch, &params);
    assert!(
        (got - 2059.63).abs() < 0.02,
        "windows-bizapp-master (list price) expected ~$2059.63, got ${got:.2}"
    );
}

// =========================================================================
// CDP: Windows ファイルサーバー 基本編 (fileserver-fsx) -> $1048.87
// FSx for Windows + Managed AD + VPN + data transfer. Exact under --list-price
// (the data-transfer first-GB free tier is stripped in list-price mode).
// =========================================================================
#[test]
fn cdp_fileserver_fsx_matches_published_total_list_price() {
    let params = [
        ("FileSystem_storage_capacity_gb", 2048.0),
        ("FileSystem_throughput_capacity_mbps", 32.0),
        ("FileSystem_backup_gb", 3072.0),
        ("ManagedAd_domain_controllers", 2.0),
        ("DataTransfer_internet_egress_gb", 200.0),
        ("DataTransfer_inter_region_gb", 0.0),
    ];
    let arch = build_cdp("cdp-fsx", "cdp-fileserver-fsx.yml", true);
    let got = total(&arch, &params);
    assert!(
        (got - 1048.87).abs() < 0.01,
        "fileserver-fsx (list price) expected $1048.87, got ${got:.2}"
    );
}

// =========================================================================
// list-price mode raises the cost of Free-Tier-eligible services
// =========================================================================
#[test]
fn cdp_ec_container_list_price_exceeds_normal() {
    let params = [
        ("AppRegistry_storage_gb", 2.0),
        ("FargateService_vcpu", 2.0),
        ("FargateService_memory_gb", 8.0),
        ("FargateService_data_transfer_out_gb", 0.0),
        ("FargateLogs_ingestion_gb", 30.0),
        ("FargateLogs_storage_gb", 30.0),
        ("EcsCluster_custom_metrics", 19.0),
        ("EcsCluster_log_gb", 0.225),
        ("Alb_lcu", 0.5),
        ("CloudFrontDistribution_data_transfer_gb", 1024.0),
        ("CloudFrontDistribution_http_requests", 10_000_000.0),
        ("NatGateway_data_processed_gb", 0.0),
        ("NatGateway2_data_processed_gb", 0.0),
    ];
    let normal = total(&build_cdp("cdp-ec", "cdp-ec-container.yml", false), &params);
    let listed = total(&build_cdp("cdp-ec", "cdp-ec-container.yml", true), &params);
    // CloudWatch Logs (5GB) and CloudFront (1TB) Free Tier are removed in
    // list-price mode, so the listed total is strictly higher.
    assert!(
        listed > normal,
        "list-price total (${listed:.2}) should exceed normal (${normal:.2})"
    );
}
