use serde::{Deserialize, Serialize};
use yevice_core::{
    cost::{CostComponent, ResourceCost, VariableInfo},
    expr::Expr,
    resource::Provider,
    types::{LogicalId, ResourceType, var},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

/// AWS Backup vault (`AWS::Backup::BackupVault`).
///
/// Cost is usage-only: warm (backup) storage is billed per GB-month at an
/// engine-specific rate. The protected-resource engine (EBS / EFS / RDS /
/// Aurora / DynamoDB) determines the rate; EBS is the default since it is the
/// most common protected resource and matches the EBS snapshot warm-storage
/// rate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupSpec {
    /// Protected-resource engine driving the warm-storage rate:
    /// ebs / efs / rds / aurora / dynamodb. Defaults to `ebs`.
    pub backup_type: String,
}

impl Default for BackupSpec {
    fn default() -> Self {
        Self {
            backup_type: "ebs".to_string(),
        }
    }
}

pub struct BackupService;

impl Service for BackupService {
    type Spec = BackupSpec;

    fn id(&self) -> &'static str {
        "aws.backup"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &BackupSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let gb_price = pricing.lookup_f64(&Sku::dynamic(format!(
            "aws.backup.warm_storage_gb_month.{}",
            spec.backup_type
        )))?;

        // Warm (backup) storage is usage-driven: GB-month of stored backup data.
        let storage = Expr::linear(gb_price, Expr::variable(id.var(var::BACKUP_GB)), 0.0);

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("Backup: {id} ({})", spec.backup_type),
            expr: storage.clone(),
            components: vec![CostComponent {
                name: format!("Warm Storage ({})", spec.backup_type),
                expr: storage,

                currency: None,
            }],
            required_variables: vec![VariableInfo::new(
                id,
                var::BACKUP_GB,
                "Warm backup storage",
                "GB",
            )],

            currency: Some("USD".into()),
        })
    }
}
