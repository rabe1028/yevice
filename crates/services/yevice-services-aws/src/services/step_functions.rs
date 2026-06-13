use serde::{Deserialize, Serialize};
use yevice_core::{
    cost::{CostComponent, ResourceCost, VariableInfo},
    expr::{Expr, Tier},
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StepFunctionsType {
    Standard,
    Express,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepFunctionsSpec {
    pub workflow_type: StepFunctionsType,
}

pub struct StepFunctionsService;

impl Service for StepFunctionsService {
    type Spec = StepFunctionsSpec;

    fn id(&self) -> &'static str {
        "aws.step_functions"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &StepFunctionsSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let standard_price =
            pricing.lookup_f64(&Sku::new("aws.step_functions.standard_transition_price"))?;
        let express_req_price =
            pricing.lookup_f64(&Sku::new("aws.step_functions.express_request_price"))?;
        let express_dur_price = pricing.lookup_f64(&Sku::new(
            "aws.step_functions.express_duration_price_per_gb_second",
        ))?;
        let free_transitions =
            pricing.lookup_f64(&Sku::new("aws.step_functions.free_tier_transitions"))?;

        match spec.workflow_type {
            StepFunctionsType::Express => {
                let sfn_requests =
                    Expr::linear(express_req_price, Expr::variable(id.var("requests")), 0.0);
                let sfn_duration = Expr::linear(
                    express_dur_price,
                    Expr::variable(id.var("duration_gb_seconds")),
                    0.0,
                );
                Ok(ResourceCost {
                    logical_id: id.clone(),
                    resource_type: rt.clone(),
                    label: format!("Step Functions Express: {id}"),
                    expr: Expr::sum(vec![sfn_requests.clone(), sfn_duration.clone()]),
                    components: vec![
                        CostComponent {
                            name: "Requests".into(),
                            expr: sfn_requests,

                            currency: None,
                        },
                        CostComponent {
                            name: "Duration".into(),
                            expr: sfn_duration,

                            currency: None,
                        },
                    ],
                    required_variables: vec![
                        VariableInfo::new(id, "requests", "Requests per month", "requests"),
                        VariableInfo::new(
                            id,
                            "duration_gb_seconds",
                            "Duration in GB-seconds",
                            "GB-seconds",
                        ),
                    ],

                    currency: Some("USD".into()),
                })
            }
            StepFunctionsType::Standard => Ok(ResourceCost {
                logical_id: id.clone(),
                resource_type: rt.clone(),
                label: format!("Step Functions Standard: {id}"),
                expr: Expr::tiered(
                    vec![
                        Tier {
                            upper_limit: Some(free_transitions),
                            unit_price: 0.0,
                        },
                        Tier {
                            upper_limit: None,
                            unit_price: standard_price,
                        },
                    ],
                    Expr::variable(id.var("transitions")),
                ),
                components: vec![],
                required_variables: vec![VariableInfo::new(
                    id,
                    "transitions",
                    "State transitions per month",
                    "transitions",
                )],

                currency: Some("USD".into()),
            }),
        }
    }
}
