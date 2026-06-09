//! GCP Cloud Storage service implementation.

use serde::{Deserialize, Serialize};
use yevice_core::{
    cost::{CostComponent, Expr, ResourceCost, VariableInfo, VariableKind},
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::{PriceCatalog, Sku};
use yevice_service_api::{CostError, service::Service};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcpCloudStorageSpec {
    pub storage_class: Option<String>,
    pub location_type: Option<String>,
}

pub struct GcpCloudStorageService;

impl Service for GcpCloudStorageService {
    type Spec = GcpCloudStorageSpec;

    fn id(&self) -> &'static str {
        "gcp.cloud_storage"
    }

    fn provider(&self) -> Provider {
        Provider::Gcp
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &GcpCloudStorageSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let storage_class = spec.storage_class.as_deref().unwrap_or("STANDARD");
        let label = storage_class.to_ascii_uppercase();
        let price_per_gb = match label.as_str() {
            "NEARLINE" => pricing.lookup_f64(&Sku::new("gcp.cloud_storage.nearline_gb_month"))?,
            "COLDLINE" => pricing.lookup_f64(&Sku::new("gcp.cloud_storage.coldline_gb_month"))?,
            "ARCHIVE" => pricing.lookup_f64(&Sku::new("gcp.cloud_storage.archive_gb_month"))?,
            _ => pricing.lookup_f64(&Sku::new("gcp.cloud_storage.standard_gb_month"))?,
        };

        let storage_cost = Expr::linear(price_per_gb, Expr::variable(id.var("storage_gb")), 0.0);

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("Cloud Storage ({label})"),
            expr: storage_cost.clone(),
            components: vec![CostComponent {
                name: format!("Storage ({label})"),
                expr: storage_cost,
            }],
            required_variables: vec![VariableInfo {
                name: id.var("storage_gb"),
                description: "Storage per month".into(),
                unit: "GB".into(),
                kind: VariableKind::Usage,
            }],
        })
    }
}
