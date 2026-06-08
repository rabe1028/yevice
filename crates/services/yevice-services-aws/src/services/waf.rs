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
pub struct WafSpec {
    pub rule_count: Option<f64>,
}

pub struct WafService;

impl Service for WafService {
    type Spec = WafSpec;

    fn id(&self) -> &'static str {
        "aws.waf"
    }

    fn provider(&self) -> Provider {
        Provider::Aws
    }

    fn build_cost(
        &self,
        id: &LogicalId,
        rt: &ResourceType,
        spec: &WafSpec,
        pricing: &dyn PriceCatalog,
    ) -> Result<ResourceCost, CostError> {
        let acl_price = pricing.lookup_f64(&Sku::new("aws.waf.web_acl_month_price"))?;
        let rule_price = pricing.lookup_f64(&Sku::new("aws.waf.rule_month_price"))?;
        let req_price = pricing.lookup_f64(&Sku::new("aws.waf.request_price_per_million"))?;

        let rule_count_expr = match spec.rule_count {
            Some(n) => Expr::constant(n),
            None => Expr::variable(id.var("rule_count")),
        };

        let acl_cost = Expr::constant(acl_price);
        let rule_cost = Expr::linear(rule_price, rule_count_expr, 0.0);
        let request_cost = Expr::linear(
            req_price / 1_000_000.0,
            Expr::variable(id.var("requests")),
            0.0,
        );

        let mut vars = vec![];
        if spec.rule_count.is_none() {
            vars.push(VariableInfo::new(
                id,
                "rule_count",
                "Number of WAF rules",
                "rules",
            ));
        }
        vars.push(VariableInfo::new(
            id,
            "requests",
            "Web requests per month",
            "requests",
        ));

        Ok(ResourceCost {
            logical_id: id.clone(),
            resource_type: rt.clone(),
            label: format!("WAF: {id}"),
            expr: Expr::sum(vec![
                acl_cost.clone(),
                rule_cost.clone(),
                request_cost.clone(),
            ]),
            components: vec![
                CostComponent {
                    name: "Web ACL".into(),
                    expr: acl_cost,
                },
                CostComponent {
                    name: "Rules".into(),
                    expr: rule_cost,
                },
                CostComponent {
                    name: "Requests".into(),
                    expr: request_cost,
                },
            ],
            required_variables: vars,
        })
    }
}
