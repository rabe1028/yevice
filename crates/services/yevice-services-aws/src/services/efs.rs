use serde::{Deserialize, Serialize};
use yevice_core::{
    cost::{CostComponent, ResourceCost, VariableInfo},
    expr::Expr,
    resource::Provider,
    types::{LogicalId, ResourceType, var},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EfsSpec {
    /// True if the filesystem has a `LifecyclePolicies` entry that transitions
    /// files to the IA storage class. Drives the per-GB pricing tier.
    pub has_ia_lifecycle: bool,
}

pub struct EfsService;

impl Service for EfsService {
    type Spec = EfsSpec;

    fn id(&self) -> &'static str {
        "aws.efs"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &EfsSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let standard_price = pricing.lookup_f64(&Sku::new("aws.efs.standard_gb_month_price"))?;
        let ia_price = pricing.lookup_f64(&Sku::new("aws.efs.ia_gb_month_price"))?;

        let storage_price = if spec.has_ia_lifecycle {
            ia_price
        } else {
            standard_price
        };
        let storage_label = if spec.has_ia_lifecycle {
            "IA"
        } else {
            "Standard"
        };

        let storage_cost =
            Expr::linear(storage_price, Expr::variable(id.var(var::STORAGE_GB)), 0.0);

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("EFS ({storage_label}): {id}"),
            expr: storage_cost.clone(),
            components: vec![CostComponent {
                name: format!("Storage ({storage_label})"),
                expr: storage_cost,

                currency: None,
            }],
            required_variables: vec![VariableInfo::new(
                id,
                var::STORAGE_GB,
                "File storage per month",
                "GB",
            )],

            currency: Some("USD".into()),
        })
    }
}
