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
pub struct S3Spec {
    pub versioning_enabled: bool,
    pub storage_class: Option<String>,
}

pub struct S3Service;

impl Service for S3Service {
    type Spec = S3Spec;

    fn id(&self) -> &'static str {
        "aws.s3"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        _spec: &S3Spec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let put_request_price = pricing.lookup_f64(&Sku::new("aws.s3.put_request_price"))?;
        let get_request_price = pricing.lookup_f64(&Sku::new("aws.s3.get_request_price"))?;
        let storage_tiers = pricing
            .lookup(&Sku::new("aws.s3.storage_tiers"))?
            .as_tiered()
            .map_err(CostError::Pricing)?
            .to_vec();

        let storage = Expr::tiered(storage_tiers, Expr::variable(id.var(var::STORAGE_GB)));
        let put = Expr::linear(
            put_request_price,
            Expr::variable(id.var("put_requests")),
            0.0,
        );
        let get = Expr::linear(
            get_request_price,
            Expr::variable(id.var("get_requests")),
            0.0,
        );

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("S3: {id}"),
            expr: Expr::sum(vec![storage.clone(), put.clone(), get.clone()]),
            components: vec![
                CostComponent {
                    name: "Storage".into(),
                    expr: storage,
                },
                CostComponent {
                    name: "PUT requests".into(),
                    expr: put,
                },
                CostComponent {
                    name: "GET requests".into(),
                    expr: get,
                },
            ],
            required_variables: vec![
                VariableInfo::new(id, var::STORAGE_GB, "Storage amount", "GB"),
                VariableInfo::new(
                    id,
                    "put_requests",
                    "PUT/POST/COPY requests per month",
                    "requests",
                ),
                VariableInfo::new(id, "get_requests", "GET requests per month", "requests"),
            ],
        })
    }
}
