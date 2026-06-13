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
pub struct KinesisFirehoseSpec {}

pub struct FirehoseService;

impl Service for FirehoseService {
    type Spec = KinesisFirehoseSpec;

    fn id(&self) -> &'static str {
        "aws.firehose"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        _spec: &KinesisFirehoseSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let ingestion_price =
            pricing.lookup_f64(&Sku::new("aws.firehose.ingestion_price_per_gb"))?;
        let ingestion = Expr::linear(ingestion_price, Expr::variable(id.var("ingestion_gb")), 0.0);

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("Kinesis Firehose: {id}"),
            expr: ingestion.clone(),
            components: vec![CostComponent {
                name: "Data Ingestion".into(),
                expr: ingestion,

                currency: None,
            }],
            required_variables: vec![VariableInfo::new(
                id,
                "ingestion_gb",
                "Data ingested per month",
                "GB",
            )],

            currency: Some("USD".into()),
        })
    }
}
