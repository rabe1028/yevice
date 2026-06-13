use serde::{Deserialize, Serialize};
use yevice_core::{
    cost::{CostComponent, ResourceCost, VariableInfo},
    expr::{Expr, Tier},
    resource::Provider,
    types::{LogicalId, ResourceType, var},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudWatchLogsSpec {
    pub retention_days: Option<u32>,
}

pub struct CloudWatchLogsService;

impl Service for CloudWatchLogsService {
    type Spec = CloudWatchLogsSpec;

    fn id(&self) -> &'static str {
        "aws.cloudwatch_logs"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        _spec: &CloudWatchLogsSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let ingestion_price =
            pricing.lookup_f64(&Sku::new("aws.cloudwatch_logs.ingestion_price_per_gb"))?;
        let storage_price =
            pricing.lookup_f64(&Sku::new("aws.cloudwatch_logs.storage_price_per_gb"))?;
        let free_ingestion =
            pricing.lookup_f64(&Sku::new("aws.cloudwatch_logs.free_tier_ingestion_gb"))?;
        let free_storage =
            pricing.lookup_f64(&Sku::new("aws.cloudwatch_logs.free_tier_storage_gb"))?;

        let ingestion = Expr::tiered(
            vec![
                Tier {
                    upper_limit: Some(free_ingestion),
                    unit_price: 0.0,
                },
                Tier {
                    upper_limit: None,
                    unit_price: ingestion_price,
                },
            ],
            Expr::variable(id.var(var::INGESTION_GB)),
        );
        let storage = Expr::tiered(
            vec![
                Tier {
                    upper_limit: Some(free_storage),
                    unit_price: 0.0,
                },
                Tier {
                    upper_limit: None,
                    unit_price: storage_price,
                },
            ],
            Expr::variable(id.var(var::STORAGE_GB)),
        );

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("CloudWatch Logs: {id}"),
            expr: Expr::sum(vec![ingestion.clone(), storage.clone()]),
            components: vec![
                CostComponent {
                    name: "Ingestion".into(),
                    expr: ingestion,
                },
                CostComponent {
                    name: "Storage".into(),
                    expr: storage,
                },
            ],
            required_variables: vec![
                VariableInfo::new(id, var::INGESTION_GB, "Log data ingested per month", "GB"),
                VariableInfo::new(id, var::STORAGE_GB, "Log data storage", "GB"),
            ],
        })
    }
}
