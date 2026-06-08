//! Cloudflare service implementations.
//!
//! Pricing reference (Workers Paid plan, as of 2024):
//!   Workers Standard: $5/mo includes 10M req + 30M CPU-ms; +$0.30/M req, +$0.02/M CPU-ms
//!   KV: included 10M reads, 1M writes; +$0.50/M reads, +$5.00/M writes
//!   R2: $0.015/GB-month storage; Class A $4.50/M ops, Class B $0.36/M ops; free egress
//!   D1: included 25B rows read, 50M rows written; +$0.001/M rows read, +$1.00/M rows written
//!   Queues: 1M ops/month free; +$0.40/M ops (2 ops per message: publish + deliver)
//!   Durable Objects: 1M req/mo free; +$0.15/M req; +$12.50/M GB-sec; +$0.20/GB-mo storage

use serde::{Deserialize, Serialize};
use yevice_core::{
    cost::{CostComponent, Expr, ResourceCost, Tier, VariableInfo},
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::PriceCatalog;
use yevice_service_api::{CostError, service::Service};

// ---------------------------------------------------------------------------
// Cloudflare Workers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum CloudflareUsageModel {
    #[default]
    Standard,
    Bundled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudflareWorkerSpec {
    #[serde(default)]
    pub usage_model: CloudflareUsageModel,
}

pub struct CloudflareWorkerService;

impl Service for CloudflareWorkerService {
    type Spec = CloudflareWorkerSpec;

    fn id(&self) -> &'static str {
        "cloudflare.worker"
    }

    fn provider(&self) -> Provider {
        Provider::Cloudflare
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &CloudflareWorkerSpec,
        _pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        // Overage pricing differs by usage model
        let (req_overage_price, cpu_overage_price) = match spec.usage_model {
            CloudflareUsageModel::Bundled => (0.50 / 1_000_000.0, 0.0),
            CloudflareUsageModel::Standard => (0.30 / 1_000_000.0, 0.02 / 1_000_000.0),
        };

        let base_cost = Expr::constant(5.0); // $5/month Paid plan

        let request_overage = Expr::tiered(
            vec![
                Tier {
                    upper_limit: Some(10_000_000.0),
                    unit_price: 0.0,
                },
                Tier {
                    upper_limit: None,
                    unit_price: req_overage_price,
                },
            ],
            Expr::variable(id.var("monthly_requests")),
        );

        let cpu_overage = Expr::tiered(
            vec![
                Tier {
                    upper_limit: Some(30_000_000.0),
                    unit_price: 0.0,
                },
                Tier {
                    upper_limit: None,
                    unit_price: cpu_overage_price,
                },
            ],
            Expr::product(vec![
                Expr::variable(id.var("monthly_requests")),
                Expr::variable(id.var("avg_cpu_ms")),
            ]),
        );

        let total = Expr::sum(vec![
            base_cost.clone(),
            request_overage.clone(),
            cpu_overage.clone(),
        ]);
        let model_label = match spec.usage_model {
            CloudflareUsageModel::Bundled => "Bundled",
            CloudflareUsageModel::Standard => "Standard",
        };

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("Workers ({model_label})"),
            expr: total,
            components: vec![
                CostComponent {
                    name: "Paid plan base".into(),
                    expr: base_cost,
                },
                CostComponent {
                    name: "Requests (over 10M)".into(),
                    expr: request_overage,
                },
                CostComponent {
                    name: "CPU time (over 30M ms)".into(),
                    expr: cpu_overage,
                },
            ],
            required_variables: vec![
                VariableInfo {
                    name: id.var("monthly_requests"),
                    description: "HTTP requests per month".into(),
                    unit: "count".into(),
                },
                VariableInfo {
                    name: id.var("avg_cpu_ms"),
                    description: "Average CPU time per request".into(),
                    unit: "ms".into(),
                },
            ],
        })
    }
}

// ---------------------------------------------------------------------------
// Workers KV
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudflareKvSpec {}

pub struct CloudflareKvService;

impl Service for CloudflareKvService {
    type Spec = CloudflareKvSpec;

    fn id(&self) -> &'static str {
        "cloudflare.kv"
    }

    fn provider(&self) -> Provider {
        Provider::Cloudflare
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        _spec: &CloudflareKvSpec,
        _pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let read_cost = Expr::tiered(
            vec![
                Tier {
                    upper_limit: Some(10_000_000.0),
                    unit_price: 0.0,
                },
                Tier {
                    upper_limit: None,
                    unit_price: 0.50 / 1_000_000.0,
                },
            ],
            Expr::variable(id.var("monthly_reads")),
        );

        let write_cost = Expr::tiered(
            vec![
                Tier {
                    upper_limit: Some(1_000_000.0),
                    unit_price: 0.0,
                },
                Tier {
                    upper_limit: None,
                    unit_price: 5.00 / 1_000_000.0,
                },
            ],
            Expr::variable(id.var("monthly_writes")),
        );

        let total = Expr::sum(vec![read_cost.clone(), write_cost.clone()]);

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: "Workers KV".into(),
            expr: total,
            components: vec![
                CostComponent {
                    name: "Reads".into(),
                    expr: read_cost,
                },
                CostComponent {
                    name: "Writes".into(),
                    expr: write_cost,
                },
            ],
            required_variables: vec![
                VariableInfo {
                    name: id.var("monthly_reads"),
                    description: "KV read operations per month".into(),
                    unit: "count".into(),
                },
                VariableInfo {
                    name: id.var("monthly_writes"),
                    description: "KV write operations per month".into(),
                    unit: "count".into(),
                },
            ],
        })
    }
}

// ---------------------------------------------------------------------------
// R2 Storage
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudflareR2Spec {}

pub struct CloudflareR2Service;

impl Service for CloudflareR2Service {
    type Spec = CloudflareR2Spec;

    fn id(&self) -> &'static str {
        "cloudflare.r2"
    }

    fn provider(&self) -> Provider {
        Provider::Cloudflare
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        _spec: &CloudflareR2Spec,
        _pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let storage_cost = Expr::tiered(
            vec![
                Tier {
                    upper_limit: Some(10.0),
                    unit_price: 0.0,
                },
                Tier {
                    upper_limit: None,
                    unit_price: 0.015,
                },
            ],
            Expr::variable(id.var("storage_gb")),
        );

        let class_a_cost = Expr::tiered(
            vec![
                Tier {
                    upper_limit: Some(1_000_000.0),
                    unit_price: 0.0,
                },
                Tier {
                    upper_limit: None,
                    unit_price: 4.50 / 1_000_000.0,
                },
            ],
            Expr::variable(id.var("class_a_ops")),
        );

        let class_b_cost = Expr::tiered(
            vec![
                Tier {
                    upper_limit: Some(10_000_000.0),
                    unit_price: 0.0,
                },
                Tier {
                    upper_limit: None,
                    unit_price: 0.36 / 1_000_000.0,
                },
            ],
            Expr::variable(id.var("class_b_ops")),
        );

        let total = Expr::sum(vec![
            storage_cost.clone(),
            class_a_cost.clone(),
            class_b_cost.clone(),
        ]);

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: "R2 Storage".into(),
            expr: total,
            components: vec![
                CostComponent {
                    name: "Storage".into(),
                    expr: storage_cost,
                },
                CostComponent {
                    name: "Class A ops (write/list)".into(),
                    expr: class_a_cost,
                },
                CostComponent {
                    name: "Class B ops (read)".into(),
                    expr: class_b_cost,
                },
            ],
            required_variables: vec![
                VariableInfo {
                    name: id.var("storage_gb"),
                    description: "Data stored per month".into(),
                    unit: "GB".into(),
                },
                VariableInfo {
                    name: id.var("class_a_ops"),
                    description: "Class A operations (writes, lists) per month".into(),
                    unit: "count".into(),
                },
                VariableInfo {
                    name: id.var("class_b_ops"),
                    description: "Class B operations (reads) per month".into(),
                    unit: "count".into(),
                },
            ],
        })
    }
}

// ---------------------------------------------------------------------------
// D1 Database
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudflareD1Spec {}

pub struct CloudflareD1Service;

impl Service for CloudflareD1Service {
    type Spec = CloudflareD1Spec;

    fn id(&self) -> &'static str {
        "cloudflare.d1"
    }

    fn provider(&self) -> Provider {
        Provider::Cloudflare
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        _spec: &CloudflareD1Spec,
        _pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let read_cost = Expr::tiered(
            vec![
                Tier {
                    upper_limit: Some(25_000_000_000.0),
                    unit_price: 0.0,
                },
                Tier {
                    upper_limit: None,
                    unit_price: 0.001 / 1_000_000.0,
                },
            ],
            Expr::variable(id.var("monthly_rows_read")),
        );

        let write_cost = Expr::tiered(
            vec![
                Tier {
                    upper_limit: Some(50_000_000.0),
                    unit_price: 0.0,
                },
                Tier {
                    upper_limit: None,
                    unit_price: 1.00 / 1_000_000.0,
                },
            ],
            Expr::variable(id.var("monthly_rows_written")),
        );

        let total = Expr::sum(vec![read_cost.clone(), write_cost.clone()]);

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: "D1 Database".into(),
            expr: total,
            components: vec![
                CostComponent {
                    name: "Rows read".into(),
                    expr: read_cost,
                },
                CostComponent {
                    name: "Rows written".into(),
                    expr: write_cost,
                },
            ],
            required_variables: vec![
                VariableInfo {
                    name: id.var("monthly_rows_read"),
                    description: "Database rows read per month".into(),
                    unit: "count".into(),
                },
                VariableInfo {
                    name: id.var("monthly_rows_written"),
                    description: "Database rows written per month".into(),
                    unit: "count".into(),
                },
            ],
        })
    }
}

// ---------------------------------------------------------------------------
// Queues
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudflareQueueSpec {}

pub struct CloudflareQueueService;

impl Service for CloudflareQueueService {
    type Spec = CloudflareQueueSpec;

    fn id(&self) -> &'static str {
        "cloudflare.queue"
    }

    fn provider(&self) -> Provider {
        Provider::Cloudflare
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        _spec: &CloudflareQueueSpec,
        _pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        // Total ops = messages * 2 (publish + deliver)
        let total_ops = Expr::product(vec![
            Expr::variable(id.var("monthly_messages")),
            Expr::constant(2.0),
        ]);

        let cost = Expr::tiered(
            vec![
                Tier {
                    upper_limit: Some(1_000_000.0),
                    unit_price: 0.0,
                },
                Tier {
                    upper_limit: None,
                    unit_price: 0.40 / 1_000_000.0,
                },
            ],
            total_ops,
        );

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: "Queues".into(),
            expr: cost.clone(),
            components: vec![CostComponent {
                name: "Message operations".into(),
                expr: cost,
            }],
            required_variables: vec![VariableInfo {
                name: id.var("monthly_messages"),
                description: "Messages published per month".into(),
                unit: "count".into(),
            }],
        })
    }
}

// ---------------------------------------------------------------------------
// Durable Objects
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudflareDurableObjectSpec {}

pub struct CloudflareDurableObjectService;

impl Service for CloudflareDurableObjectService {
    type Spec = CloudflareDurableObjectSpec;

    fn id(&self) -> &'static str {
        "cloudflare.durable_object"
    }

    fn provider(&self) -> Provider {
        Provider::Cloudflare
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        _spec: &CloudflareDurableObjectSpec,
        _pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let request_cost = Expr::tiered(
            vec![
                Tier {
                    upper_limit: Some(1_000_000.0),
                    unit_price: 0.0,
                },
                Tier {
                    upper_limit: None,
                    unit_price: 0.15 / 1_000_000.0,
                },
            ],
            Expr::variable(id.var("monthly_requests")),
        );

        let duration_cost = Expr::tiered(
            vec![
                Tier {
                    upper_limit: Some(400_000.0),
                    unit_price: 0.0,
                },
                Tier {
                    upper_limit: None,
                    unit_price: 12.50 / 1_000_000.0,
                },
            ],
            Expr::variable(id.var("monthly_gb_seconds")),
        );

        let storage_cost = Expr::tiered(
            vec![
                Tier {
                    upper_limit: Some(1.0),
                    unit_price: 0.0,
                },
                Tier {
                    upper_limit: None,
                    unit_price: 0.20,
                },
            ],
            Expr::variable(id.var("storage_gb")),
        );

        let total = Expr::sum(vec![
            request_cost.clone(),
            duration_cost.clone(),
            storage_cost.clone(),
        ]);

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: "Durable Objects".into(),
            expr: total,
            components: vec![
                CostComponent {
                    name: "Requests".into(),
                    expr: request_cost,
                },
                CostComponent {
                    name: "Duration (GB-seconds)".into(),
                    expr: duration_cost,
                },
                CostComponent {
                    name: "Storage".into(),
                    expr: storage_cost,
                },
            ],
            required_variables: vec![
                VariableInfo {
                    name: id.var("monthly_requests"),
                    description: "Requests to Durable Object per month".into(),
                    unit: "count".into(),
                },
                VariableInfo {
                    name: id.var("monthly_gb_seconds"),
                    description: "Compute duration in GB-seconds per month".into(),
                    unit: "GB-seconds".into(),
                },
                VariableInfo {
                    name: id.var("storage_gb"),
                    description: "Persistent storage used".into(),
                    unit: "GB".into(),
                },
            ],
        })
    }
}
