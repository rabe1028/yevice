use serde::{Deserialize, Serialize};
use yevice_core::{
    cost::{CostComponent, ResourceCost, VariableInfo},
    expr::Expr,
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

/// AWS Directory Service for Microsoft Active Directory (AWS Managed Microsoft AD).
///
/// Billed per domain controller-hour. AWS provisions a minimum of two domain
/// controllers per directory for high availability and lists each as its own
/// line item, so the directory cost is `dc_hour_price * controllers * 730h`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryServiceSpec {
    /// Directory edition: `Standard` or `Enterprise` (from `AWS::DirectoryService::MicrosoftAD` `Edition`).
    pub edition: String,
    /// Number of domain controllers. `None` falls back to the AWS minimum of two.
    pub domain_controllers: Option<f64>,
}

pub struct DirectoryServiceService;

const HOURS_PER_MONTH: f64 = 730.0;
/// AWS provisions a minimum of two domain controllers per Managed Microsoft AD.
const DEFAULT_DOMAIN_CONTROLLERS: f64 = 2.0;

impl Service for DirectoryServiceService {
    type Spec = DirectoryServiceSpec;

    fn id(&self) -> &'static str {
        "aws.directory_service"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &DirectoryServiceSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        // Per domain controller-hour price, selected by edition (Standard/Enterprise).
        let dc_hour_price = pricing.lookup_f64(&Sku::dynamic(format!(
            "aws.directory_service.dc_hour.{}",
            spec.edition
        )))?;

        // Controller count: fixed from the template when present, else a usage
        // variable. AWS provisions a minimum of two domain controllers.
        let (controllers, required) = match spec.domain_controllers {
            Some(n) => (Expr::constant(n.max(DEFAULT_DOMAIN_CONTROLLERS)), vec![]),
            None => (
                Expr::variable(id.var("domain_controllers")),
                vec![VariableInfo::new(
                    id,
                    "domain_controllers",
                    "Number of domain controllers (minimum 2)",
                    "controllers",
                )],
            ),
        };

        // Directory cost = dc_hour_price * 730h * controllers.
        let directory = Expr::product(vec![
            Expr::constant(dc_hour_price * HOURS_PER_MONTH),
            controllers,
        ]);

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!(
                "Directory Service: {id} (Managed Microsoft AD, {})",
                spec.edition
            ),
            expr: directory.clone(),
            components: vec![CostComponent {
                name: format!("Domain Controllers ({})", spec.edition),
                expr: directory,
            }],
            required_variables: required,
        })
    }
}
