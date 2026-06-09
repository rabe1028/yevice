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
pub struct DocumentDbSpec {
    pub instance_type: String,
    pub instance_count: Option<f64>,
}

pub struct DocumentDbService;

impl Service for DocumentDbService {
    type Spec = DocumentDbSpec;

    fn id(&self) -> &'static str {
        "aws.documentdb"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &DocumentDbSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let sku = Sku::dynamic(format!("aws.documentdb.{}", spec.instance_type));
        let storage_sku = Sku::dynamic(format!("aws.documentdb_storage.{}", spec.instance_type));
        let instance_hour = pricing.lookup_f64(&sku)?;
        let storage_price = pricing.lookup_f64(&storage_sku)?;

        let instance_expr = match spec.instance_count {
            Some(n) => Expr::constant(n),
            None => Expr::variable(id.var("instance_count")),
        };

        let instance_cost = Expr::linear(instance_hour * HOURS_PER_MONTH, instance_expr, 0.0);
        let storage_cost = Expr::linear(storage_price, Expr::variable(id.var("storage_gb")), 0.0);

        let total = Expr::sum(vec![instance_cost.clone(), storage_cost.clone()]);

        let mut required = vec![VariableInfo::new(id, "storage_gb", "Storage size", "GB")];
        if spec.instance_count.is_none() {
            required.push(VariableInfo::new(
                id,
                "instance_count",
                "Number of instances in cluster",
                "instances",
            ));
        }

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("DocumentDB: {id}"),
            expr: total,
            components: vec![
                CostComponent {
                    name: "Instances".into(),
                    expr: instance_cost,
                },
                CostComponent {
                    name: "Storage".into(),
                    expr: storage_cost,
                },
            ],
            required_variables: required,
        })
    }
}
