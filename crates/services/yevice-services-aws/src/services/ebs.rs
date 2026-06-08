use serde::{Deserialize, Serialize};
use yevice_core::{
    cost::{CostComponent, ResourceCost, VariableInfo},
    expr::Expr,
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

/// Standalone Amazon EBS volume (`AWS::EC2::Volume`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EbsSpec {
    /// Volume type: gp3 / gp2 / st1 / sc1 / io1 / io2.
    pub volume_type: String,
    /// Provisioned size in GB (from the template when present).
    pub size_gb: Option<f64>,
    /// Provisioned IOPS (`Iops`). Billed for io1/io2, and for gp3 above the
    /// 3,000 included baseline.
    pub iops: Option<f64>,
    /// Provisioned throughput in MB/s (`Throughput`). Billed for gp3 above the
    /// 125 MB/s included baseline.
    pub throughput: Option<f64>,
}

// ap-northeast-1 provisioned-performance rates (match the RDS gp3/io model).
const GP3_BASELINE_IOPS: f64 = 3000.0;
const GP3_BASELINE_THROUGHPUT_MBPS: f64 = 125.0;
const GP3_IOPS_MONTH_PRICE: f64 = 0.008;
const GP3_THROUGHPUT_MBPS_MONTH_PRICE: f64 = 0.048;
const PIOPS_MONTH_PRICE: f64 = 0.074; // io1/io2 provisioned IOPS

pub struct EbsService;

impl Service for EbsService {
    type Spec = EbsSpec;

    fn id(&self) -> &'static str {
        "aws.ebs"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &EbsSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let gb_price = pricing.lookup_f64(&Sku::dynamic(format!(
            "aws.ebs.gb_month.{}",
            spec.volume_type
        )))?;
        let snapshot_price = pricing.lookup_f64(&Sku::new("aws.ebs.snapshot_gb_month"))?;

        // Provisioned performance, billed in addition to GB-month storage:
        //   gp3 — IOPS above the 3,000 baseline and throughput above 125 MB/s
        //   io1/io2 — all provisioned IOPS
        // It is folded into the volume cost (as the linear offset / constant)
        // so the volume charge is storage + provisioned performance, not just
        // `gb_price * size`.
        let iops = spec.iops.unwrap_or(0.0);
        let throughput = spec.throughput.unwrap_or(0.0);
        let perf = match spec.volume_type.as_str() {
            "gp3" => {
                let extra_iops = (iops - GP3_BASELINE_IOPS).max(0.0) * GP3_IOPS_MONTH_PRICE;
                let extra_tput = (throughput - GP3_BASELINE_THROUGHPUT_MBPS).max(0.0)
                    * GP3_THROUGHPUT_MBPS_MONTH_PRICE;
                extra_iops + extra_tput
            }
            "io1" | "io2" => iops * PIOPS_MONTH_PRICE,
            _ => 0.0,
        };

        // Volume cost = gb_price*size + provisioned performance; size is fixed
        // from the template or a usage variable when unspecified.
        let (volume, mut required) = match spec.size_gb {
            Some(size) => (Expr::constant(gb_price * size + perf), vec![]),
            None => (
                Expr::linear(gb_price, Expr::variable(id.var("size_gb")), perf),
                vec![VariableInfo::new(id, "size_gb", "Volume size", "GB")],
            ),
        };

        // Snapshot storage is usage-driven (not a volume property).
        let snapshot = Expr::linear(snapshot_price, Expr::variable(id.var("snapshot_gb")), 0.0);
        required.push(VariableInfo::new(
            id,
            "snapshot_gb",
            "EBS snapshot storage",
            "GB",
        ));

        let perf_note = if perf > 0.0 {
            ", incl. provisioned IOPS/throughput"
        } else {
            ""
        };

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("EBS: {id} ({})", spec.volume_type),
            expr: Expr::sum(vec![volume.clone(), snapshot.clone()]),
            components: vec![
                CostComponent {
                    name: format!("Volume ({}{perf_note})", spec.volume_type),
                    expr: volume,
                },
                CostComponent {
                    name: "Snapshots".into(),
                    expr: snapshot,
                },
            ],
            required_variables: required,
        })
    }
}
