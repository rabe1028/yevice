use serde::{Deserialize, Serialize};
use yevice_core::{
    cost::{CostComponent, ResourceCost, VariableInfo},
    expr::Expr,
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AthenaSpec {}

pub struct AthenaService;

impl Service for AthenaService {
    type Spec = AthenaSpec;

    fn id(&self) -> &'static str {
        "aws.athena"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        _spec: &AthenaSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let scan_price_tb = pricing.lookup_f64(&Sku::new("aws.athena.scan_price_per_tb"))?;
        // convert to per-GB for user input
        let cost = Expr::linear(
            scan_price_tb / 1_000.0,
            Expr::variable(id.var("scan_gb")),
            0.0,
        );

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("Athena: {id}"),
            expr: cost.clone(),
            components: vec![CostComponent {
                name: "Data Scanned".into(),
                expr: cost,

                currency: None,
            }],
            required_variables: vec![VariableInfo::new(
                id,
                "scan_gb",
                "Data scanned per month",
                "GB",
            )],

            currency: Some("USD".into()),
        })
    }
}
