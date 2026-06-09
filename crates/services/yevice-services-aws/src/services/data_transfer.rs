use serde::{Deserialize, Serialize};
use yevice_core::{
    cost::{CostComponent, ResourceCost, VariableInfo},
    expr::Expr,
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

/// Standalone AWS Data Transfer charges (ap-northeast-1 / Tokyo).
///
/// Usage-only: AWS exposes no first-class CloudFormation resource for raw data
/// transfer, so this attaches to a marker resource (see `cfn::data_transfer`).
///
/// Two usage-driven dimensions:
/// - Internet egress (data transfer out to the internet), priced with the
///   shared tiered SKU `aws.data_transfer.egress_tiers` (first GB free, then
///   $0.114/GB to 10 TB, etc.). This intentionally reuses the same tier table
///   that `common::egress_cost_expr` consumes, so the model is not duplicated.
/// - Inter-region transfer out (e.g. Tokyo -> Osaka ap-northeast-3), a flat
///   per-GB rate via `aws.data_transfer.inter_region_price_per_gb`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DataTransferSpec {}

pub struct DataTransferService;

impl Service for DataTransferService {
    type Spec = DataTransferSpec;

    fn id(&self) -> &'static str {
        "aws.data_transfer"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        _spec: &DataTransferSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        // Internet egress: reuse the shared tiered SKU rather than redefining
        // the tier table (complements common::egress_cost_expr).
        let egress_record = pricing.lookup(&Sku::new("aws.data_transfer.egress_tiers"))?;
        let egress_tiers = egress_record.as_tiered().map_err(CostError::Pricing)?;
        let internet_egress = Expr::tiered(
            egress_tiers.to_vec(),
            Expr::variable(id.var("internet_egress_gb")),
        );

        // Inter-region transfer out: flat per-GB rate (Tokyo -> another region).
        let inter_region_price =
            pricing.lookup_f64(&Sku::new("aws.data_transfer.inter_region_price_per_gb"))?;
        let inter_region = Expr::linear(
            inter_region_price,
            Expr::variable(id.var("inter_region_gb")),
            0.0,
        );

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("Data Transfer: {id}"),
            expr: Expr::sum(vec![internet_egress.clone(), inter_region.clone()]),
            components: vec![
                CostComponent {
                    name: "Internet Egress".into(),
                    expr: internet_egress,
                },
                CostComponent {
                    name: "Inter-Region Transfer".into(),
                    expr: inter_region,
                },
            ],
            required_variables: vec![
                VariableInfo::new(
                    id,
                    "internet_egress_gb",
                    "Data transfer out to the internet per month",
                    "GB",
                ),
                VariableInfo::new(
                    id,
                    "inter_region_gb",
                    "Inter-region data transfer out per month",
                    "GB",
                ),
            ],
        })
    }
}
