use serde::{Deserialize, Serialize};
use yevice_core::{
    cost::{CostComponent, ResourceCost, VariableInfo},
    expr::Expr,
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

use super::lambda::egress_cost_expr;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EcsEc2Spec {
    pub instance_type: String,
    pub instance_count: Option<f64>,
}

pub struct EcsEc2Service;

const HOURS_PER_MONTH: f64 = 730.0;

impl Service for EcsEc2Service {
    type Spec = EcsEc2Spec;

    fn id(&self) -> &'static str {
        "aws.ecs_ec2"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &EcsEc2Spec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let sku = Sku::dynamic(format!("aws.ec2.instance.{}", spec.instance_type));
        let hourly_price = pricing.lookup_f64(&sku)?;
        let instance_expr = match spec.instance_count {
            Some(n) => Expr::constant(n),
            None => Expr::variable(id.var("instance_count")),
        };
        let instance_cost = Expr::linear(hourly_price * HOURS_PER_MONTH, instance_expr, 0.0);
        let (egress_cost, egress_var) = egress_cost_expr(id, pricing)?;

        let mut vars = vec![];
        if spec.instance_count.is_none() {
            vars.push(VariableInfo::new(
                id,
                "instance_count",
                "Number of EC2 instances in the ECS cluster",
                "instances",
            ));
        }
        vars.push(egress_var);

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("ECS on EC2: {id} ({})", spec.instance_type),
            expr: Expr::sum(vec![instance_cost.clone(), egress_cost.clone()]),
            components: vec![
                CostComponent {
                    name: format!("EC2 Instances ({})", spec.instance_type),
                    expr: instance_cost,
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

const _: f64 = HOURS_PER_MONTH;
