use serde::{Deserialize, Serialize};
use yevice_core::{
    HOURS_PER_MONTH,
    cost::{CostComponent, ResourceCost},
    expr::Expr,
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

use crate::common::egress_cost_expr;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
pub enum Ec2Os {
    #[default]
    Linux,
    Windows,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ec2Spec {
    pub instance_type: String,
    #[serde(default)]
    pub os: Ec2Os,
}

pub struct Ec2Service;

impl Service for Ec2Service {
    type Spec = Ec2Spec;

    fn id(&self) -> &'static str {
        "aws.ec2"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &Ec2Spec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        // Windows carries a license uplift; route to the Windows SKU.
        let (sku, os_label) = match spec.os {
            Ec2Os::Windows => (
                Sku::dynamic(format!("aws.ec2.os.windows.{}", spec.instance_type)),
                "Windows",
            ),
            Ec2Os::Linux => (
                Sku::dynamic(format!("aws.ec2.instance.{}", spec.instance_type)),
                "Linux",
            ),
        };
        let hourly_price = pricing.lookup_f64(&sku)?;
        let instance_cost = Expr::constant(hourly_price * HOURS_PER_MONTH);
        let (egress_cost, egress_var) = egress_cost_expr(id, pricing)?;

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("EC2: {id} ({}, {os_label})", spec.instance_type),
            expr: Expr::sum(vec![instance_cost.clone(), egress_cost.clone()]),
            components: vec![
                CostComponent {
                    name: format!("Instance ({}, {os_label})", spec.instance_type),
                    expr: instance_cost,
                },
                CostComponent {
                    name: "Data Transfer Out".into(),
                    expr: egress_cost,
                },
            ],
            required_variables: vec![egress_var],
        })
    }
}
