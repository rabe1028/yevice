//! TF adapter for GCP Pub/Sub.

use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::iac::{IacError, RawTfResource, TfAdapter};

use crate::services::pubsub::GcpPubSubSpec;

pub struct GcpPubSubTfAdapter;

impl TfAdapter for GcpPubSubTfAdapter {
    fn handles(&self) -> &[&'static str] {
        &["google_pubsub_topic"]
    }

    fn convert(&self, _raw: &RawTfResource) -> Result<ResourceShell, IacError> {
        Ok(ResourceShell::new(
            "gcp.pubsub",
            Provider::Gcp,
            &GcpPubSubSpec {},
        ))
    }
}
