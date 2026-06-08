use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};

use crate::services::quicksight::QuickSightSpec;

pub struct QuickSightCfnAdapter;

impl CfnAdapter for QuickSightCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        // QuickSight billing is account-level (creator/viewer subscriptions +
        // SPICE) and is NOT modelled by any CloudFormation resource — the real
        // AWS::QuickSight::Analysis/Dashboard/DataSet/Template resources are
        // free, so they are intentionally NOT handled here (they incur no cost).
        // The account cost is supplied exactly once via a single
        // `Yevice::QuickSight` marker (same pattern as `Yevice::DataTransfer`),
        // which avoids both double-counting and dropping the cost.
        &["Yevice::QuickSight"]
    }

    fn convert(&self, _raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        let spec = QuickSightSpec {
            creators: None,
            viewer_users: None,
            sessions_per_user: None,
            spice_gb: None,
            is_cost_anchor: true,
        };
        Ok(ResourceShell::new("aws.quicksight", Provider::Aws, &spec))
    }
}
