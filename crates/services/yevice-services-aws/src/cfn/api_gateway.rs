use crate::services::api_gateway::{ApiGatewaySpec, ApiGatewayType};
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};
pub struct ApiGatewayCfnAdapter;
impl CfnAdapter for ApiGatewayCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &[
            "AWS::ApiGateway::RestApi",
            "AWS::Serverless::Api",
            "AWS::ApiGatewayV2::Api",
            "AWS::Serverless::HttpApi",
        ]
    }
    fn convert(&self, raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        let api_type = match raw.resource_type.as_str() {
            "AWS::ApiGatewayV2::Api" | "AWS::Serverless::HttpApi" => ApiGatewayType::Http,
            _ => ApiGatewayType::Rest,
        };
        Ok(ResourceShell::new(
            "aws.api_gateway",
            Provider::Aws,
            &ApiGatewaySpec { api_type },
        ))
    }
}
