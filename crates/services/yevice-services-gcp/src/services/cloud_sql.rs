//! GCP Cloud SQL service implementation.

use serde::{Deserialize, Serialize};
use yevice_core::{
    HOURS_PER_MONTH,
    cost::{CostComponent, Expr, ResourceCost, VariableInfo, VariableKind},
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::{PriceCatalog, Sku};
use yevice_service_api::{CostError, service::Service};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcpCloudSqlSpec {
    pub tier: String,
    pub database_version: String,
    pub disk_size_gb: Option<f64>,
    pub availability_type: String,
}

impl Default for GcpCloudSqlSpec {
    fn default() -> Self {
        Self {
            tier: "db-n1-standard-1".to_string(),
            database_version: "POSTGRES_15".to_string(),
            disk_size_gb: None,
            availability_type: "ZONAL".to_string(),
        }
    }
}

pub struct GcpCloudSqlService;

impl Service for GcpCloudSqlService {
    type Spec = GcpCloudSqlSpec;

    fn id(&self) -> &'static str {
        "gcp.cloud_sql"
    }

    fn provider(&self) -> Provider {
        Provider::Gcp
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &GcpCloudSqlSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let vcpu_hour = pricing.lookup_f64(&Sku::new("gcp.cloud_sql.vcpu_hour"))?;
        let ram_gb_hour = pricing.lookup_f64(&Sku::new("gcp.cloud_sql.ram_gb_hour"))?;
        let ssd_gb_month = pricing.lookup_f64(&Sku::new("gcp.cloud_sql.ssd_gb_month"))?;

        let (vcpu, ram_gb) = parse_sql_tier(&spec.tier);
        let ha_multiplier = if spec.availability_type.eq_ignore_ascii_case("REGIONAL") {
            2.0
        } else {
            1.0
        };

        let instance_monthly =
            (vcpu * vcpu_hour + ram_gb * ram_gb_hour) * HOURS_PER_MONTH * ha_multiplier;
        let instance_cost = Expr::constant(instance_monthly);
        let storage_cost = Expr::linear(ssd_gb_month, Expr::variable(id.var("storage_gb")), 0.0);

        let ha_label = if ha_multiplier > 1.0 { " HA" } else { "" };

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!(
                "Cloud SQL {} ({}{})",
                spec.database_version, spec.tier, ha_label
            ),
            expr: Expr::sum(vec![instance_cost.clone(), storage_cost.clone()]),
            components: vec![
                CostComponent {
                    name: format!("Instance ({}{})", spec.tier, ha_label),
                    expr: instance_cost,

                    currency: None,
                },
                CostComponent {
                    name: "Storage (SSD)".into(),
                    expr: storage_cost,

                    currency: None,
                },
            ],
            required_variables: vec![VariableInfo {
                name: id.var("storage_gb"),
                description: "Allocated storage".into(),
                unit: "GB".into(),
                kind: VariableKind::Usage,
            }],

            currency: Some("USD".into()),
        })
    }
}

/// Map Cloud SQL tier string to (vCPU count, RAM GB).
pub(crate) fn parse_sql_tier(tier: &str) -> (f64, f64) {
    match tier {
        "db-f1-micro" => (0.2, 0.6),
        "db-g1-small" => (0.5, 1.7),
        v if v.contains("standard") => {
            let n = v
                .split('-')
                .next_back()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1.0);
            (n, n * 3.75)
        }
        v if v.contains("highmem") => {
            let n = v
                .split('-')
                .next_back()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1.0);
            (n, n * 6.5)
        }
        v if v.contains("highcpu") => {
            let n = v
                .split('-')
                .next_back()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1.0);
            (n, n * 0.9)
        }
        // db-custom-N-M: N vCPUs, M MB RAM
        v if v.starts_with("db-custom-") => {
            let parts: Vec<&str> = v.split('-').collect();
            if parts.len() >= 4 {
                let vcpu = parts[2].parse().unwrap_or(1.0);
                let ram_gb = parts[3].parse::<f64>().unwrap_or(3840.0) / 1024.0;
                (vcpu, ram_gb)
            } else {
                (1.0, 3.75)
            }
        }
        _ => {
            tracing::warn!(tier = %tier, "unknown Cloud SQL tier; using default vCPU/RAM");
            (1.0, 3.75)
        }
    }
}
