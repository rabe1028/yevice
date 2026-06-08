use serde_json::Value;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};

use crate::services::batch::{BatchEbsConfig, BatchJobDefinitionSpec, BatchLaunchType};

pub struct BatchCfnAdapter;

impl CfnAdapter for BatchCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::Batch::JobDefinition"]
    }

    fn convert(&self, raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        let container = raw.get_object("ContainerProperties");

        let platform = raw
            .get_object("PlatformCapabilities")
            .and_then(Value::as_array)
            .and_then(|arr| arr.first())
            .and_then(Value::as_str)
            .or_else(|| raw.get_str("PlatformCapabilities"));

        let launch_type = if platform.is_some_and(|p| p == "FARGATE") {
            BatchLaunchType::Fargate
        } else {
            BatchLaunchType::Ec2
        };

        let get_f64_from = |v: &Value, key: &str| -> Option<f64> {
            v.get(key)?
                .as_f64()
                .or_else(|| v.get(key)?.as_str()?.parse().ok())
        };

        let vcpu = container
            .and_then(|c| {
                get_f64_from(c, "Vcpus").or_else(|| {
                    c.get("ResourceRequirements")
                        .and_then(Value::as_array)
                        .and_then(|reqs| {
                            reqs.iter().find_map(|r| {
                                if r.get("Type")?.as_str()? == "VCPU" {
                                    r.get("Value")?.as_str()?.parse().ok()
                                } else {
                                    None
                                }
                            })
                        })
                })
            })
            .unwrap_or(1.0);

        let memory_mb = container
            .and_then(|c| {
                get_f64_from(c, "Memory").or_else(|| {
                    c.get("ResourceRequirements")
                        .and_then(Value::as_array)
                        .and_then(|reqs| {
                            reqs.iter().find_map(|r| {
                                if r.get("Type")?.as_str()? == "MEMORY" {
                                    r.get("Value")?.as_str()?.parse().ok()
                                } else {
                                    None
                                }
                            })
                        })
                })
            })
            .unwrap_or(2048.0);

        let ephemeral_storage = container
            .and_then(|c| c.get("EphemeralStorage"))
            .and_then(|v| get_f64_from(v, "SizeInGiB"));

        let ebs = container
            .and_then(|c| c.get("Volumes"))
            .and_then(Value::as_array)
            .and_then(|vols| {
                vols.iter().find_map(|vol| {
                    let ebs_vol = vol.get("EbsVolume")?;
                    Some(BatchEbsConfig {
                        size_gb: get_f64_from(ebs_vol, "SizeInGiB").unwrap_or(100.0),
                        volume_type: ebs_vol
                            .get("VolumeType")
                            .and_then(Value::as_str)
                            .unwrap_or("gp3")
                            .to_string(),
                        iops: get_f64_from(ebs_vol, "Iops"),
                        throughput_mibps: get_f64_from(ebs_vol, "Throughput"),
                    })
                })
            });

        let spec = BatchJobDefinitionSpec {
            launch_type,
            vcpu,
            memory_gb: memory_mb / 1024.0,
            ephemeral_storage_gb: ephemeral_storage,
            ebs,
        };
        Ok(ResourceShell::new("aws.batch", Provider::Aws, &spec))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Regression: CloudFormation `Parameters:` of `Type: Number` resolve to
    /// JSON strings (e.g. `"100"`). The adapter must still surface the
    /// configured ephemeral storage size instead of falling back to the 20 GB
    /// default downstream.
    #[test]
    fn parses_ephemeral_storage_from_string() {
        let raw = RawCfnResource::new(
            "MyJob",
            "AWS::Batch::JobDefinition",
            json!({
                "PlatformCapabilities": ["FARGATE"],
                "ContainerProperties": {
                    "ResourceRequirements": [
                        {"Type": "VCPU", "Value": "1"},
                        {"Type": "MEMORY", "Value": "2048"}
                    ],
                    "EphemeralStorage": {
                        "SizeInGiB": "100"
                    }
                }
            }),
        );

        let shell = BatchCfnAdapter.convert(&raw).expect("convert ok");
        let spec: BatchJobDefinitionSpec = shell.decode().expect("decode spec");
        assert_eq!(spec.ephemeral_storage_gb, Some(100.0));
    }

    /// Regression: same string-parameter issue for `Volumes[].EbsVolume`
    /// fields (`SizeInGiB`, `Iops`, `Throughput`). Without the fix, the
    /// adapter silently fell back to the 100 GB default and dropped IOPS
    /// / throughput overrides for parameterized job definitions.
    #[test]
    fn parses_ebs_volume_from_strings() {
        let raw = RawCfnResource::new(
            "MyJob",
            "AWS::Batch::JobDefinition",
            json!({
                "PlatformCapabilities": ["FARGATE"],
                "ContainerProperties": {
                    "ResourceRequirements": [
                        {"Type": "VCPU", "Value": "1"},
                        {"Type": "MEMORY", "Value": "2048"}
                    ],
                    "Volumes": [{
                        "EbsVolume": {
                            "SizeInGiB": "500",
                            "VolumeType": "gp3",
                            "Iops": "6000",
                            "Throughput": "250"
                        }
                    }]
                }
            }),
        );

        let shell = BatchCfnAdapter.convert(&raw).expect("convert ok");
        let spec: BatchJobDefinitionSpec = shell.decode().expect("decode spec");
        let ebs = spec.ebs.expect("ebs config present");
        assert_eq!(ebs.size_gb, 500.0);
        assert_eq!(ebs.volume_type, "gp3");
        assert_eq!(ebs.iops, Some(6000.0));
        assert_eq!(ebs.throughput_mibps, Some(250.0));
    }
}
