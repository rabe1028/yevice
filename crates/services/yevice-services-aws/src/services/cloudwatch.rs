use serde::{Deserialize, Serialize};
use yevice_core::{
    cost::{CostComponent, ResourceCost},
    expr::Expr,
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

/// Amazon CloudWatch alarms (`AWS::CloudWatch::Alarm`).
///
/// Each `AWS::CloudWatch::Alarm` resource is one standard-resolution alarm by
/// default. A template may set an optional `AlarmCount` property to have a
/// single resource stand in for several identical alarms (handy when matching
/// a published estimate that aggregates "N alarms" into one line). The count is
/// fixed at generate time — there is no usage variable, so adding more alarm
/// resources never double-counts.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CloudWatchSpec {
    /// Alarms represented by this resource (defaults to 1).
    pub alarm_count: Option<f64>,
}

pub struct CloudWatchService;

impl Service for CloudWatchService {
    type Spec = CloudWatchSpec;

    fn id(&self) -> &'static str {
        "aws.cloudwatch"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &CloudWatchSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let alarm_price = pricing.lookup_f64(&Sku::new("aws.cloudwatch.alarm_month_price"))?;
        let count = spec.alarm_count.unwrap_or(1.0);
        let alarm = Expr::constant(alarm_price * count);

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("CloudWatch Alarm: {id} (x{count})"),
            expr: alarm.clone(),
            components: vec![CostComponent {
                name: "Alarms".into(),
                expr: alarm,

                currency: None,
            }],
            required_variables: vec![],

            currency: Some("USD".into()),
        })
    }
}
