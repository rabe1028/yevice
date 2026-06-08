use crate::services::step_functions::{StepFunctionsSpec, StepFunctionsType};
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{IacError, RawTfResource, TfAdapter};
pub struct StepFunctionsTfAdapter;
impl TfAdapter for StepFunctionsTfAdapter {
    fn handles(&self) -> &[&'static str] {
        &["aws_sfn_state_machine"]
    }
    fn convert(&self, raw: &RawTfResource) -> Result<ResourceShell, IacError> {
        let wf = match raw.get_str("type") {
            Some(k) if k.eq_ignore_ascii_case("EXPRESS") => StepFunctionsType::Express,
            _ => StepFunctionsType::Standard,
        };
        let shell = ResourceShell::new(
            "aws.step_functions",
            Provider::Aws,
            &StepFunctionsSpec {
                workflow_type: wf.clone(),
            },
        )
        .with_metadata(
            "workflow_type",
            match wf {
                StepFunctionsType::Standard => "standard",
                StepFunctionsType::Express => "express",
            },
        );
        Ok(shell)
    }
}
