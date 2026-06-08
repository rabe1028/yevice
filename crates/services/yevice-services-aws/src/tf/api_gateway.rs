use crate::services::api_gateway::{ApiGatewaySpec, ApiGatewayType};
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{IacError, RawTfResource, TfAdapter};
pub struct ApiGatewayTfAdapter;
impl TfAdapter for ApiGatewayTfAdapter {
    fn handles(&self) -> &[&'static str] {
        &["aws_api_gateway_rest_api", "aws_apigatewayv2_api"]
    }
    fn convert(&self, raw: &RawTfResource) -> Result<ResourceShell, IacError> {
        let api_type = if raw.resource_type.as_str() == "aws_apigatewayv2_api" {
            match raw.get_str("protocol_type") {
                Some(p) if p.eq_ignore_ascii_case("HTTP") => ApiGatewayType::Http,
                _ => ApiGatewayType::Rest,
            }
        } else {
            ApiGatewayType::Rest
        };
        Ok(ResourceShell::new(
            "aws.api_gateway",
            Provider::Aws,
            &ApiGatewaySpec { api_type },
        ))
    }
}
