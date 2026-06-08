use serde::{Deserialize, Serialize};
use yevice_core::{
    cost::{CostComponent, ResourceCost, VariableInfo},
    expr::Expr,
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EcrSpec {
    pub is_private: bool,
}

pub struct EcrService;

impl Service for EcrService {
    type Spec = EcrSpec;

    fn id(&self) -> &'static str {
        "aws.ecr"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &EcrSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let private_storage = pricing.lookup_f64(&Sku::new("aws.ecr.private_storage_gb_month"))?;
        let storage_price = if spec.is_private {
            private_storage
        } else {
            0.0
        };

        let storage_cost = Expr::linear(storage_price, Expr::variable(id.var("storage_gb")), 0.0);
        let repo_type = if spec.is_private { "Private" } else { "Public" };

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("ECR {repo_type}: {id}"),
            expr: storage_cost.clone(),
            components: vec![CostComponent {
                name: format!("Storage ({repo_type})"),
                expr: storage_cost,
            }],
            required_variables: vec![VariableInfo::new(
                id,
                "storage_gb",
                "Container image storage per month",
                "GB",
            )],
        })
    }
}
