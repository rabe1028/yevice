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
pub enum BatchLaunchType {
    Fargate,
    Ec2,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchJobDefinitionSpec {
    pub launch_type: BatchLaunchType,
    pub vcpu: f64,
    pub memory_gb: f64,
    pub ephemeral_storage_gb: Option<f64>,
    pub ebs: Option<BatchEbsConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchEbsConfig {
    pub size_gb: f64,
    pub volume_type: String,
    pub iops: Option<f64>,
    pub throughput_mibps: Option<f64>,
}

pub struct BatchService;

impl Service for BatchService {
    type Spec = BatchJobDefinitionSpec;

    fn id(&self) -> &'static str {
        "aws.batch"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &BatchJobDefinitionSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let vcpu_hour = pricing.lookup_f64(&Sku::new("aws.batch.fargate_vcpu_hour_price"))?;
        let mem_hour = pricing.lookup_f64(&Sku::new("aws.batch.fargate_memory_gb_hour_price"))?;
        let ephemeral_hour = pricing.lookup_f64(&Sku::new(
            "aws.batch.fargate_ephemeral_storage_gb_hour_price",
        ))?;
        let ephemeral_free =
            pricing.lookup_f64(&Sku::new("aws.batch.fargate_ephemeral_free_gb"))?;
        let ebs_gb_month = pricing.lookup_f64(&Sku::new("aws.batch.ebs_gp3_gb_month_price"))?;
        let ebs_iops_month = pricing.lookup_f64(&Sku::new("aws.batch.ebs_gp3_iops_month_price"))?;
        let ebs_iops_free = pricing.lookup_f64(&Sku::new("aws.batch.ebs_gp3_iops_free"))?;
        let ebs_throughput_month =
            pricing.lookup_f64(&Sku::new("aws.batch.ebs_gp3_throughput_mibps_month_price"))?;
        let ebs_throughput_free =
            pricing.lookup_f64(&Sku::new("aws.batch.ebs_gp3_throughput_free_mibps"))?;

        let hours_per_month = 730.0;

        let var_executions = id.var("executions");
        let var_duration_sec = id.var("avg_duration_sec");

        let hours_per_exec = Expr::div(
            Expr::variable(var_duration_sec.clone()),
            Expr::constant(3600.0),
        );

        let compute_per_hour = Expr::sum(vec![
            Expr::constant(vcpu_hour * spec.vcpu),
            Expr::constant(mem_hour * spec.memory_gb),
        ]);

        let compute_cost = Expr::product(vec![
            compute_per_hour,
            hours_per_exec.clone(),
            Expr::variable(var_executions.clone()),
        ]);

        let storage_cost = match (&spec.launch_type, &spec.ebs) {
            (_, Some(ebs)) => {
                let base_monthly = ebs.size_gb * ebs_gb_month;
                let iops_monthly = ebs
                    .iops
                    .map_or(0.0, |i| (i - ebs_iops_free).max(0.0) * ebs_iops_month);
                let throughput_monthly = ebs.throughput_mibps.map_or(0.0, |t| {
                    (t - ebs_throughput_free).max(0.0) * ebs_throughput_month
                });
                let total_monthly = base_monthly + iops_monthly + throughput_monthly;
                Expr::product(vec![
                    Expr::constant(total_monthly / hours_per_month),
                    hours_per_exec.clone(),
                    Expr::variable(var_executions.clone()),
                ])
            }
            (BatchLaunchType::Fargate, None) => {
                let ephemeral_gb = spec.ephemeral_storage_gb.unwrap_or(20.0);
                let billable_gb = (ephemeral_gb - ephemeral_free).max(0.0);
                if billable_gb > 0.0 {
                    Expr::product(vec![
                        Expr::constant(billable_gb * ephemeral_hour),
                        hours_per_exec,
                        Expr::variable(var_executions.clone()),
                    ])
                } else {
                    Expr::constant(0.0)
                }
            }
            _ => Expr::constant(0.0),
        };

        let mut label_parts = vec![
            format!("{}vCPU", spec.vcpu),
            format!("{:.0}GB", spec.memory_gb),
        ];
        if let Some(ebs) = &spec.ebs {
            label_parts.push(format!("EBS {}GB", ebs.size_gb));
        }

        let storage_label = match &spec.ebs {
            Some(ebs) => format!("EBS Storage ({})", ebs.volume_type),
            None => "Ephemeral Storage".into(),
        };

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("Batch Job: {id} ({})", label_parts.join(", ")),
            expr: Expr::sum(vec![compute_cost.clone(), storage_cost.clone()]),
            components: vec![
                CostComponent {
                    name: format!("Compute ({}vCPU, {}GB)", spec.vcpu, spec.memory_gb),
                    expr: compute_cost,
                },
                CostComponent {
                    name: storage_label,
                    expr: storage_cost,
                },
            ],
            required_variables: vec![
                VariableInfo::new(
                    id,
                    "executions",
                    "Number of job executions per month",
                    "executions",
                ),
                VariableInfo::new(id, "avg_duration_sec", "Average job duration", "seconds"),
            ],
        })
    }
}
