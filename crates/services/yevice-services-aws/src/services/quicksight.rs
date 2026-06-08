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
pub struct QuickSightSpec {
    pub creators: Option<f64>,
    pub viewer_users: Option<f64>,
    pub sessions_per_user: Option<f64>,
    pub spice_gb: Option<f64>,
    /// QuickSight billing is account-level. Only the anchor resource carries
    /// the subscription/SPICE cost; other QuickSight resources are structural
    /// ($0) so multi-resource stacks do not double-count. Defaults to `true`
    /// for backward compatibility when absent.
    #[serde(default = "default_true")]
    pub is_cost_anchor: bool,
}

fn default_true() -> bool {
    true
}

pub struct QuickSightService;

impl Service for QuickSightService {
    type Spec = QuickSightSpec;

    fn id(&self) -> &'static str {
        "aws.quicksight"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &QuickSightSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        // Non-anchor QuickSight resources (Dashboard/DataSet/Template) are
        // structural — the account-level cost is billed once on the anchor.
        if !spec.is_cost_anchor {
            return Ok(ResourceCost {
                logical_id: id.clone(),
                resource_type: rt.clone(),
                label: format!("QuickSight (structural): {id}"),
                expr: Expr::constant(0.0),
                components: vec![],
                required_variables: vec![],
            });
        }

        let creator_price = pricing.lookup_f64(&Sku::new("aws.quicksight.creator_month_price"))?;
        let viewer_session_price =
            pricing.lookup_f64(&Sku::new("aws.quicksight.viewer_session_price"))?;
        let viewer_max_month_price =
            pricing.lookup_f64(&Sku::new("aws.quicksight.viewer_max_month_price"))?;
        let spice_gb_price =
            pricing.lookup_f64(&Sku::new("aws.quicksight.spice_gb_month_price"))?;
        let free_spice_gb = pricing.lookup_f64(&Sku::new("aws.quicksight.free_spice_gb"))?;

        // Per-user viewer cost is capped at viewer_max_month_price ($5/user).
        // The cap expressed in sessions = max_price / session_price.
        let viewer_cap_sessions = viewer_max_month_price / viewer_session_price;

        // Creator cost: creators * creator_price
        let creators_expr = match spec.creators {
            Some(n) => Expr::constant(n),
            None => Expr::variable(id.var("creators")),
        };
        let creator_cost =
            Expr::product(vec![creators_expr.clone(), Expr::constant(creator_price)]);

        // Viewer cost with per-user monthly cap ($5.00/user):
        //   viewer_users * min(sessions_per_user * $0.30, $5.00)
        // Implemented as tiered: first 16.67 sessions at $0.30, rest at $0.00
        let users_expr = match spec.viewer_users {
            Some(n) => Expr::constant(n),
            None => Expr::variable(id.var("viewer_users")),
        };
        let sessions_expr = match spec.sessions_per_user {
            Some(s) => Expr::constant(s),
            None => Expr::variable(id.var("sessions_per_user")),
        };

        // Per-user capped cost using tiered pricing
        let per_user_viewer = Expr::tiered(
            vec![
                yevice_core::expr::Tier {
                    upper_limit: Some(viewer_cap_sessions),
                    unit_price: viewer_session_price,
                },
                yevice_core::expr::Tier {
                    upper_limit: None,
                    unit_price: 0.0,
                },
            ],
            sessions_expr.clone(),
        );

        // Total viewer cost = users * per_user_cost
        let viewer_cost = Expr::product(vec![users_expr.clone(), per_user_viewer]);

        // SPICE capacity cost: max(0, (spice_gb - free_spice_gb * creators) * spice_gb_price).
        // The free allocation (10 GB) is included *per creator/author*, so it
        // scales with the creator count rather than being a single flat 10 GB.
        let spice_expr = match spec.spice_gb {
            Some(s) => Expr::constant(s),
            None => Expr::variable(id.var("spice_gb")),
        };
        let free_spice = Expr::product(vec![creators_expr.clone(), Expr::constant(-free_spice_gb)]);
        let spice_over_free = Expr::sum(vec![spice_expr.clone(), free_spice]);
        let spice_cost = Expr::tiered(
            vec![
                yevice_core::expr::Tier {
                    upper_limit: Some(0.0),
                    unit_price: 0.0,
                },
                yevice_core::expr::Tier {
                    upper_limit: None,
                    unit_price: spice_gb_price,
                },
            ],
            spice_over_free,
        );

        let mut vars = vec![
            VariableInfo::new(id, "creators", "Number of QuickSight creators", "users"),
            VariableInfo::new(id, "viewer_users", "Number of QuickSight viewers", "users"),
            VariableInfo::new(
                id,
                "sessions_per_user",
                "Sessions per viewer per month",
                "sessions",
            ),
        ];
        if spec.spice_gb.is_none() {
            vars.push(VariableInfo::new(
                id,
                "spice_gb",
                "Total SPICE capacity (the included free allocation is deducted automatically)",
                "GB",
            ));
        }

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("QuickSight: {id}"),
            expr: Expr::sum(vec![
                creator_cost.clone(),
                viewer_cost.clone(),
                spice_cost.clone(),
            ]),
            components: vec![
                CostComponent {
                    name: "Creators".into(),
                    expr: creator_cost,
                },
                CostComponent {
                    name: "Viewer Sessions (capped $5/user)".into(),
                    expr: viewer_cost,
                },
                CostComponent {
                    name: "SPICE Capacity".into(),
                    expr: spice_cost,
                },
            ],
            required_variables: vars,
        })
    }
}
