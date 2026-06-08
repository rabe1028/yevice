//! TF adapters for GCP Cloud Run.

use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::iac::{IacError, RawTfResource, TfAdapter};

use crate::services::cloud_run::GcpCloudRunSpec;

pub struct GcpCloudRunTfAdapter;

impl TfAdapter for GcpCloudRunTfAdapter {
    fn handles(&self) -> &[&'static str] {
        &["google_cloud_run_service", "google_cloud_run_v2_service"]
    }

    fn convert(&self, raw: &RawTfResource) -> Result<ResourceShell, IacError> {
        // cpu/memory for Cloud Run are in deeply nested blocks (template→containers→resources→limits)
        // which the single-level block parser cannot reach; default values are used.
        let cpu = raw.get_str("cpu").and_then(parse_cpu_string).or_else(|| {
            raw.get_block("template")
                .and_then(|b| b.get("cpu"))
                .and_then(|v| v.as_str())
                .and_then(parse_cpu_string)
        });

        let memory_mb = raw
            .get_str("memory")
            .and_then(parse_memory_string)
            .or_else(|| {
                raw.get_block("template")
                    .and_then(|b| b.get("memory"))
                    .and_then(|v| v.as_str())
                    .and_then(parse_memory_string)
            });

        if cpu.is_none() || memory_mb.is_none() {
            tracing::warn!(
                resource = %raw.logical_id,
                "Cloud Run cpu/memory not found in top-level attributes; using defaults (1 vCPU, 512 MB)."
            );
        }

        // `scaling { min_instance_count }` is normally nested under `template`
        // (v2) or a top-level `scaling` block; the nested-block-aware parser
        // merges those into the enclosing block's attributes.
        let min_instances = raw
            .get_f64("min_instance_count")
            .or_else(|| raw.get_f64("min_instances"))
            .or_else(|| nested_f64(raw, "template", "min_instance_count"))
            .or_else(|| nested_f64(raw, "scaling", "min_instance_count"))
            .or_else(|| nested_f64(raw, "template", "min_instances"));

        let spec = GcpCloudRunSpec {
            cpu: cpu.unwrap_or(1.0),
            memory_mb: memory_mb.unwrap_or(512.0),
            min_instances,
        };

        Ok(ResourceShell::new("gcp.cloud_run", Provider::Gcp, &spec))
    }
}

/// Read a numeric attribute from a (nested-merged) block by name.
fn nested_f64(raw: &RawTfResource, block: &str, key: &str) -> Option<f64> {
    raw.get_block(block).and_then(|b| match b.get(key)? {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.parse().ok(),
        _ => None,
    })
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

fn parse_cpu_string(s: &str) -> Option<f64> {
    if let Some(n) = s.strip_suffix('m') {
        n.parse::<f64>().ok().map(|v| v / 1000.0)
    } else {
        s.parse().ok()
    }
}
