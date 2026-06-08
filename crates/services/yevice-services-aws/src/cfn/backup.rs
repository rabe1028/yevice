use crate::services::backup::BackupSpec;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};

pub struct BackupCfnAdapter;

impl CfnAdapter for BackupCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::Backup::BackupVault"]
    }

    fn convert(&self, raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        // The protected-resource engine (and thus the warm-storage rate) is not
        // declared on the vault itself; default to EBS, overridable via the
        // `BackupType` property when present in the template.
        let spec = BackupSpec {
            backup_type: raw.get_str("BackupType").unwrap_or("ebs").to_string(),
        };
        Ok(ResourceShell::new("aws.backup", Provider::Aws, &spec))
    }
}
