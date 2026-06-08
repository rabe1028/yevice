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
pub struct KendraSpec {
    /// Index edition: DEVELOPER_EDITION (default) or ENTERPRISE_EDITION.
    pub edition: String,
}

impl Default for KendraSpec {
    fn default() -> Self {
        Self {
            edition: "DEVELOPER_EDITION".to_string(),
        }
    }
}

pub struct KendraService;

const HOURS_PER_MONTH: f64 = 730.0;

impl Service for KendraService {
    type Spec = KendraSpec;

    fn id(&self) -> &'static str {
        "aws.kendra"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &KendraSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        // Amazon Kendra index, billed per hour (730h/mo) at the edition's rate.
        let index_hour_price = pricing.lookup_f64(&Sku::dynamic(format!(
            "aws.kendra.index_hour.{}",
            spec.edition
        )))?;
        // Connector usage: per-document scan + per scan-hour (sync). Usage-driven.
        let scan_document_price =
            pricing.lookup_f64(&Sku::new("aws.kendra.connector_scan_document_price"))?;
        let scan_hour_price =
            pricing.lookup_f64(&Sku::new("aws.kendra.connector_scan_hour_price"))?;

        // Index runs continuously: fixed monthly cost.
        let index = Expr::constant(index_hour_price * HOURS_PER_MONTH);

        // Connector document scans, driven by documents scanned per month.
        let connector_documents = Expr::linear(
            scan_document_price,
            Expr::variable(id.var("documents_scanned")),
            0.0,
        );

        // Connector sync compute, driven by hours spent syncing per month.
        let connector_hours = Expr::linear(
            scan_hour_price,
            Expr::variable(id.var("connector_scan_hours")),
            0.0,
        );

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("Kendra (Developer Edition): {id}"),
            expr: Expr::sum(vec![
                index.clone(),
                connector_documents.clone(),
                connector_hours.clone(),
            ]),
            components: vec![
                CostComponent {
                    name: "Index Hours".into(),
                    expr: index,
                },
                CostComponent {
                    name: "Connector Document Scans".into(),
                    expr: connector_documents,
                },
                CostComponent {
                    name: "Connector Scan Hours".into(),
                    expr: connector_hours,
                },
            ],
            required_variables: vec![
                VariableInfo::new(
                    id,
                    "documents_scanned",
                    "Documents scanned by connectors per month",
                    "documents",
                ),
                VariableInfo::new(
                    id,
                    "connector_scan_hours",
                    "Connector sync (scan) hours per month",
                    "hours",
                ),
            ],
        })
    }
}

const _: f64 = HOURS_PER_MONTH;
