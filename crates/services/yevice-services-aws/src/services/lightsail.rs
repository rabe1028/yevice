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
pub struct LightsailSpec {
    pub bundle_id: Option<String>,
    pub disk_size_gb: Option<f64>,
    /// `true` for a standalone `AWS::Lightsail::Disk` (storage only, no
    /// instance bundle); `false` for an `AWS::Lightsail::Instance`.
    #[serde(default)]
    pub is_disk: bool,
}

pub struct LightsailService;

impl Service for LightsailService {
    type Spec = LightsailSpec;

    fn id(&self) -> &'static str {
        "aws.lightsail"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &LightsailSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        // A standalone AWS::Lightsail::Disk is block storage only — no instance
        // bundle. Lightsail instance bundles already include their root SSD in
        // the fixed plan price, so additional block storage is modeled as its
        // own AWS::Lightsail::Disk resource and charged only here.
        if spec.is_disk {
            let disk_gb_price =
                pricing.lookup_f64(&Sku::new("aws.lightsail.disk_gb_month_price"))?;
            let mut vars = vec![];
            let disk_cost = match spec.disk_size_gb {
                Some(size) => Expr::constant(disk_gb_price * size),
                None => {
                    vars.push(VariableInfo::new(
                        id,
                        "disk_size_gb",
                        "Lightsail block-storage disk size",
                        "GB",
                    ));
                    Expr::linear(disk_gb_price, Expr::variable(id.var("disk_size_gb")), 0.0)
                }
            };
            return Ok(ResourceCost {
                logical_id: id.clone(),
                resource_type: rt.clone(),
                label: format!("Lightsail Disk: {id}"),
                expr: disk_cost.clone(),
                components: vec![CostComponent {
                    name: "Disk".into(),
                    expr: disk_cost,

                    currency: None,
                }],
                required_variables: vars,

                currency: Some("USD".into()),
            });
        }

        // AWS::Lightsail::Instance: the bundle plan price already includes the
        // instance's root SSD storage, so no separate disk cost is added here.
        let bundle = spec.bundle_id.as_deref().unwrap_or("nano_2_0");
        let bundle_month_price =
            pricing.lookup_f64(&Sku::dynamic(format!("aws.lightsail.bundle.{bundle}")))?;
        let instance_cost = Expr::constant(bundle_month_price);

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("Lightsail: {id}"),
            expr: instance_cost.clone(),
            components: vec![CostComponent {
                name: "Instance (Bundle)".into(),
                expr: instance_cost,

                currency: None,
            }],
            required_variables: vec![],

            currency: Some("USD".into()),
        })
    }
}
