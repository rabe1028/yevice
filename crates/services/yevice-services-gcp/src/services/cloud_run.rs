//! GCP Cloud Run service implementation.

use serde::{Deserialize, Serialize};
use yevice_core::{
    cost::{CostComponent, Expr, ResourceCost, Tier, VariableInfo, VariableKind},
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::{PriceCatalog, Sku};
use yevice_service_api::{CostError, service::Service};

// Free tier constants
const FREE_TIER_REQUESTS: f64 = 2_000_000.0;
const FREE_TIER_VCPU_SECONDS: f64 = 180_000.0;
const FREE_TIER_MEMORY_GB_SECONDS: f64 = 360_000.0;
const SECONDS_PER_MONTH: f64 = 730.0 * 3600.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcpCloudRunSpec {
    pub cpu: f64,
    pub memory_mb: f64,
    pub min_instances: Option<f64>,
}

impl Default for GcpCloudRunSpec {
    fn default() -> Self {
        Self {
            cpu: 1.0,
            memory_mb: 512.0,
            min_instances: None,
        }
    }
}

pub struct GcpCloudRunService;

impl Service for GcpCloudRunService {
    type Spec = GcpCloudRunSpec;

    fn id(&self) -> &'static str {
        "gcp.cloud_run"
    }

    fn provider(&self) -> Provider {
        Provider::Gcp
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &GcpCloudRunSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let req_per_million = pricing.lookup_f64(&Sku::new("gcp.cloud_run.request_per_million"))?;
        let vcpu_second = pricing.lookup_f64(&Sku::new("gcp.cloud_run.vcpu_second"))?;
        let memory_gb_second = pricing.lookup_f64(&Sku::new("gcp.cloud_run.memory_gb_second"))?;
        let idle_vcpu_second = pricing.lookup_f64(&Sku::new("gcp.cloud_run.idle_vcpu_second"))?;

        let memory_gb = spec.memory_mb / 1024.0;
        let price_per_request = req_per_million / 1_000_000.0;

        let request_cost = Expr::tiered(
            vec![
                Tier {
                    upper_limit: Some(FREE_TIER_REQUESTS),
                    unit_price: 0.0,
                },
                Tier {
                    upper_limit: None,
                    unit_price: price_per_request,
                },
            ],
            Expr::variable(id.var("monthly_requests")),
        );

        let vcpu_seconds_expr = Expr::product(vec![
            Expr::variable(id.var("monthly_requests")),
            Expr::linear(1.0 / 1000.0, Expr::variable(id.var("avg_duration_ms")), 0.0),
            Expr::constant(spec.cpu),
        ]);

        let vcpu_cost = Expr::tiered(
            vec![
                Tier {
                    upper_limit: Some(FREE_TIER_VCPU_SECONDS),
                    unit_price: 0.0,
                },
                Tier {
                    upper_limit: None,
                    unit_price: vcpu_second,
                },
            ],
            vcpu_seconds_expr,
        );

        let mem_seconds_expr = Expr::product(vec![
            Expr::variable(id.var("monthly_requests")),
            Expr::linear(1.0 / 1000.0, Expr::variable(id.var("avg_duration_ms")), 0.0),
            Expr::constant(memory_gb),
        ]);

        let mem_cost = Expr::tiered(
            vec![
                Tier {
                    upper_limit: Some(FREE_TIER_MEMORY_GB_SECONDS),
                    unit_price: 0.0,
                },
                Tier {
                    upper_limit: None,
                    unit_price: memory_gb_second,
                },
            ],
            mem_seconds_expr,
        );

        // Min-instances stay warm and are billed even with no traffic, so a
        // request-only model would report $0 for an always-warm service. They
        // are billed at the reduced *idle* vCPU rate, and only for the time they
        // are NOT serving: idle_seconds = max(0, min_instances*month - active
        // request instance-seconds). The active serving time is billed by the
        // vCPU/memory components above, so this avoids double-counting. Zero
        // when `min_instances` is unset/0.
        let min_instances = spec.min_instances.unwrap_or(0.0);
        let allocated_seconds = min_instances * SECONDS_PER_MONTH;
        let active_instance_seconds = Expr::product(vec![
            Expr::variable(id.var("monthly_requests")),
            Expr::linear(1.0 / 1000.0, Expr::variable(id.var("avg_duration_ms")), 0.0),
        ]);
        let idle_seconds = Expr::Max {
            expr: Box::new(Expr::sum(vec![
                Expr::constant(allocated_seconds),
                Expr::product(vec![Expr::constant(-1.0), active_instance_seconds]),
            ])),
            floor: 0.0,
        };
        let min_instance_cost = Expr::product(vec![
            idle_seconds,
            Expr::constant(spec.cpu * idle_vcpu_second + memory_gb * memory_gb_second),
        ]);

        let mut components = vec![
            CostComponent {
                name: "Requests".into(),
                expr: request_cost.clone(),
            },
            CostComponent {
                name: format!("vCPU ({} core)", spec.cpu),
                expr: vcpu_cost.clone(),
            },
            CostComponent {
                name: format!("Memory ({}MB)", spec.memory_mb as u32),
                expr: mem_cost.clone(),
            },
        ];
        let mut expr_parts = vec![request_cost, vcpu_cost, mem_cost];
        if min_instances > 0.0 {
            components.push(CostComponent {
                name: format!("Min Instances ({min_instances} warm)"),
                expr: min_instance_cost.clone(),
            });
            expr_parts.push(min_instance_cost);
        }

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!(
                "Cloud Run ({} vCPU / {}MB)",
                spec.cpu, spec.memory_mb as u32
            ),
            expr: Expr::sum(expr_parts),
            components,
            required_variables: vec![
                VariableInfo {
                    name: id.var("monthly_requests"),
                    description: "Requests per month".into(),
                    unit: "count".into(),
                    kind: VariableKind::Usage,
                },
                VariableInfo {
                    name: id.var("avg_duration_ms"),
                    description: "Average request duration".into(),
                    unit: "ms".into(),
                    kind: VariableKind::Usage,
                },
            ],
        })
    }
}
