use serde::{Deserialize, Serialize};
use yevice_core::{
    HOURS_PER_MONTH,
    capacity::{CapacityModel, Constraint, QuotaType, Quotas, Severity},
    cost::{CostComponent, ResourceCost, VariableInfo},
    expr::Expr,
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

use crate::quotas::{
    DEFAULT_KINESIS_MAX_MB_PER_SEC_PER_SHARD, DEFAULT_KINESIS_MAX_RECORDS_PER_SEC_PER_SHARD,
    DEFAULT_KINESIS_MAX_SHARDS_PER_STREAM, KINESIS_MAX_MB_PER_SEC_PER_SHARD,
    KINESIS_MAX_RECORDS_PER_SEC_PER_SHARD, KINESIS_MAX_SHARDS_PER_STREAM,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum KinesisStreamMode {
    Provisioned { shard_count: Option<f64> },
    OnDemand,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KinesisSpec {
    pub stream_mode: KinesisStreamMode,
    pub retention_hours: f64,
}

impl Default for KinesisSpec {
    fn default() -> Self {
        Self {
            stream_mode: KinesisStreamMode::Provisioned { shard_count: None },
            retention_hours: 24.0,
        }
    }
}

pub struct KinesisService;

impl Service for KinesisService {
    type Spec = KinesisSpec;

    fn id(&self) -> &'static str {
        "aws.kinesis"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &KinesisSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let shard_hour_price = pricing.lookup_f64(&Sku::new("aws.kinesis.shard_hour_price"))?;
        let put_payload_unit_price =
            pricing.lookup_f64(&Sku::new("aws.kinesis.put_payload_unit_price"))?;
        let on_demand_ingestion_price =
            pricing.lookup_f64(&Sku::new("aws.kinesis.on_demand_ingestion_price_per_gb"))?;
        let on_demand_retrieval_price =
            pricing.lookup_f64(&Sku::new("aws.kinesis.on_demand_retrieval_price_per_gb"))?;
        let on_demand_stream_hour_price =
            pricing.lookup_f64(&Sku::new("aws.kinesis.on_demand_stream_hour_price"))?;

        match &spec.stream_mode {
            KinesisStreamMode::Provisioned { shard_count } => {
                let shards_expr = match shard_count {
                    Some(n) => Expr::constant(*n),
                    None => Expr::variable(id.var("shard_count")),
                };
                let shard_cost = Expr::linear(shard_hour_price * HOURS_PER_MONTH, shards_expr, 0.0);
                let put_cost = Expr::linear(
                    put_payload_unit_price,
                    Expr::variable(id.var("put_records")),
                    0.0,
                );

                let mut vars = vec![VariableInfo::new(
                    id,
                    "put_records",
                    "PUT records per month",
                    "records",
                )];
                if shard_count.is_none() {
                    vars.insert(
                        0,
                        VariableInfo::new(id, "shard_count", "Number of shards", "shards"),
                    );
                }

                Ok(ResourceCost {
                    logical_id: id.clone(),
                    resource_type: rt.clone(),
                    label: format!("Kinesis Provisioned: {id}"),
                    expr: Expr::sum(vec![shard_cost.clone(), put_cost.clone()]),
                    components: vec![
                        CostComponent {
                            name: "Shards".into(),
                            expr: shard_cost,

                            currency: None,
                        },
                        CostComponent {
                            name: "PUT Payload".into(),
                            expr: put_cost,

                            currency: None,
                        },
                    ],
                    required_variables: vars,

                    currency: Some("USD".into()),
                })
            }
            KinesisStreamMode::OnDemand => {
                let stream_hour = Expr::constant(on_demand_stream_hour_price * HOURS_PER_MONTH);
                let ingestion = Expr::linear(
                    on_demand_ingestion_price,
                    Expr::variable(id.var("data_ingestion_gb")),
                    0.0,
                );
                let retrieval = Expr::linear(
                    on_demand_retrieval_price,
                    Expr::variable(id.var("retrieval_gb")),
                    0.0,
                );

                Ok(ResourceCost {
                    logical_id: id.clone(),
                    resource_type: rt.clone(),
                    label: format!("Kinesis On-Demand: {id}"),
                    expr: Expr::sum(vec![
                        stream_hour.clone(),
                        ingestion.clone(),
                        retrieval.clone(),
                    ]),
                    components: vec![
                        CostComponent {
                            name: "Stream Hours".into(),
                            expr: stream_hour,

                            currency: None,
                        },
                        CostComponent {
                            name: "Ingestion".into(),
                            expr: ingestion,

                            currency: None,
                        },
                        CostComponent {
                            name: "Retrieval".into(),
                            expr: retrieval,

                            currency: None,
                        },
                    ],
                    required_variables: vec![
                        VariableInfo::new(id, "data_ingestion_gb", "Data ingested per month", "GB"),
                        VariableInfo::new(id, "retrieval_gb", "Data retrieved per month", "GB"),
                    ],

                    currency: Some("USD".into()),
                })
            }
        }
    }

    fn build_capacity(
        &self,
        id: &LogicalId,
        spec: &KinesisSpec,
        quotas: &Quotas,
    ) -> Option<CapacityModel> {
        let KinesisStreamMode::Provisioned { shard_count } = &spec.stream_mode else {
            return None;
        };
        let shard_count = (*shard_count)?;

        let max_mb_per_sec_per_shard = quotas
            .get(KINESIS_MAX_MB_PER_SEC_PER_SHARD)
            .unwrap_or(DEFAULT_KINESIS_MAX_MB_PER_SEC_PER_SHARD);
        let max_records_per_sec_per_shard = quotas
            .get(KINESIS_MAX_RECORDS_PER_SEC_PER_SHARD)
            .unwrap_or(DEFAULT_KINESIS_MAX_RECORDS_PER_SEC_PER_SHARD);
        let max_shards_per_stream = quotas
            .get(KINESIS_MAX_SHARDS_PER_STREAM)
            .unwrap_or(DEFAULT_KINESIS_MAX_SHARDS_PER_STREAM);

        let mut constraints = vec![
            Constraint {
                dimension: "shard_throughput".into(),
                required: Expr::ceil(Expr::div(
                    Expr::variable(id.var("peak_ingestion_mb_per_sec")),
                    Expr::constant(max_mb_per_sec_per_shard),
                )),
                limit: shard_count,
                quota_type: QuotaType::Soft,
                severity: Severity::Error,
                message_template:
                    "Required {required} shards for throughput but only {limit} provisioned".into(),
            },
            Constraint {
                dimension: "shard_record_rate".into(),
                required: Expr::ceil(Expr::div(
                    Expr::variable(id.var("peak_records_per_sec")),
                    Expr::constant(max_records_per_sec_per_shard),
                )),
                limit: shard_count,
                quota_type: QuotaType::Soft,
                severity: Severity::Error,
                message_template:
                    "Required {required} shards for record rate but only {limit} provisioned".into(),
            },
        ];

        if shard_count > max_shards_per_stream * 0.8 {
            constraints.push(Constraint {
                dimension: "shard_quota".into(),
                required: Expr::constant(shard_count),
                limit: max_shards_per_stream,
                quota_type: QuotaType::Soft,
                severity: Severity::Warning,
                message_template: "Shard count {required} is above 80% of stream quota {limit}"
                    .into(),
            });
        }

        Some(CapacityModel {
            logical_id: id.clone(),
            label: format!("Kinesis Provisioned: {id}"),
            constraints,
        })
    }
}
