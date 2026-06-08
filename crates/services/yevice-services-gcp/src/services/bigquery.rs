//! GCP BigQuery service implementation.

use serde::{Deserialize, Serialize};
use yevice_core::{
    cost::{CostComponent, Expr, ResourceCost, Tier, VariableInfo},
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::{PriceCatalog, Sku};
use yevice_service_api::{CostError, service::Service};

const FREE_TIER_QUERY_GB: f64 = 1_000.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcpBigQuerySpec {}

pub struct GcpBigQueryService;

impl Service for GcpBigQueryService {
    type Spec = GcpBigQuerySpec;

    fn id(&self) -> &'static str {
        "gcp.bigquery"
    }

    fn provider(&self) -> Provider {
        Provider::Gcp
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        _spec: &GcpBigQuerySpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let active_storage_gb_month =
            pricing.lookup_f64(&Sku::new("gcp.bigquery.active_storage_gb_month"))?;
        let query_per_tb = pricing.lookup_f64(&Sku::new("gcp.bigquery.query_per_tb"))?;

        let storage_cost = Expr::linear(
            active_storage_gb_month,
            Expr::variable(id.var("storage_gb")),
            0.0,
        );

        let query_cost = Expr::tiered(
            vec![
                Tier {
                    upper_limit: Some(FREE_TIER_QUERY_GB),
                    unit_price: 0.0,
                },
                Tier {
                    upper_limit: None,
                    unit_price: query_per_tb / 1_000.0,
                },
            ],
            Expr::variable(id.var("query_gb_scanned")),
        );

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: "BigQuery".into(),
            expr: Expr::sum(vec![storage_cost.clone(), query_cost.clone()]),
            components: vec![
                CostComponent {
                    name: "Storage (active)".into(),
                    expr: storage_cost,
                },
                CostComponent {
                    name: "Queries (data scanned)".into(),
                    expr: query_cost,
                },
            ],
            required_variables: vec![
                VariableInfo {
                    name: id.var("storage_gb"),
                    description: "Active storage per month".into(),
                    unit: "GB".into(),
                },
                VariableInfo {
                    name: id.var("query_gb_scanned"),
                    description: "Data scanned by queries per month".into(),
                    unit: "GB".into(),
                },
            ],
        })
    }
}
