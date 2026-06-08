use serde::{Deserialize, Serialize};
use yevice_core::{
    cost::{CostComponent, ResourceCost, VariableInfo},
    expr::Expr,
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

/// AWS CloudTrail trail (`AWS::CloudTrail::Trail`).
///
/// The first copy of management events per region is free; additional copies
/// (e.g. a second/third trail, or multi-region duplicates) are billed per
/// 100,000 events, as are data events. Both are usage-driven.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CloudTrailSpec {}

pub struct CloudTrailService;

impl Service for CloudTrailService {
    type Spec = CloudTrailSpec;

    fn id(&self) -> &'static str {
        "aws.cloudtrail"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        _spec: &CloudTrailSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let data_event_price_100k =
            pricing.lookup_f64(&Sku::new("aws.cloudtrail.data_event_price_per_100k"))?;
        let mgmt_copy_price_100k = pricing.lookup_f64(&Sku::new(
            "aws.cloudtrail.management_event_copy_price_per_100k",
        ))?;

        // Data events delivered to S3, billed per 100,000 events.
        let data_events = Expr::linear(
            data_event_price_100k,
            Expr::variable(id.var("data_events_100k")),
            0.0,
        );
        // Additional management-event copies beyond the free first regional copy
        // (e.g. extra trails / multi-region duplicates), per 100,000 events.
        let mgmt_copies = Expr::linear(
            mgmt_copy_price_100k,
            Expr::variable(id.var("management_event_copies_100k")),
            0.0,
        );

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("CloudTrail: {id}"),
            expr: Expr::sum(vec![data_events.clone(), mgmt_copies.clone()]),
            components: vec![
                CostComponent {
                    name: "Data Events".into(),
                    expr: data_events,
                },
                CostComponent {
                    name: "Management Event Copies".into(),
                    expr: mgmt_copies,
                },
            ],
            required_variables: vec![
                VariableInfo::new(
                    id,
                    "data_events_100k",
                    "Data events delivered per month",
                    "100k events",
                ),
                VariableInfo::new(
                    id,
                    "management_event_copies_100k",
                    "Additional management-event copies (beyond the free first)",
                    "100k events",
                ),
            ],
        })
    }
}
