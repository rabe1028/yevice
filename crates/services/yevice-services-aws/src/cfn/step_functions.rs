use crate::services::step_functions::{StepFunctionsSpec, StepFunctionsType};
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};
pub struct StepFunctionsCfnAdapter;
impl CfnAdapter for StepFunctionsCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &[
            "AWS::StepFunctions::StateMachine",
            "AWS::Serverless::StateMachine",
        ]
    }
    fn convert(&self, raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        let is_express = raw
            .get_str("Type")
            .is_some_and(|t| t.eq_ignore_ascii_case("express"));
        let spec = StepFunctionsSpec {
            workflow_type: if is_express {
                StepFunctionsType::Express
            } else {
                StepFunctionsType::Standard
            },
        };
        let shell = ResourceShell::new("aws.step_functions", Provider::Aws, &spec).with_metadata(
            "workflow_type",
            match spec.workflow_type {
                StepFunctionsType::Standard => "standard",
                StepFunctionsType::Express => "express",
            },
        );
        Ok(shell)
    }
}
