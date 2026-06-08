use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{IacError, RawTfResource, TfAdapter};

use crate::services::dynamodb::{DynamoDbBillingMode, DynamoDbSpec};

pub struct DynamoDbTfAdapter;

impl TfAdapter for DynamoDbTfAdapter {
    fn handles(&self) -> &[&'static str] {
        &["aws_dynamodb_table"]
    }

    fn convert(&self, raw: &RawTfResource) -> Result<ResourceShell, IacError> {
        let billing_mode = match raw.get_str("billing_mode") {
            Some(mode) if mode.eq_ignore_ascii_case("PAY_PER_REQUEST") => {
                DynamoDbBillingMode::OnDemand
            }
            _ => DynamoDbBillingMode::Provisioned {
                write_capacity_units: raw.get_f64("write_capacity"),
                read_capacity_units: raw.get_f64("read_capacity"),
            },
        };

        let has_stream = raw.get_bool("stream_enabled").unwrap_or(false)
            || raw
                .get_str("stream_enabled")
                .is_some_and(|v| v.eq_ignore_ascii_case("true"))
            || raw.blocks.contains_key("stream_specification");

        let gsi_count = raw.blocks.get("global_secondary_index").map_or(0, Vec::len);

        let spec = DynamoDbSpec {
            billing_mode: billing_mode.clone(),
            has_stream,
            gsi_count,
        };
        let shell = ResourceShell::new("aws.dynamodb", Provider::Aws, &spec);
        let shell = if matches!(billing_mode, DynamoDbBillingMode::OnDemand) {
            shell.with_metadata("billing_mode", "on_demand")
        } else {
            shell.with_metadata("billing_mode", "provisioned")
        };
        Ok(shell)
    }
}
