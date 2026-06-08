use serde::{Deserialize, Serialize};
use yevice_core::{
    capacity::{CapacityModel, Constraint, QuotaType, Quotas, Severity},
    cost::{CostComponent, ResourceCost, VariableInfo},
    expr::{Expr, Tier},
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

use crate::quotas::{DEFAULT_LAMBDA_CONCURRENT_EXECUTIONS, LAMBDA_CONCURRENT_EXECUTIONS};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LambdaSpec {
    pub memory_mb: f64,
    pub timeout_sec: f64,
    pub runtime: Option<String>,
}

impl Default for LambdaSpec {
    fn default() -> Self {
        Self {
            memory_mb: 128.0,
            timeout_sec: 3.0,
            runtime: None,
        }
    }
}

const SKU_REQUEST: Sku = Sku::new("aws.lambda.request_price");
const SKU_GB_SECOND: Sku = Sku::new("aws.lambda.gb_second");
const SKU_FREE_REQUESTS: Sku = Sku::new("aws.lambda.free_tier_requests");
const SKU_FREE_GB_SEC: Sku = Sku::new("aws.lambda.free_tier_gb_seconds");

pub struct LambdaService;

pub fn egress_cost_expr(
    id: &LogicalId,
    pricing: &dyn PriceCatalog,
) -> Result<(Expr, VariableInfo), CostError> {
    let egress_record = pricing.lookup(&Sku::new("aws.data_transfer.egress_tiers"))?;
    let egress_tiers = egress_record.as_tiered().map_err(CostError::Pricing)?;
    let expr = Expr::tiered(
        egress_tiers.to_vec(),
        Expr::variable(id.var("data_transfer_out_gb")),
    );
    let info = VariableInfo::new(
        id,
        "data_transfer_out_gb",
        "Data transfer out per month",
        "GB",
    );
    Ok((expr, info))
}

impl Service for LambdaService {
    type Spec = LambdaSpec;

    fn id(&self) -> &'static str {
        "aws.lambda"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &LambdaSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let request_price = pricing.lookup_f64(&SKU_REQUEST)?;
        let gb_second_price = pricing.lookup_f64(&SKU_GB_SECOND)?;
        let free_tier_requests = pricing.lookup_f64(&SKU_FREE_REQUESTS)?;
        let free_tier_gb_seconds = pricing.lookup_f64(&SKU_FREE_GB_SEC)?;

        let memory_gb = spec.memory_mb / 1024.0;

        let request_cost = Expr::tiered(
            vec![
                Tier {
                    upper_limit: Some(free_tier_requests),
                    unit_price: 0.0,
                },
                Tier {
                    upper_limit: None,
                    unit_price: request_price,
                },
            ],
            Expr::variable(id.var("requests")),
        );

        let gb_seconds = Expr::product(vec![
            Expr::variable(id.var("requests")),
            Expr::linear(1.0 / 1000.0, Expr::variable(id.var("avg_duration_ms")), 0.0),
            Expr::constant(memory_gb),
        ]);

        let compute_cost = Expr::tiered(
            vec![
                Tier {
                    upper_limit: Some(free_tier_gb_seconds),
                    unit_price: 0.0,
                },
                Tier {
                    upper_limit: None,
                    unit_price: gb_second_price,
                },
            ],
            gb_seconds,
        );

        let (egress_cost, egress_var) = egress_cost_expr(id, pricing)?;

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("Lambda: {id}"),
            expr: Expr::sum(vec![
                request_cost.clone(),
                compute_cost.clone(),
                egress_cost.clone(),
            ]),
            components: vec![
                CostComponent {
                    name: "Requests".into(),
                    expr: request_cost,
                },
                CostComponent {
                    name: format!("Compute ({}MB)", spec.memory_mb),
                    expr: compute_cost,
                },
                CostComponent {
                    name: "Data Transfer Out".into(),
                    expr: egress_cost,
                },
            ],
            required_variables: vec![
                VariableInfo::new(id, "requests", "Lambda invocations per month", "requests"),
                VariableInfo::new(
                    id,
                    "avg_duration_ms",
                    "Average duration per invocation",
                    "ms",
                ),
                egress_var,
            ],
        })
    }

    fn build_capacity(
        &self,
        id: &LogicalId,
        _spec: &LambdaSpec,
        quotas: &Quotas,
    ) -> Option<CapacityModel> {
        let concurrent = Expr::product(vec![
            Expr::variable(id.var("peak_requests_per_sec")),
            Expr::linear(1.0 / 1000.0, Expr::variable(id.var("avg_duration_ms")), 0.0),
        ]);

        Some(CapacityModel {
            logical_id: id.clone(),
            label: format!("Lambda: {id}"),
            constraints: vec![Constraint {
                dimension: "concurrent_executions".into(),
                required: concurrent,
                limit: quotas
                    .get(LAMBDA_CONCURRENT_EXECUTIONS)
                    .unwrap_or(DEFAULT_LAMBDA_CONCURRENT_EXECUTIONS),
                quota_type: QuotaType::Soft,
                severity: Severity::Error,
                message_template: "Concurrent executions {required} exceeds account quota {limit}"
                    .into(),
            }],
        })
    }
}
