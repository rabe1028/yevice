use serde_json::Value;
use yevice_core::resource::{Provider, ResourceShell};
use yevice_service_api::{CfnAdapter, IacError, RawCfnResource};

use crate::services::dynamodb::{DynamoDbBillingMode, DynamoDbSpec};

pub struct DynamoDbCfnAdapter;

/// Parse a JSON value as `f64`, accepting both numeric and numeric-string forms.
///
/// CloudFormation `Parameters:` of `Type: Number` are resolved into JSON
/// strings (e.g. `"5"`), so `Value::as_f64` alone drops them. Mirroring the
/// behaviour of `RawCfnResource::get_f64` keeps fixed throughput values from
/// being silently lost.
fn value_as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse::<f64>().ok(),
        _ => None,
    }
}

impl CfnAdapter for DynamoDbCfnAdapter {
    fn handles(&self) -> &[&'static str] {
        &["AWS::DynamoDB::Table", "AWS::DynamoDB::GlobalTable"]
    }

    fn convert(&self, raw: &RawCfnResource) -> Result<ResourceShell, IacError> {
        let billing_mode_str = raw.get_str("BillingMode").unwrap_or("PROVISIONED");

        let billing_mode = if billing_mode_str == "PAY_PER_REQUEST" {
            DynamoDbBillingMode::OnDemand
        } else {
            let wcu = raw
                .get_object("ProvisionedThroughput")
                .and_then(|v| v.get("WriteCapacityUnits"))
                .and_then(value_as_f64);
            let rcu = raw
                .get_object("ProvisionedThroughput")
                .and_then(|v| v.get("ReadCapacityUnits"))
                .and_then(value_as_f64);
            DynamoDbBillingMode::Provisioned {
                write_capacity_units: wcu,
                read_capacity_units: rcu,
            }
        };

        let has_stream = raw.get_object("StreamSpecification").is_some();

        let gsi_count = raw
            .get_object("GlobalSecondaryIndexes")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);

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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Regression: CloudFormation `Parameters:` of `Type: Number` resolve to
    /// JSON strings (e.g. `"5"`). The adapter must still recognise the fixed
    /// throughput values instead of treating them as missing.
    #[test]
    fn parses_provisioned_throughput_from_strings() {
        let raw = RawCfnResource::new(
            "MyTable",
            "AWS::DynamoDB::Table",
            json!({
                "BillingMode": "PROVISIONED",
                "ProvisionedThroughput": {
                    "WriteCapacityUnits": "5",
                    "ReadCapacityUnits": "10"
                }
            }),
        );

        let shell = DynamoDbCfnAdapter.convert(&raw).expect("convert ok");
        let spec: DynamoDbSpec = shell.decode().expect("decode spec");
        match spec.billing_mode {
            DynamoDbBillingMode::Provisioned {
                write_capacity_units,
                read_capacity_units,
            } => {
                assert_eq!(write_capacity_units, Some(5.0));
                assert_eq!(read_capacity_units, Some(10.0));
            }
            other @ DynamoDbBillingMode::OnDemand => {
                panic!("expected provisioned billing mode, got {other:?}")
            }
        }
    }
}
