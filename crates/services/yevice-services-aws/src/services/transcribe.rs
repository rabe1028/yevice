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
pub struct TranscribeSpec {}

pub struct TranscribeService;

impl Service for TranscribeService {
    type Spec = TranscribeSpec;

    fn id(&self) -> &'static str {
        "aws.transcribe"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        _spec: &TranscribeSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let minute_price =
            pricing.lookup_f64(&Sku::new("aws.transcribe.standard_batch_price_per_minute"))?;
        // Standard batch transcription is billed per minute of audio processed.
        let cost = Expr::linear(minute_price, Expr::variable(id.var("minutes")), 0.0);

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("Transcribe: {id}"),
            expr: cost.clone(),
            components: vec![CostComponent {
                name: "Standard Batch Transcription".into(),
                expr: cost,
            }],
            required_variables: vec![VariableInfo::new(
                id,
                "minutes",
                "Audio transcribed per month",
                "minutes",
            )],
        })
    }
}
