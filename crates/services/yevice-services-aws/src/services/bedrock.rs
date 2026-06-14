use serde::{Deserialize, Serialize};
use yevice_core::{
    cost::{CostComponent, ResourceCost, VariableInfo},
    expr::Expr,
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

/// Amazon Bedrock foundation-model invocation cost (usage-driven).
///
/// Cost is driven entirely by usage variables (input/output tokens), so the
/// spec carries no structural fields. A placeholder CFN resource
/// (`AWS::Bedrock::Agent`) emits this shell; the actual token volumes come
/// from `usage.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BedrockSpec {}

pub struct BedrockService;

impl Service for BedrockService {
    type Spec = BedrockSpec;

    fn id(&self) -> &'static str {
        "aws.bedrock"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        _spec: &BedrockSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        // Anthropic Claude 3 / 3.5 Sonnet on-demand rates, billed per 1,000 tokens.
        let input_price_k =
            pricing.lookup_f64(&Sku::new("aws.bedrock.input_token_price_per_1k"))?;
        let output_price_k =
            pricing.lookup_f64(&Sku::new("aws.bedrock.output_token_price_per_1k"))?;

        // Usage variables are expressed in thousands of tokens, so the per-1K
        // rate is the linear coefficient directly.
        let input_cost = Expr::linear(input_price_k, Expr::variable(id.var("input_ktokens")), 0.0);
        let output_cost = Expr::linear(
            output_price_k,
            Expr::variable(id.var("output_ktokens")),
            0.0,
        );

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("Bedrock: {id} (Claude 3.5 Sonnet)"),
            expr: Expr::sum(vec![input_cost.clone(), output_cost.clone()]),
            components: vec![
                CostComponent {
                    name: "Input Tokens".into(),
                    expr: input_cost,

                    currency: None,
                },
                CostComponent {
                    name: "Output Tokens".into(),
                    expr: output_cost,

                    currency: None,
                },
            ],
            required_variables: vec![
                VariableInfo::new(
                    id,
                    "input_ktokens",
                    "Input tokens processed per month",
                    "thousand tokens",
                ),
                VariableInfo::new(
                    id,
                    "output_ktokens",
                    "Output tokens generated per month",
                    "thousand tokens",
                ),
            ],

            currency: Some("USD".into()),
        })
    }
}
