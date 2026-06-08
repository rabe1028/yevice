use serde::{Deserialize, Serialize};
use yevice_core::{
    cost::{CostComponent, ResourceCost, VariableInfo},
    expr::Expr,
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenSearchServerlessSpec {
    pub collection_type: Option<String>,
}

pub struct OpenSearchServerlessService;

const HOURS_PER_MONTH: f64 = 730.0;
const OCU_FLOOR: f64 = 0.5;

impl Service for OpenSearchServerlessService {
    type Spec = OpenSearchServerlessSpec;

    fn id(&self) -> &'static str {
        "aws.opensearch_serverless"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        _spec: &OpenSearchServerlessSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let ocu_hour_price =
            pricing.lookup_f64(&Sku::new("aws.opensearch_serverless.ocu_hour_price"))?;
        let storage_price =
            pricing.lookup_f64(&Sku::new("aws.opensearch_serverless.storage_price_per_gb"))?;

        // OpenSearch Serverless bills a minimum of 0.5 OCU per dimension
        // (indexing + search) for a dev/test collection — more with redundant
        // replicas. Honour fractional inputs but never below the 0.5 floor, so
        // a 0 (or omitted) usage value cannot under-report below what AWS bills.
        let indexing = Expr::linear(
            ocu_hour_price * HOURS_PER_MONTH,
            Expr::Max {
                expr: Box::new(Expr::variable(id.var("indexing_ocu"))),
                floor: OCU_FLOOR,
            },
            0.0,
        );
        let search = Expr::linear(
            ocu_hour_price * HOURS_PER_MONTH,
            Expr::Max {
                expr: Box::new(Expr::variable(id.var("search_ocu"))),
                floor: OCU_FLOOR,
            },
            0.0,
        );
        let storage = Expr::linear(storage_price, Expr::variable(id.var("storage_gb")), 0.0);

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("OpenSearch Serverless: {id}"),
            expr: Expr::sum(vec![indexing.clone(), search.clone(), storage.clone()]),
            components: vec![
                CostComponent {
                    name: "Indexing OCU".into(),
                    expr: indexing,
                },
                CostComponent {
                    name: "Search OCU".into(),
                    expr: search,
                },
                CostComponent {
                    name: "Storage".into(),
                    expr: storage,
                },
            ],
            required_variables: vec![
                VariableInfo::new(id, "indexing_ocu", "Indexing OCU count", "OCU"),
                VariableInfo::new(id, "search_ocu", "Search/query OCU count", "OCU"),
                VariableInfo::new(id, "storage_gb", "Managed storage", "GB"),
            ],
        })
    }
}

const _: f64 = HOURS_PER_MONTH;
