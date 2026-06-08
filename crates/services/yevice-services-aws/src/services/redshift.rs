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
pub struct RedshiftSpec {
    pub node_type: String,
    pub node_count: Option<f64>,
    /// Cluster run-hours per month. Defaults to a full month (730) so a
    /// template-defined cluster is never priced at $0; some AWS estimates
    /// (incl. parts of the CDP) use 720. Set via the `Hours` template property.
    pub hours: Option<f64>,
}

const FULL_MONTH_HOURS: f64 = 730.0;

pub struct RedshiftService;

impl Service for RedshiftService {
    type Spec = RedshiftSpec;

    fn id(&self) -> &'static str {
        "aws.redshift"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &RedshiftSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let sku = Sku::dynamic(format!("aws.redshift.{}", spec.node_type));
        let node_hour = pricing.lookup_f64(&sku)?;
        let storage_price = pricing.lookup_f64(&Sku::new("aws.redshift.storage_gb_month"))?;
        let spectrum_price = pricing.lookup_f64(&Sku::new("aws.redshift.spectrum_tb_scan"))?;

        let node_expr = match spec.node_count {
            Some(n) => Expr::constant(n),
            None => Expr::variable(id.var("node_count")),
        };

        // Node cost = node_hour x node_count x hours. `hours` is fixed at
        // generate time (default = full month) so the cluster never evaluates
        // to $0 from an un-filled usage placeholder.
        let hours = spec.hours.unwrap_or(FULL_MONTH_HOURS);
        let nodes = Expr::product(vec![Expr::constant(node_hour * hours), node_expr]);
        // Managed (RA3/RMS) storage per GB-month.
        let storage = Expr::linear(storage_price, Expr::variable(id.var("storage_gb")), 0.0);
        // Redshift Spectrum: per-TB scanned against external (S3) data.
        let spectrum = Expr::linear(spectrum_price, Expr::variable(id.var("spectrum_tb")), 0.0);

        let mut required = vec![];
        if spec.node_count.is_none() {
            required.push(VariableInfo::new(
                id,
                "node_count",
                "Number of Redshift nodes",
                "nodes",
            ));
        }
        required.push(VariableInfo::new(id, "storage_gb", "Managed storage", "GB"));
        required.push(VariableInfo::new(
            id,
            "spectrum_tb",
            "Redshift Spectrum data scanned",
            "TB",
        ));

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("Redshift: {id}"),
            expr: Expr::sum(vec![nodes.clone(), storage.clone(), spectrum.clone()]),
            components: vec![
                CostComponent {
                    name: "Nodes".into(),
                    expr: nodes,
                },
                CostComponent {
                    name: "Managed Storage".into(),
                    expr: storage,
                },
                CostComponent {
                    name: "Spectrum Scan".into(),
                    expr: spectrum,
                },
            ],
            required_variables: required,
        })
    }
}
