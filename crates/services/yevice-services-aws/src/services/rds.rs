use serde::{Deserialize, Serialize};
use yevice_core::{
    HOURS_PER_MONTH,
    cost::{CostComponent, ResourceCost, VariableInfo},
    expr::Expr,
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RdsEngine {
    Mysql,
    Postgres,
    Mariadb,
    SqlServer,
    AuroraMysql,
    AuroraPostgresql,
    Other(String),
}

impl RdsEngine {
    pub fn from_cfn(engine: &str) -> Self {
        match engine {
            "mysql" => Self::Mysql,
            "postgres" => Self::Postgres,
            "mariadb" => Self::Mariadb,
            // Only SQL Server Standard Edition has verified rates. Other
            // editions (Enterprise/Express/Web) keep their distinct engine key
            // via `Other`, so they are NOT silently priced as Standard — they
            // resolve to no rate (explicitly unsupported) until rates are added.
            "sqlserver-se" | "sqlserver" => Self::SqlServer,
            "aurora-mysql" => Self::AuroraMysql,
            "aurora-postgresql" => Self::AuroraPostgresql,
            other => Self::Other(other.to_string()),
        }
    }

    pub fn is_aurora(&self) -> bool {
        matches!(self, Self::AuroraMysql | Self::AuroraPostgresql)
    }

    pub fn as_pricing_key(&self) -> &str {
        match self {
            Self::Mysql => "mysql",
            Self::Postgres => "postgres",
            Self::Mariadb => "mariadb",
            Self::SqlServer => "sqlserver-se",
            Self::AuroraMysql => "aurora-mysql",
            Self::AuroraPostgresql => "aurora-postgresql",
            Self::Other(s) => s,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RdsSpec {
    pub instance_type: String,
    pub engine: RdsEngine,
    pub allocated_storage_gb: f64,
    pub storage_type: String,
    pub iops: Option<f64>,
    pub multi_az: bool,
}

pub struct RdsService;

impl Service for RdsService {
    type Spec = RdsSpec;

    fn id(&self) -> &'static str {
        "aws.rds"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &RdsSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let engine_key = spec.engine.as_pricing_key();
        let hourly_sku = Sku::dynamic(format!("aws.rds.{}.{}", engine_key, spec.instance_type));
        let hourly_price = pricing.lookup_f64(&hourly_sku)?;
        let storage_sku = Sku::dynamic(format!(
            "aws.rds_storage.{}.{}",
            engine_key, spec.instance_type
        ));
        let storage_price_per_gb = pricing.lookup_f64(&storage_sku)?;

        let az_mult = if spec.multi_az { 2.0 } else { 1.0 };
        let instance_cost = hourly_price * HOURS_PER_MONTH * az_mult;
        let multi_az_label = if spec.multi_az { ", Multi-AZ" } else { "" };

        if spec.engine.is_aurora() {
            let aurora_instance = Expr::constant(instance_cost);
            let aurora_storage =
                Expr::linear(0.12, Expr::variable(id.var("aurora_storage_gb")), 0.0);
            Ok(ResourceCost {
                logical_id: id.clone(),
                resource_type: rt.clone(),
                label: format!(
                    "RDS Aurora: {id} ({}, {}{multi_az_label})",
                    spec.instance_type, engine_key
                ),
                expr: Expr::sum(vec![aurora_instance.clone(), aurora_storage.clone()]),
                components: vec![
                    CostComponent {
                        name: "Instance".into(),
                        expr: aurora_instance,
                    },
                    CostComponent {
                        name: "Storage".into(),
                        expr: aurora_storage,
                    },
                ],
                required_variables: vec![VariableInfo::new(
                    id,
                    "aurora_storage_gb",
                    "Aurora storage amount",
                    "GB",
                )],
            })
        } else {
            let storage_per_az = match spec.storage_type.as_str() {
                "gp3" => {
                    let base = 0.1216 * spec.allocated_storage_gb;
                    let iops_cost = spec.iops.map_or(0.0, |i| {
                        if i > 3000.0 {
                            (i - 3000.0) * 0.008
                        } else {
                            0.0
                        }
                    });
                    base + iops_cost
                }
                "io1" | "io2" => {
                    0.142 * spec.allocated_storage_gb + spec.iops.unwrap_or(0.0) * 0.074
                }
                _ => storage_price_per_gb * spec.allocated_storage_gb,
            };
            // Multi-AZ deployments replicate storage to the standby, so storage
            // is billed on both instances (same multiplier as compute).
            let storage_cost = storage_per_az * az_mult;

            Ok(ResourceCost {
                logical_id: id.clone(),
                resource_type: rt.clone(),
                label: format!(
                    "RDS: {id} ({}, {}, {}{multi_az_label})",
                    spec.instance_type, engine_key, spec.storage_type
                ),
                expr: Expr::constant(instance_cost + storage_cost),
                components: vec![
                    CostComponent {
                        name: "Instance".into(),
                        expr: Expr::constant(instance_cost),
                    },
                    CostComponent {
                        name: "Storage".into(),
                        expr: Expr::constant(storage_cost),
                    },
                ],
                required_variables: vec![],
            })
        }
    }
}
