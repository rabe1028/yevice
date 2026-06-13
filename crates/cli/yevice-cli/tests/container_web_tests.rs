//! Integration tests for ECS Fargate / ALB / RDS / ElastiCache patterns.
//!
//! Fixtures:
//!   fargate-web-app.yml      — WebService(2 tasks) + ALB + RDS(t3.medium) + ElastiCache
//!   fargate-production.yml   — ApiService(4) + BackendService(2) + ALB + RDS(r5.large MultiAZ) + Redis + NAT GW
//!   fargate-dev.yml          — DevService(1) + ALB + RDS(t3.micro)

mod common;
use common::{Params, build_arch, load_fixture, p};

use yevice_core::evaluate::evaluate_architecture;

// Pricing constants (ap-northeast-1)
const VCPU_HOUR: f64 = 0.05056;
const HOURS: f64 = 730.0;
const RDS_T3_MEDIUM_HOURLY: f64 = 0.104;
const RDS_R5_LARGE_HOURLY: f64 = 0.290;
const RDS_GP2_STORAGE_PER_GB: f64 = 0.138;
const CACHE_T3_MEDIUM_HOURLY: f64 = 0.084;
const ALB_HOUR: f64 = 0.0243;
const NAT_HOUR: f64 = 0.062;

// =========================================================================
// Test 1: fargate-web-app.yml has exactly 4 resources
// =========================================================================
#[test]
fn test_fargate_web_app_resource_count() {
    let resources = load_fixture("fargate-web-app.yml");
    let arch = build_arch("fargate-web-app", &resources, false);

    // WebService, AppLoadBalancer, AppDatabase, SessionCache
    assert_eq!(
        arch.resources.len(),
        4,
        "expected 4 resources, got {}: {:?}",
        arch.resources.len(),
        arch.resources.iter().map(|r| &r.label).collect::<Vec<_>>()
    );
}

// =========================================================================
// Test 2: ECS Fargate resources have correct label format
// =========================================================================
#[test]
fn test_fargate_labels() {
    let resources = load_fixture("fargate-web-app.yml");
    let arch = build_arch("fargate-web-app", &resources, false);

    let fargate_resources: Vec<_> = arch
        .resources
        .iter()
        .filter(|r| r.label.starts_with("ECS Fargate:"))
        .collect();

    assert_eq!(
        fargate_resources.len(),
        1,
        "should have 1 ECS Fargate resource"
    );
    assert_eq!(
        fargate_resources[0].label, "ECS Fargate: WebService",
        "label should be 'ECS Fargate: WebService'"
    );
}

// =========================================================================
// Test 3: ElastiCache has no required variables (constant cost)
// =========================================================================
#[test]
fn test_elasticache_has_no_required_variables() {
    let resources = load_fixture("fargate-web-app.yml");
    let arch = build_arch("fargate-web-app", &resources, false);

    let cache = arch
        .resources
        .iter()
        .find(|r| r.label.starts_with("ElastiCache:"))
        .expect("should have ElastiCache resource");

    assert_eq!(
        cache.required_variables.len(),
        0,
        "ElastiCache should have no required variables (cost is constant)"
    );
}

// =========================================================================
// Test 4: ElastiCache constant cost evaluates to > 0 with empty params
// =========================================================================
#[test]
fn test_elasticache_constant_cost() {
    let resources = load_fixture("fargate-web-app.yml");
    let arch = build_arch("fargate-web-app", &resources, false);

    let cache = arch
        .resources
        .iter()
        .find(|r| r.label.starts_with("ElastiCache:"))
        .expect("should have ElastiCache resource");

    // cost = 0.084 * 730 * 2 nodes = $122.64/month
    let expected = CACHE_T3_MEDIUM_HOURLY * HOURS * 2.0;
    let expr_result = yevice_core::evaluate::evaluate(&cache.expr, &Params::default()).unwrap();

    assert!(
        expr_result > 0.0,
        "ElastiCache cost should be positive with no params"
    );
    assert!(
        (expr_result - expected).abs() < 0.01,
        "cache.t3.medium x2 cost should be ~${expected:.2}, got ${expr_result:.2}"
    );
}

// =========================================================================
// Test 5: RDS cost includes instance + storage (both constant for gp2)
// =========================================================================
#[test]
fn test_rds_cost_includes_instance_and_storage() {
    let resources = load_fixture("fargate-web-app.yml");
    let arch = build_arch("fargate-web-app", &resources, false);

    let rds = arch
        .resources
        .iter()
        .find(|r| r.label.starts_with("RDS:"))
        .expect("should have RDS resource");

    // db.t3.medium MySQL, 100 GB gp2, single AZ
    let instance_cost = RDS_T3_MEDIUM_HOURLY * HOURS;
    let storage_cost = RDS_GP2_STORAGE_PER_GB * 100.0;
    let expected_total = instance_cost + storage_cost;

    let actual = yevice_core::evaluate::evaluate(&rds.expr, &Params::default()).unwrap();

    assert!(
        actual > instance_cost,
        "total RDS cost (${actual:.2}) should exceed instance-only cost (${instance_cost:.2})"
    );
    assert!(
        (actual - expected_total).abs() < 0.01,
        "RDS total should be ~${expected_total:.2} (instance ${instance_cost:.2} + storage ${storage_cost:.2}), got ${actual:.2}"
    );

    // gp2 RDS has no required variables
    assert_eq!(
        rds.required_variables.len(),
        0,
        "non-Aurora gp2 RDS should have no required variables"
    );
}

// =========================================================================
// Test 6: A 4-task Fargate service costs more than a 2-task service
// =========================================================================
#[test]
fn test_multitask_fargate_costs_more() {
    // web-app: WebService has DesiredCount: 2
    let web_resources = load_fixture("fargate-web-app.yml");
    let web_arch = build_arch("web-app", &web_resources, false);

    // production: ApiService has DesiredCount: 4
    let prd_resources = load_fixture("fargate-production.yml");
    let prd_arch = build_arch("production", &prd_resources, false);

    let vcpu = 1.0;
    let memory_gb = 2.0;
    let egress_gb = 0.0;

    // Params for web-app (WebService)
    let web_params = p(&[
        ("WebService_vcpu", vcpu),
        ("WebService_memory_gb", memory_gb),
        ("WebService_data_transfer_out_gb", egress_gb),
        ("AppLoadBalancer_lcu", 0.0),
    ]);
    // Params for production (ApiService + BackendService)
    let prd_params = p(&[
        ("ApiService_vcpu", vcpu),
        ("ApiService_memory_gb", memory_gb),
        ("ApiService_data_transfer_out_gb", egress_gb),
        ("BackendService_vcpu", vcpu),
        ("BackendService_memory_gb", memory_gb),
        ("BackendService_data_transfer_out_gb", egress_gb),
        ("AppLoadBalancer_lcu", 0.0),
        ("NatGateway_data_processed_gb", 0.0),
        ("RedisCluster_lcu", 0.0), // not actually needed, but harmless
    ]);

    let web_api_svc = web_arch
        .resources
        .iter()
        .find(|r| r.logical_id.as_str() == "WebService")
        .expect("WebService not found");
    let prd_api_svc = prd_arch
        .resources
        .iter()
        .find(|r| r.logical_id.as_str() == "ApiService")
        .expect("ApiService not found");

    let web_cost = yevice_core::evaluate::evaluate(&web_api_svc.expr, &web_params).unwrap();
    let prd_cost = yevice_core::evaluate::evaluate(&prd_api_svc.expr, &prd_params).unwrap();

    // 4-task should cost exactly 2x the 2-task service with same vcpu/memory
    assert!(
        prd_cost > web_cost,
        "4-task ApiService (${prd_cost:.2}) should cost more than 2-task WebService (${web_cost:.2})"
    );
    assert!(
        (prd_cost - 2.0 * web_cost).abs() < 0.01,
        "4-task should be exactly 2x 2-task cost: expected ${:.2}, got ${prd_cost:.2}",
        2.0 * web_cost
    );
}

// =========================================================================
// Test 7: Multi-AZ RDS costs more than single-AZ
// =========================================================================
#[test]
fn test_multiaz_rds_costs_more_than_singleaz() {
    // web-app: db.t3.medium single-AZ
    let web_resources = load_fixture("fargate-web-app.yml");
    let web_arch = build_arch("web-app", &web_resources, false);

    // production: db.r5.large Multi-AZ
    let prd_resources = load_fixture("fargate-production.yml");
    let prd_arch = build_arch("production", &prd_resources, false);

    let web_rds = web_arch
        .resources
        .iter()
        .find(|r| r.label.starts_with("RDS:"))
        .expect("AppDatabase not found");
    let prd_rds = prd_arch
        .resources
        .iter()
        .find(|r| r.label.starts_with("RDS:"))
        .expect("ProdDatabase not found");

    let web_rds_cost = yevice_core::evaluate::evaluate(&web_rds.expr, &Params::default()).unwrap();
    let prd_rds_cost = yevice_core::evaluate::evaluate(&prd_rds.expr, &Params::default()).unwrap();

    // production: r5.large MultiAZ = 0.290 * 730 * 2 + 0.138 * 500 * 2 = 423.40 + 138.00 = 561.40
    //   (Multi-AZ replicates storage to the standby, so storage is billed twice)
    // web-app: t3.medium single = 0.104 * 730 + 0.138 * 100 = 75.92 + 13.80 = 89.72
    assert!(
        prd_rds_cost > web_rds_cost,
        "Multi-AZ r5.large RDS (${prd_rds_cost:.2}) should cost more than single-AZ t3.medium (${web_rds_cost:.2})"
    );

    let expected_prd = RDS_R5_LARGE_HOURLY * HOURS * 2.0 + RDS_GP2_STORAGE_PER_GB * 500.0 * 2.0;
    let expected_web = RDS_T3_MEDIUM_HOURLY * HOURS + RDS_GP2_STORAGE_PER_GB * 100.0;
    assert!(
        (prd_rds_cost - expected_prd).abs() < 0.01,
        "Multi-AZ r5.large cost should be ~${expected_prd:.2}, got ${prd_rds_cost:.2}"
    );
    assert!(
        (web_rds_cost - expected_web).abs() < 0.01,
        "Single-AZ t3.medium cost should be ~${expected_web:.2}, got ${web_rds_cost:.2}"
    );
}

// =========================================================================
// Test 8: Production total cost > dev total cost
// =========================================================================
#[test]
fn test_fargate_prd_vs_dev_total_cost() {
    let dev_resources = load_fixture("fargate-dev.yml");
    let dev_arch = build_arch("fargate-dev", &dev_resources, false);

    let prd_resources = load_fixture("fargate-production.yml");
    let prd_arch = build_arch("fargate-prd", &prd_resources, false);

    // Minimal usage params
    let dev_params = p(&[
        ("DevService_vcpu", 0.25),
        ("DevService_memory_gb", 0.5),
        ("DevService_data_transfer_out_gb", 0.0),
        ("DevLoadBalancer_lcu", 0.0),
    ]);
    let prd_params = p(&[
        ("ApiService_vcpu", 1.0),
        ("ApiService_memory_gb", 2.0),
        ("ApiService_data_transfer_out_gb", 0.0),
        ("BackendService_vcpu", 1.0),
        ("BackendService_memory_gb", 2.0),
        ("BackendService_data_transfer_out_gb", 0.0),
        ("AppLoadBalancer_lcu", 0.0),
        ("NatGateway_data_processed_gb", 0.0),
    ]);

    let dev_result = evaluate_architecture(&dev_arch, &dev_params).unwrap();
    let prd_result = evaluate_architecture(&prd_arch, &prd_params).unwrap();

    assert!(
        prd_result.naive_total() > dev_result.naive_total(),
        "production (${:.2}) should cost more than dev (${:.2})",
        prd_result.naive_total(),
        dev_result.naive_total()
    );
}

// =========================================================================
// Test 9: NAT Gateway has fixed base cost plus data variable
// =========================================================================
#[test]
fn test_fargate_prd_nat_gateway_base_cost() {
    let resources = load_fixture("fargate-production.yml");
    let arch = build_arch("fargate-prd", &resources, false);

    let nat = arch
        .resources
        .iter()
        .find(|r| r.label.starts_with("NAT Gateway:"))
        .expect("NatGateway not found");

    // NAT GW has 1 required variable: data_processed_gb
    assert_eq!(
        nat.required_variables.len(),
        1,
        "NAT Gateway should have 1 required variable (data_processed_gb)"
    );
    assert_eq!(
        nat.required_variables[0].name.as_str(),
        "NatGateway_data_processed_gb"
    );

    // With 0 GB data, cost = hourly * 730 = 0.062 * 730 = $45.26
    let base_cost = NAT_HOUR * HOURS;
    let params_zero = p(&[("NatGateway_data_processed_gb", 0.0)]);
    let cost_zero = yevice_core::evaluate::evaluate(&nat.expr, &params_zero).unwrap();

    assert!(
        (cost_zero - base_cost).abs() < 0.01,
        "NAT GW base cost should be ~${base_cost:.2}, got ${cost_zero:.2}"
    );

    // With 100 GB data, cost = base + 0.062 * 100 = $45.26 + $6.20 = $51.46
    let params_data = p(&[("NatGateway_data_processed_gb", 100.0)]);
    let cost_data = yevice_core::evaluate::evaluate(&nat.expr, &params_data).unwrap();
    let expected_data = base_cost + 0.062 * 100.0;

    assert!(
        (cost_data - expected_data).abs() < 0.01,
        "NAT GW with 100 GB data should be ~${expected_data:.2}, got ${cost_data:.2}"
    );
    assert!(
        cost_data > cost_zero,
        "NAT GW cost with data (${cost_data:.2}) should exceed base cost (${cost_zero:.2})"
    );
}

// =========================================================================
// Test 10: More vCPU → proportionally more cost for Fargate
// =========================================================================
#[test]
fn test_fargate_vcpu_scales_cost() {
    let resources = load_fixture("fargate-web-app.yml");
    let arch = build_arch("fargate-web-app", &resources, false);

    let web_svc = arch
        .resources
        .iter()
        .find(|r| r.logical_id.as_str() == "WebService")
        .expect("WebService not found");

    let mem = 2.0;
    let egress = 0.0;

    let params_1cpu = p(&[
        ("WebService_vcpu", 1.0),
        ("WebService_memory_gb", mem),
        ("WebService_data_transfer_out_gb", egress),
    ]);
    let params_2cpu = p(&[
        ("WebService_vcpu", 2.0),
        ("WebService_memory_gb", mem),
        ("WebService_data_transfer_out_gb", egress),
    ]);
    let params_4cpu = p(&[
        ("WebService_vcpu", 4.0),
        ("WebService_memory_gb", mem),
        ("WebService_data_transfer_out_gb", egress),
    ]);

    let cost_1cpu = yevice_core::evaluate::evaluate(&web_svc.expr, &params_1cpu).unwrap();
    let cost_2cpu = yevice_core::evaluate::evaluate(&web_svc.expr, &params_2cpu).unwrap();
    let cost_4cpu = yevice_core::evaluate::evaluate(&web_svc.expr, &params_4cpu).unwrap();

    assert!(
        cost_2cpu > cost_1cpu,
        "2 vCPU (${cost_2cpu:.2}) should cost more than 1 vCPU (${cost_1cpu:.2})"
    );
    assert!(
        cost_4cpu > cost_2cpu,
        "4 vCPU (${cost_4cpu:.2}) should cost more than 2 vCPU (${cost_2cpu:.2})"
    );

    // vCPU cost is linear: diff should be exactly 1 vCPU * 2 tasks * monthly_price
    let vcpu_monthly = VCPU_HOUR * HOURS;
    let desired_count = 2.0;
    let expected_diff_per_vcpu = vcpu_monthly * desired_count;

    let diff_1_to_2 = cost_2cpu - cost_1cpu;
    assert!(
        (diff_1_to_2 - expected_diff_per_vcpu).abs() < 0.01,
        "Adding 1 vCPU to 2-task service should add ${expected_diff_per_vcpu:.2}, got ${diff_1_to_2:.2}"
    );

    let diff_2_to_4 = cost_4cpu - cost_2cpu;
    let expected_diff_2vcpu = 2.0 * expected_diff_per_vcpu;
    assert!(
        (diff_2_to_4 - expected_diff_2vcpu).abs() < 0.01,
        "Adding 2 vCPU to 2-task service should add ${expected_diff_2vcpu:.2}, got ${diff_2_to_4:.2}"
    );
}

// =========================================================================
// Bonus test: fargate-production.yml has 6 resources
// =========================================================================
#[test]
fn test_fargate_production_resource_count() {
    let resources = load_fixture("fargate-production.yml");
    let arch = build_arch("fargate-production", &resources, false);

    // ApiService, BackendService, AppLoadBalancer, ProdDatabase, RedisCluster, NatGateway
    assert_eq!(
        arch.resources.len(),
        6,
        "expected 6 resources, got {}: {:?}",
        arch.resources.len(),
        arch.resources.iter().map(|r| &r.label).collect::<Vec<_>>()
    );
}

// =========================================================================
// Bonus test: fargate-dev.yml has 3 resources
// =========================================================================
#[test]
fn test_fargate_dev_resource_count() {
    let resources = load_fixture("fargate-dev.yml");
    let arch = build_arch("fargate-dev", &resources, false);

    // DevService, DevLoadBalancer, DevDatabase
    assert_eq!(
        arch.resources.len(),
        3,
        "expected 3 resources, got {}: {:?}",
        arch.resources.len(),
        arch.resources.iter().map(|r| &r.label).collect::<Vec<_>>()
    );
}

// =========================================================================
// Bonus test: ALB label and lcu variable
// =========================================================================
#[test]
fn test_alb_label_and_lcu_variable() {
    let resources = load_fixture("fargate-web-app.yml");
    let arch = build_arch("fargate-web-app", &resources, false);

    let alb = arch
        .resources
        .iter()
        .find(|r| r.label.starts_with("ALB:"))
        .expect("should have ALB resource");

    assert_eq!(alb.label, "ALB: AppLoadBalancer");

    assert_eq!(
        alb.required_variables.len(),
        1,
        "ALB should have 1 required variable (lcu)"
    );
    assert_eq!(
        alb.required_variables[0].name.as_str(),
        "AppLoadBalancer_lcu"
    );

    // Fixed cost (0 LCU) = 0.0243 * 730 = $17.739
    let base = ALB_HOUR * HOURS;
    let params_zero = p(&[("AppLoadBalancer_lcu", 0.0)]);
    let cost_zero = yevice_core::evaluate::evaluate(&alb.expr, &params_zero).unwrap();
    assert!(
        (cost_zero - base).abs() < 0.01,
        "ALB base cost should be ~${base:.2}, got ${cost_zero:.2}"
    );
}
