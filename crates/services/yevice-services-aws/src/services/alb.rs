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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlbSpec {
    pub load_balancer_type: String,
}

pub struct AlbService;

impl Service for AlbService {
    type Spec = AlbSpec;

    fn id(&self) -> &'static str {
        "aws.alb"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &AlbSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let alb_hour = pricing.lookup_f64(&Sku::new("aws.alb.alb_hour_price"))?;
        let lcu_hour = pricing.lookup_f64(&Sku::new("aws.alb.lcu_hour_price"))?;

        let lb_type = spec.load_balancer_type.to_ascii_uppercase();
        let is_alb = lb_type != "NETWORK";

        let fixed_cost = Expr::constant(alb_hour * HOURS_PER_MONTH);
        let lcu_cost = if is_alb {
            Expr::linear(
                lcu_hour * HOURS_PER_MONTH,
                Expr::variable(id.var("lcu")),
                0.0,
            )
        } else {
            Expr::constant(0.0)
        };

        let label = if is_alb { "ALB" } else { "NLB" };
        let mut vars = vec![];
        if is_alb {
            vars.push(VariableInfo::new(
                id,
                "lcu",
                "Avg Load Capacity Units (LCU)",
                "LCU",
            ));
        }

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("{label}: {id}"),
            expr: Expr::sum(vec![fixed_cost.clone(), lcu_cost.clone()]),
            components: vec![
                CostComponent {
                    name: format!("{label} Hours"),
                    expr: fixed_cost,
                },
                CostComponent {
                    name: "LCU Hours".into(),
                    expr: lcu_cost,
                },
            ],
            required_variables: vars,
        })
    }
}
