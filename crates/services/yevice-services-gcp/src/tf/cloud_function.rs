//! TF adapters for GCP Cloud Functions (gen1 and gen2).

use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::iac::{IacError, RawTfResource, TfAdapter};

use crate::services::cloud_function::GcpCloudFunctionSpec;

pub struct GcpCloudFunctionTfAdapter;

impl TfAdapter for GcpCloudFunctionTfAdapter {
    fn handles(&self) -> &[&'static str] {
        &[
            "google_cloudfunctions_function",
            "google_cloudfunctions2_function",
        ]
    }

    fn convert(&self, raw: &RawTfResource) -> Result<ResourceShell, IacError> {
        let spec = match raw.resource_type.as_str() {
            "google_cloudfunctions_function" => convert_v1(raw),
            _ => convert_v2(raw),
        };
        Ok(ResourceShell::new(
            "gcp.cloud_function",
            Provider::Gcp,
            &spec,
        ))
    }
}

fn convert_v1(raw: &RawTfResource) -> GcpCloudFunctionSpec {
    GcpCloudFunctionSpec {
        memory_mb: raw.get_f64("available_memory_mb").unwrap_or(256.0),
        generation: 1,
    }
}

fn convert_v2(raw: &RawTfResource) -> GcpCloudFunctionSpec {
    // Try service_config block first, then top-level attr
    let memory_mb = raw
        .get_block("service_config")
        .and_then(|b| b.get("available_memory"))
        .and_then(|v| v.as_str())
        .and_then(parse_memory_string)
        .or_else(|| {
            raw.get_str("available_memory")
                .and_then(parse_memory_string)
        })
        .or_else(|| raw.get_f64("available_memory_mb"))
        .unwrap_or(256.0);

    GcpCloudFunctionSpec {
        memory_mb,
        generation: 2,
    }
}

fn parse_memory_string(s: &str) -> Option<f64> {
    if let Some(n) = s.strip_suffix("Mi").or_else(|| s.strip_suffix('M')) {
        n.parse().ok()
    } else if let Some(n) = s.strip_suffix("Gi").or_else(|| s.strip_suffix('G')) {
        n.parse::<f64>().ok().map(|v| v * 1024.0)
    } else if let Some(n) = s.strip_suffix("Ki") {
        n.parse::<f64>().ok().map(|v| v / 1024.0)
    } else {
        s.parse().ok()
    }
}
