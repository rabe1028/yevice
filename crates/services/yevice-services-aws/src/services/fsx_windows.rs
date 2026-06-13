use serde::{Deserialize, Serialize};
use yevice_core::{
    cost::{CostComponent, ResourceCost, VariableInfo},
    expr::Expr,
    resource::Provider,
    types::{LogicalId, ResourceType, var},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

/// Amazon FSx for Windows File Server file system (`AWS::FSx::FileSystem`,
/// `FileSystemType` = `WINDOWS`).
///
/// Pricing has four dimensions in ap-northeast-1, all rated per the chosen
/// deployment option (Single-AZ vs Multi-AZ) and, for storage, the storage
/// type (SSD vs HDD):
///   * storage capacity (GB-month),
///   * provisioned throughput capacity (MBps-month),
///   * backup storage (GB-month, usage-driven).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsxWindowsSpec {
    /// Storage type: `SSD` or `HDD` (defaults to SSD when absent).
    pub storage_type: String,
    /// `true` for Multi-AZ deployments, `false` for Single-AZ.
    pub multi_az: bool,
    /// Provisioned storage capacity in GB (from the template when present).
    pub storage_capacity_gb: Option<f64>,
    /// Provisioned throughput capacity in MBps (from the template when present).
    pub throughput_capacity_mbps: Option<f64>,
}

pub struct FsxWindowsService;

impl FsxWindowsService {
    /// SKU dimension suffix encoding deployment option and (where relevant)
    /// storage type, e.g. `multi_az` or `ssd.single_az`.
    fn deployment(spec: &FsxWindowsSpec) -> &'static str {
        if spec.multi_az {
            "multi_az"
        } else {
            "single_az"
        }
    }

    fn storage_type(spec: &FsxWindowsSpec) -> &'static str {
        if spec.storage_type.eq_ignore_ascii_case("hdd") {
            "hdd"
        } else {
            "ssd"
        }
    }
}

impl Service for FsxWindowsService {
    type Spec = FsxWindowsSpec;

    fn id(&self) -> &'static str {
        "aws.fsx_windows"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &FsxWindowsSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let deployment = Self::deployment(spec);
        let storage_type = Self::storage_type(spec);

        let storage_price = pricing.lookup_f64(&Sku::dynamic(format!(
            "aws.fsx_windows.storage_gb_month.{storage_type}.{deployment}"
        )))?;
        let throughput_price = pricing.lookup_f64(&Sku::dynamic(format!(
            "aws.fsx_windows.throughput_mbps_month.{deployment}"
        )))?;
        let backup_price = pricing.lookup_f64(&Sku::new("aws.fsx_windows.backup_gb_month"))?;

        let mut required = vec![];

        // Storage capacity: fixed from the template, or a variable when absent.
        let storage = match spec.storage_capacity_gb {
            Some(gb) => Expr::constant(storage_price * gb),
            None => {
                required.push(VariableInfo::new(
                    id,
                    "storage_capacity_gb",
                    "Provisioned storage capacity",
                    "GB",
                ));
                Expr::linear(
                    storage_price,
                    Expr::variable(id.var("storage_capacity_gb")),
                    0.0,
                )
            }
        };

        // Throughput capacity: fixed from the template, or a variable when absent.
        let throughput = match spec.throughput_capacity_mbps {
            Some(mbps) => Expr::constant(throughput_price * mbps),
            None => {
                required.push(VariableInfo::new(
                    id,
                    "throughput_capacity_mbps",
                    "Provisioned throughput capacity",
                    "MBps",
                ));
                Expr::linear(
                    throughput_price,
                    Expr::variable(id.var("throughput_capacity_mbps")),
                    0.0,
                )
            }
        };

        // Backup storage is usage-driven (not a file-system property).
        let backup = Expr::linear(backup_price, Expr::variable(id.var(var::BACKUP_GB)), 0.0);
        required.push(VariableInfo::new(
            id,
            var::BACKUP_GB,
            "Backup storage per month",
            "GB",
        ));

        let deployment_label = if spec.multi_az {
            "Multi-AZ"
        } else {
            "Single-AZ"
        };
        let storage_label = storage_type.to_uppercase();

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("FSx for Windows: {id} ({storage_label}, {deployment_label})"),
            expr: Expr::sum(vec![storage.clone(), throughput.clone(), backup.clone()]),
            components: vec![
                CostComponent {
                    name: format!("Storage ({storage_label})"),
                    expr: storage,
                },
                CostComponent {
                    name: "Throughput Capacity".into(),
                    expr: throughput,
                },
                CostComponent {
                    name: "Backups".into(),
                    expr: backup,
                },
            ],
            required_variables: required,
        })
    }
}
