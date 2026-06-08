use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};

use crate::services::ec2::{Ec2Os, Ec2Spec};

pub struct Ec2CfnAdapter;

impl CfnAdapter for Ec2CfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::EC2::Instance"]
    }

    fn convert(&self, raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        let instance_type = raw
            .get_str("InstanceType")
            .unwrap_or("t3.micro")
            .to_string();
        let os = if is_windows(raw) {
            Ec2Os::Windows
        } else {
            Ec2Os::Linux
        };
        Ok(ResourceShell::new(
            "aws.ec2",
            Provider::Aws,
            &Ec2Spec { instance_type, os },
        ))
    }
}

/// Decide whether an `AWS::EC2::Instance` runs Windows.
///
/// LIMITATION: the operating system lives in the AMI, and `AWS::EC2::Instance`
/// has no OS property. yevice cannot resolve an opaque `ami-0123...` ID to its
/// OS, so such instances are priced as **Linux by default**. To bill Windows
/// licensing, give an explicit hint that this looks for:
///   - a `Platform: windows` property (yevice convention), or
///   - an `ImageId` whose value contains `windows` — covers descriptive AMI
///     names and SSM public-parameter paths such as
///     `/aws/service/ami-windows-latest/...` (incl. `{{resolve:ssm:...}}`).
fn is_windows(raw: &RawCfnResource) -> bool {
    if raw
        .get_str("Platform")
        .is_some_and(|p| p.eq_ignore_ascii_case("windows"))
    {
        return true;
    }
    raw.get_str("ImageId")
        .is_some_and(|i| i.to_ascii_lowercase().contains("windows"))
}
