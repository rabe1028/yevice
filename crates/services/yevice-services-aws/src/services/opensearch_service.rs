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
pub struct OpenSearchServiceSpec {
    pub instance_type: String,
    pub instance_count: Option<f64>,
    pub storage_gb: Option<f64>,
}

pub struct OpenSearchServiceService;

const HOURS_PER_MONTH: f64 = 730.0;

impl Service for OpenSearchServiceService {
    type Spec = OpenSearchServiceSpec;

    fn id(&self) -> &'static str {
        "aws.opensearch_service"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &OpenSearchServiceSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let sku = Sku::dynamic(format!("aws.opensearch_service.{}", spec.instance_type));
        let storage_sku = Sku::dynamic(format!(
            "aws.opensearch_service_storage.{}",
            spec.instance_type
        ));
        let instance_hour = pricing.lookup_f64(&sku)?;
        let storage_price = pricing.lookup_f64(&storage_sku)?;

        let instance_expr = match spec.instance_count {
            Some(n) => Expr::constant(n),
            None => Expr::variable(id.var("instance_count")),
        };

        let instance_cost =
            Expr::linear(instance_hour * HOURS_PER_MONTH, instance_expr.clone(), 0.0);

        let storage_expr = match spec.storage_gb {
            Some(gb) => Expr::constant(gb),
            None => Expr::variable(id.var("storage_gb")),
        };
        let storage_cost = Expr::linear(
            storage_price,
            Expr::product(vec![instance_expr, storage_expr]),
            0.0,
        );

        let total = Expr::sum(vec![instance_cost.clone(), storage_cost.clone()]);

        let mut required = vec![];
        if spec.instance_count.is_none() {
            required.push(VariableInfo::new(
                id,
                "instance_count",
                "Number of data nodes",
                "nodes",
            ));
        }
        if spec.storage_gb.is_none() {
            required.push(VariableInfo::new(
                id,
                "storage_gb",
                "EBS storage per node",
                "GB",
            ));
        }

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("OpenSearch Service: {id}"),
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

const _: f64 = HOURS_PER_MONTH;
