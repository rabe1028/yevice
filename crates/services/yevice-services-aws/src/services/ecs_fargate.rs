use serde::{Deserialize, Serialize};
use yevice_core::{
    HOURS_PER_MONTH,
    cost::{CostComponent, ResourceCost, VariableInfo},
    expr::Expr,
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

use crate::common::egress_cost_expr;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EcsFargateSpec {
    pub desired_count: Option<f64>,
}

pub struct EcsFargateService;

impl Service for EcsFargateService {
    type Spec = EcsFargateSpec;

    fn id(&self) -> &'static str {
        "aws.ecs_fargate"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &EcsFargateSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let vcpu_hour = pricing.lookup_f64(&Sku::new("aws.fargate.vcpu_hour_price"))?;
        let mem_hour = pricing.lookup_f64(&Sku::new("aws.fargate.memory_gb_hour_price"))?;

        let tasks_expr = match spec.desired_count {
            Some(n) => Expr::constant(n),
            None => Expr::variable(id.var("desired_count")),
        };

        let vcpu_cost_per_task = Expr::linear(
            vcpu_hour * HOURS_PER_MONTH,
            Expr::variable(id.var("vcpu")),
            0.0,
        );
        let memory_cost_per_task = Expr::linear(
            mem_hour * HOURS_PER_MONTH,
            Expr::variable(id.var("memory_gb")),
            0.0,
        );
        let per_task = Expr::sum(vec![
            vcpu_cost_per_task.clone(),
            memory_cost_per_task.clone(),
        ]);

        // Scale the breakdown components by the number of tasks so they sum
        // to the same total as `expr`. Without this, `evaluate_architecture`
        // would report a per-task cost while `expr` correctly multiplies by
        // the task count, making the resource breakdown inconsistent.
        let vcpu_cost_total = Expr::product(vec![tasks_expr.clone(), vcpu_cost_per_task]);
        let memory_cost_total = Expr::product(vec![tasks_expr.clone(), memory_cost_per_task]);

        let mut vars = vec![
            VariableInfo::new(id, "vcpu", "vCPU per task", "vCPU"),
            VariableInfo::new(id, "memory_gb", "Memory per task", "GB"),
        ];
        if spec.desired_count.is_none() {
            vars.insert(
                0,
                VariableInfo::new(id, "desired_count", "Number of tasks", "tasks"),
            );
        }

        let (egress_cost, egress_var) = egress_cost_expr(id, pricing)?;
        vars.push(egress_var);

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("ECS Fargate: {id}"),
            expr: Expr::sum(vec![
                Expr::product(vec![tasks_expr, per_task]),
                egress_cost.clone(),
            ]),
            components: vec![
                CostComponent {
                    name: "vCPU".into(),
                    expr: vcpu_cost_total,
                },
                CostComponent {
                    name: "Memory".into(),
                    expr: memory_cost_total,
                },
                CostComponent {
                    name: "Data Transfer Out".into(),
                    expr: egress_cost,
                },
            ],
            required_variables: vars,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use yevice_core::evaluate::evaluate;
    use yevice_core::expr::Tier;
    use yevice_pricing::catalog::PriceRecord;
    use yevice_pricing::error::PricingError;

    struct TestCatalog;

    impl PriceCatalog for TestCatalog {
        fn region(&self) -> &'static str {
            "test"
        }

        fn lookup(&self, sku: &Sku) -> Result<PriceRecord, PricingError> {
            match sku.as_str() {
                "aws.fargate.vcpu_hour_price" => Ok(PriceRecord::flat(0.04)),
                "aws.fargate.memory_gb_hour_price" => Ok(PriceRecord::flat(0.004)),
                "aws.data_transfer.egress_tiers" => Ok(PriceRecord::tiered(vec![Tier {
                    upper_limit: None,
                    unit_price: 0.09,
                }])),
                other => Err(PricingError::NotFound {
                    service: other.to_string(),
                    region: "test".to_string(),
                }),
            }
        }
    }

    /// Regression: the per-resource breakdown summed each per-task component
    /// once while `expr` correctly multiplied by the number of tasks. The
    /// total in the breakdown therefore disagreed with the architecture-level
    /// total. Now each non-shared component is scaled by `tasks_expr`, so the
    /// sum of (vCPU + Memory) equals `total - egress`.
    #[test]
    fn component_breakdown_scales_with_task_count() {
        let id = LogicalId::new("svc");
        let rt = ResourceType::new("AWS::ECS::Service");
        let spec = EcsFargateSpec {
            desired_count: Some(3.0),
        };
        let pricing = TestCatalog;
        let cost = EcsFargateService
            .build_cost(&id, &rt, &spec, &pricing)
            .expect("build cost");

        let mut params: HashMap<_, _> = HashMap::new();
        params.insert(id.var("vcpu"), 0.5);
        params.insert(id.var("memory_gb"), 1.0);
        params.insert(id.var("data_transfer_out_gb"), 0.0);

        let total = evaluate(&cost.expr, &params).expect("evaluate total");

        let mut comp_sum = 0.0;
        for c in &cost.components {
            comp_sum += evaluate(&c.expr, &params).expect("evaluate component");
        }

        assert!(
            (total - comp_sum).abs() < 1e-9,
            "components ({comp_sum}) should sum to total ({total})"
        );
    }
}
