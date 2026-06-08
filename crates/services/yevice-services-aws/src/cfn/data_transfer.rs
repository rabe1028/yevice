use crate::services::data_transfer::DataTransferSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};

/// CloudFormation adapter for standalone data transfer.
///
/// AWS has no first-class data-transfer resource, so this binds to a custom
/// marker type (`Yevice::DataTransfer`) that lets a template declare data
/// transfer usage explicitly. All cost is usage-driven; the marker carries no
/// CFN properties.
pub struct DataTransferCfnAdapter;

impl CfnAdapter for DataTransferCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["Yevice::DataTransfer"]
    }

    fn convert(&self, _raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        Ok(ResourceShell::new(
            "aws.data_transfer",
            Provider::Aws,
            &DataTransferSpec {},
        ))
    }
}
