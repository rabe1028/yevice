use serde::{Deserialize, Serialize};
use yevice_core::{
    cost::{CostComponent, ResourceCost, VariableInfo},
    expr::Expr,
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

/// Amazon GuardDuty detector (`AWS::GuardDuty::Detector`).
///
/// GuardDuty is billed purely on usage; the detector resource is just a marker
/// that enables the service. Costs are driven by the volume of foundational
/// data sources analyzed:
///   * CloudTrail management events (priced per 1M events), and
///   * VPC Flow Logs + DNS query logs (priced per GB, volume-tiered).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GuardDutySpec {}

pub struct GuardDutyService;

/// CloudTrail management events are billed per individual event, but users think
/// in millions of events. We expose `cloudtrail_events_millions` and convert.
const EVENTS_PER_MILLION: f64 = 1_000_000.0;

impl Service for GuardDutyService {
    type Spec = GuardDutySpec;

    fn id(&self) -> &'static str {
        "aws.guardduty"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        _spec: &GuardDutySpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        // CloudTrail management-event analysis: flat per-event rate, charged per
        // 1M events. Convert the per-event price to a per-million coefficient.
        let cloudtrail_event_price =
            pricing.lookup_f64(&Sku::new("aws.guardduty.cloudtrail_event_price"))?;
        let cloudtrail = Expr::linear(
            cloudtrail_event_price * EVENTS_PER_MILLION,
            Expr::variable(id.var("cloudtrail_events_millions")),
            0.0,
        );

        // VPC Flow Logs + DNS query log analysis: volume-tiered per-GB pricing.
        let flowlog_tiers = pricing
            .lookup(&Sku::new("aws.guardduty.flowlog_dns_gb_tiers"))?
            .as_tiered()
            .map_err(CostError::Pricing)?;
        let flowlog = Expr::tiered(flowlog_tiers, Expr::variable(id.var("flowlog_gb")));

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("GuardDuty: {id}"),
            expr: Expr::sum(vec![cloudtrail.clone(), flowlog.clone()]),
            components: vec![
                CostComponent {
                    name: "CloudTrail Event Analysis".into(),
                    expr: cloudtrail,

                    currency: None,
                },
                CostComponent {
                    name: "VPC Flow Log & DNS Analysis".into(),
                    expr: flowlog,

                    currency: None,
                },
            ],
            required_variables: vec![
                VariableInfo::new(
                    id,
                    "cloudtrail_events_millions",
                    "CloudTrail management events analyzed per month",
                    "million events",
                ),
                VariableInfo::new(
                    id,
                    "flowlog_gb",
                    "VPC Flow Log + DNS query log data analyzed per month",
                    "GB",
                ),
            ],

            currency: Some("USD".into()),
        })
    }
}

const _: f64 = EVENTS_PER_MILLION;
