//! Shared helpers for AWS service cost implementations.

use yevice_core::{cost::VariableInfo, expr::Expr, types::LogicalId};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::error::CostError;

/// Build the egress (data-transfer-out) cost expression for a resource.
///
/// Looks up the shared tiered SKU `aws.data_transfer.egress_tiers` and
/// returns a tiered [`Expr`] driven by the variable
/// `<id>.data_transfer_out_gb`, together with the corresponding
/// [`VariableInfo`] metadata.
///
/// Used by services that include internet egress as a cost component
/// (Lambda, EC2, ECS Fargate, ECS on EC2).  Centralised here so the
/// tier table is not duplicated across service implementations.
pub(crate) fn egress_cost_expr(
    id: &LogicalId,
    pricing: &dyn PriceCatalog,
) -> Result<(Expr, VariableInfo), CostError> {
    let egress_record = pricing.lookup(&Sku::new("aws.data_transfer.egress_tiers"))?;
    let egress_tiers = egress_record.as_tiered().map_err(CostError::Pricing)?;
    let expr = Expr::tiered(egress_tiers, Expr::variable(id.var("data_transfer_out_gb")));
    let info = VariableInfo::new(
        id,
        "data_transfer_out_gb",
        "Data transfer out per month",
        "GB",
    );
    Ok((expr, info))
}
