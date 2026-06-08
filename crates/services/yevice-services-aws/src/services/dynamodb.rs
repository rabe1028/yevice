use serde::{Deserialize, Serialize};
use yevice_core::{
    capacity::{CapacityModel, Constraint, QuotaType, Quotas, Severity},
    cost::{CostComponent, ResourceCost, VariableInfo},
    expr::{Expr, Tier},
    resource::Provider,
    types::{LogicalId, ResourceType},
};
use yevice_pricing::catalog::{PriceCatalog, Sku};
use yevice_service_api::{Service, error::CostError};

use crate::quotas::{
    DEFAULT_DYNAMODB_MAX_WCU_PER_TABLE, DEFAULT_DYNAMODB_ONDEMAND_INITIAL_THROUGHPUT,
    DYNAMODB_MAX_WCU_PER_TABLE, DYNAMODB_ONDEMAND_INITIAL_THROUGHPUT,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DynamoDbBillingMode {
    OnDemand,
    Provisioned {
        write_capacity_units: Option<f64>,
        read_capacity_units: Option<f64>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DynamoDbSpec {
    pub billing_mode: DynamoDbBillingMode,
    pub has_stream: bool,
    pub gsi_count: usize,
}

pub struct DynamoDbService;

const HOURS_PER_MONTH: f64 = 730.0;

impl Service for DynamoDbService {
    type Spec = DynamoDbSpec;

    fn id(&self) -> &'static str {
        "aws.dynamodb"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &DynamoDbSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let write_request_price =
            pricing.lookup_f64(&Sku::new("aws.dynamodb.write_request_price"))?;
        let read_request_price =
            pricing.lookup_f64(&Sku::new("aws.dynamodb.read_request_price"))?;
        let wcu_hour_price = pricing.lookup_f64(&Sku::new("aws.dynamodb.wcu_hour_price"))?;
        let rcu_hour_price = pricing.lookup_f64(&Sku::new("aws.dynamodb.rcu_hour_price"))?;
        let storage_price_per_gb =
            pricing.lookup_f64(&Sku::new("aws.dynamodb.storage_price_per_gb"))?;
        let free_tier_wru = pricing.lookup_f64(&Sku::new("aws.dynamodb.free_tier_wru"))?;
        let free_tier_rru = pricing.lookup_f64(&Sku::new("aws.dynamodb.free_tier_rru"))?;
        let free_tier_storage_gb =
            pricing.lookup_f64(&Sku::new("aws.dynamodb.free_tier_storage_gb"))?;

        let storage_cost = Expr::tiered(
            vec![
                Tier {
                    upper_limit: Some(free_tier_storage_gb),
                    unit_price: 0.0,
                },
                Tier {
                    upper_limit: None,
                    unit_price: storage_price_per_gb,
                },
            ],
            Expr::variable(id.var("storage_gb")),
        );

        match &spec.billing_mode {
            DynamoDbBillingMode::OnDemand => {
                let write_cost = Expr::tiered(
                    vec![
                        Tier {
                            upper_limit: Some(free_tier_wru),
                            unit_price: 0.0,
                        },
                        Tier {
                            upper_limit: None,
                            unit_price: write_request_price,
                        },
                    ],
                    Expr::variable(id.var("write_request_units")),
                );
                let read_cost = Expr::tiered(
                    vec![
                        Tier {
                            upper_limit: Some(free_tier_rru),
                            unit_price: 0.0,
                        },
                        Tier {
                            upper_limit: None,
                            unit_price: read_request_price,
                        },
                    ],
                    Expr::variable(id.var("read_request_units")),
                );

                Ok(ResourceCost {
                    logical_id: id.clone(),
                    resource_type: rt.clone(),
                    label: format!("DynamoDB On-Demand: {id}"),
                    expr: Expr::sum(vec![
                        write_cost.clone(),
                        read_cost.clone(),
                        storage_cost.clone(),
                    ]),
                    components: vec![
                        CostComponent {
                            name: "Write Request Units".into(),
                            expr: write_cost,
                        },
                        CostComponent {
                            name: "Read Request Units".into(),
                            expr: read_cost,
                        },
                        CostComponent {
                            name: "Storage".into(),
                            expr: storage_cost,
                        },
                    ],
                    required_variables: vec![
                        VariableInfo::new(
                            id,
                            "write_request_units",
                            "Write request units per month",
                            "WRU",
                        ),
                        VariableInfo::new(
                            id,
                            "read_request_units",
                            "Read request units per month",
                            "RRU",
                        ),
                        VariableInfo::new(id, "storage_gb", "Table storage", "GB"),
                    ],
                })
            }
            DynamoDbBillingMode::Provisioned {
                write_capacity_units,
                read_capacity_units,
            } => {
                let wcu_expr = match write_capacity_units {
                    Some(n) => Expr::constant(*n),
                    None => Expr::variable(id.var("write_capacity_units")),
                };
                let rcu_expr = match read_capacity_units {
                    Some(n) => Expr::constant(*n),
                    None => Expr::variable(id.var("read_capacity_units")),
                };

                let write_cost = Expr::linear(wcu_hour_price * HOURS_PER_MONTH, wcu_expr, 0.0);
                let read_cost = Expr::linear(rcu_hour_price * HOURS_PER_MONTH, rcu_expr, 0.0);

                let mut vars = vec![VariableInfo::new(id, "storage_gb", "Table storage", "GB")];
                if write_capacity_units.is_none() {
                    vars.insert(
                        0,
                        VariableInfo::new(
                            id,
                            "write_capacity_units",
                            "Provisioned write capacity units",
                            "WCU",
                        ),
                    );
                }
                if read_capacity_units.is_none() {
                    let pos = usize::from(write_capacity_units.is_none());
                    vars.insert(
                        pos,
                        VariableInfo::new(
                            id,
                            "read_capacity_units",
                            "Provisioned read capacity units",
                            "RCU",
                        ),
                    );
                }

                Ok(ResourceCost {
                    logical_id: id.clone(),
                    resource_type: rt.clone(),
                    label: format!("DynamoDB Provisioned: {id}"),
                    expr: Expr::sum(vec![
                        write_cost.clone(),
                        read_cost.clone(),
                        storage_cost.clone(),
                    ]),
                    components: vec![
                        CostComponent {
                            name: "Write Capacity".into(),
                            expr: write_cost,
                        },
                        CostComponent {
                            name: "Read Capacity".into(),
                            expr: read_cost,
                        },
                        CostComponent {
                            name: "Storage".into(),
                            expr: storage_cost,
                        },
                    ],
                    required_variables: vars,
                })
            }
        }
    }

    fn build_capacity(
        &self,
        id: &LogicalId,
        spec: &DynamoDbSpec,
        quotas: &Quotas,
    ) -> Option<CapacityModel> {
        let max_wcu = quotas
            .get(DYNAMODB_MAX_WCU_PER_TABLE)
            .unwrap_or(DEFAULT_DYNAMODB_MAX_WCU_PER_TABLE);

        match &spec.billing_mode {
            DynamoDbBillingMode::Provisioned {
                write_capacity_units,
                read_capacity_units,
            } => {
                let mut constraints = Vec::new();

                if let Some(wcu) = write_capacity_units {
                    constraints.push(Constraint {
                        dimension: "write_capacity_units".into(),
                        required: Expr::variable(id.var("peak_writes_per_sec")),
                        limit: *wcu,
                        quota_type: QuotaType::Soft,
                        severity: Severity::Error,
                        message_template:
                            "Peak write demand {required} WCU exceeds provisioned {limit} WCU"
                                .into(),
                    });

                    if *wcu > max_wcu * 0.8 {
                        constraints.push(Constraint {
                            dimension: "wcu_quota".into(),
                            required: Expr::constant(*wcu),
                            limit: max_wcu,
                            quota_type: QuotaType::Soft,
                            severity: Severity::Warning,
                            message_template:
                                "Provisioned WCU {required} is above 80% of table quota {limit}"
                                    .into(),
                        });
                    }
                }

                if let Some(rcu) = read_capacity_units {
                    constraints.push(Constraint {
                        dimension: "read_capacity_units".into(),
                        required: Expr::variable(id.var("peak_reads_per_sec")),
                        limit: *rcu,
                        quota_type: QuotaType::Soft,
                        severity: Severity::Error,
                        message_template:
                            "Peak read demand {required} RCU exceeds provisioned {limit} RCU"
                                .into(),
                    });
                }

                if constraints.is_empty() {
                    return None;
                }

                Some(CapacityModel {
                    logical_id: id.clone(),
                    label: format!("DynamoDB Provisioned: {id}"),
                    constraints,
                })
            }
            DynamoDbBillingMode::OnDemand => Some(CapacityModel {
                logical_id: id.clone(),
                label: format!("DynamoDB On-Demand: {id}"),
                constraints: vec![Constraint {
                    dimension: "peak_writes_per_sec".into(),
                    required: Expr::variable(id.var("peak_writes_per_sec")),
                    limit: quotas
                        .get(DYNAMODB_ONDEMAND_INITIAL_THROUGHPUT)
                        .unwrap_or(DEFAULT_DYNAMODB_ONDEMAND_INITIAL_THROUGHPUT),
                    quota_type: QuotaType::Soft,
                    severity: Severity::Warning,
                    message_template:
                        "Peak writes {required}/sec may exceed On-Demand initial limit of {limit}/sec (auto-scales but takes time)"
                            .into(),
                }],
            }),
        }
    }
}
