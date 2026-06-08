use crate::services::directory_service::DirectoryServiceSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};

pub struct DirectoryServiceCfnAdapter;

impl CfnAdapter for DirectoryServiceCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::DirectoryService::MicrosoftAD"]
    }

    fn convert(&self, raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        // `Edition` defaults to Enterprise when omitted (AWS default).
        // AWS Managed Microsoft AD always provisions at least two domain
        // controllers, so default to 2 rather than leaving a usage placeholder
        // that would otherwise price the directory at $0. A template may add
        // more via the optional (yevice) `DomainControllers` property.
        let domain_controllers = Some(raw.get_f64("DomainControllers").unwrap_or(2.0).max(2.0));
        let spec = DirectoryServiceSpec {
            edition: raw.get_str("Edition").unwrap_or("Enterprise").to_string(),
            domain_controllers,
        };
        Ok(ResourceShell::new(
            "aws.directory_service",
            Provider::Aws,
            &spec,
        ))
    }
}
