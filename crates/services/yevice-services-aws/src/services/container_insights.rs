use serde::{Deserialize, Serialize};
use yevice_core::{
    cost::{CostComponent, ResourceCost, VariableInfo},
    expr::Expr,
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

/// ECS Cluster with optional CloudWatch Container Insights.
///
/// A bare ECS cluster is free; the cost comes entirely from Container Insights
/// (custom metrics + the logs it emits) when enabled on the cluster.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerInsightsSpec {
    /// Whether Container Insights is enabled on the cluster.
    pub enabled: bool,
}

pub struct ContainerInsightsService;

impl Service for ContainerInsightsService {
    type Spec = ContainerInsightsSpec;

    fn id(&self) -> &'static str {
        "aws.container_insights"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &ContainerInsightsSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        if !spec.enabled {
            // A cluster without Container Insights is free.
            return Ok(ResourceCost {
                logical_id: id.clone(),
                resource_type: rt.clone(),
                label: format!("ECS Cluster: {id} (Container Insights off)"),
                expr: Expr::constant(0.0),
                components: vec![],
                required_variables: vec![],
            });
        }

        let metric_price =
            pricing.lookup_f64(&Sku::new("aws.cloudwatch.custom_metric_month_price"))?;
        let ingestion_price =
            pricing.lookup_f64(&Sku::new("aws.cloudwatch_logs.ingestion_price_per_gb"))?;
        let storage_price =
            pricing.lookup_f64(&Sku::new("aws.cloudwatch_logs.storage_price_per_gb"))?;

        // Custom metrics: count x per-metric monthly price.
        let metrics = Expr::linear(metric_price, Expr::variable(id.var("custom_metrics")), 0.0);

        // Container Insights emits its own logs. A single `log_gb` drives both
        // ingestion and storage (list price; the account free tier is modelled
        // on the dedicated log groups). No free tier is applied here to avoid
        // double-crediting the account-level allowance.
        let log_gb = || Expr::variable(id.var("log_gb"));
        let ingestion = Expr::linear(ingestion_price, log_gb(), 0.0);
        let storage = Expr::linear(storage_price, log_gb(), 0.0);

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("Container Insights: {id}"),
            expr: Expr::sum(vec![metrics.clone(), ingestion.clone(), storage.clone()]),
            components: vec![
                CostComponent {
                    name: "Custom Metrics".into(),
                    expr: metrics,
                },
                CostComponent {
                    name: "Logs Ingestion".into(),
                    expr: ingestion,
                },
                CostComponent {
                    name: "Logs Storage".into(),
                    expr: storage,
                },
            ],
            required_variables: vec![
                VariableInfo::new(
                    id,
                    "custom_metrics",
                    "Number of Container Insights custom metrics",
                    "metrics",
                ),
                VariableInfo::new(
                    id,
                    "log_gb",
                    "Container Insights log volume (ingested = stored)",
                    "GB",
                ),
            ],
        })
    }
}
