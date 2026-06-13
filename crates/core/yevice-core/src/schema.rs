//! JSON Schema generation and template YAML generation from cost models.

use std::collections::{BTreeMap, HashSet};

use serde::Serialize;

use crate::cost::ArchitectureCost;
use crate::types::VariableName;

/// Compute the set of binding targets that are **derivable** — i.e., their
/// expression can be evaluated solely from modeled variables or other
/// derivable targets.
///
/// A target is derivable when every variable in `b.expr.variables()` is either:
/// - a modeled variable that is **not itself a binding target** (i.e. a
///   genuine user-supplied seed), **or**
/// - itself already derivable (fixed-point closure).
///
/// Using `modeled_vars − binding_targets` as the seed set ensures that
/// **circular bindings** (e.g. A_requests = B_requests and B_requests =
/// A_requests, where both are modeled) are treated correctly: neither side
/// qualifies as a derivable seed, so both remain in the schema and the user
/// can supply one to break the cycle.
///
/// Binding targets that depend on external (unmodeled) sources are *not*
/// derivable and must remain in the schema so the user can supply them.
fn compute_derivable_targets(arch: &ArchitectureCost) -> HashSet<VariableName> {
    // Step 1: collect all variable names that are modeled (appear as a
    // required_variable in some resource).
    let modeled_vars: HashSet<&VariableName> = arch
        .resources
        .iter()
        .flat_map(|r| r.required_variables.iter().map(|v| &v.name))
        .collect();

    // Step 2: collect all binding target names.
    let binding_targets: HashSet<&VariableName> = arch.bindings.iter().map(|b| &b.target).collect();

    // Step 3: the base "already known" seed set is modeled vars that are NOT
    // themselves binding targets.  Binding targets are derived values, not
    // user-supplied seeds; including them in the seed would incorrectly mark
    // circularly-bound variables as derivable.
    let seeds: HashSet<&VariableName> =
        modeled_vars.difference(&binding_targets).copied().collect();

    // Step 4: fixed-point iteration — a binding target is derivable when all
    // variables it references are either seeds or already derivable.
    let mut derivable: HashSet<VariableName> = HashSet::new();
    loop {
        let prev_len = derivable.len();
        for b in &arch.bindings {
            if derivable.contains(&b.target) {
                continue;
            }
            let all_known = b
                .expr
                .variables()
                .iter()
                .all(|v| seeds.contains(v) || derivable.contains(v));
            if all_known {
                derivable.insert(b.target.clone());
            }
        }
        if derivable.len() == prev_len {
            break;
        }
    }

    derivable
}

/// JSON Schema for the hierarchical usage parameters file.
///
/// Structure:
/// ```yaml
/// IngestFunction:
///   requests: 5000000
///   avg_duration_ms: 200
/// DataTable:
///   write_request_units: 500000
/// ```
#[derive(Debug, Serialize)]
pub struct UsageSchema {
    #[serde(rename = "$schema")]
    pub schema: String,
    pub title: String,
    pub description: String,
    #[serde(rename = "type")]
    pub schema_type: String,
    pub properties: BTreeMap<String, ResourceSchema>,
    pub required: Vec<String>,
    #[serde(rename = "additionalProperties")]
    pub additional_properties: bool,
}

#[derive(Debug, Serialize)]
pub struct ResourceSchema {
    #[serde(rename = "type")]
    pub schema_type: String,
    pub description: String,
    pub properties: BTreeMap<String, PropertySchema>,
    pub required: Vec<String>,
    #[serde(rename = "additionalProperties")]
    pub additional_properties: bool,
}

#[derive(Debug, Serialize)]
pub struct PropertySchema {
    #[serde(rename = "type")]
    pub schema_type: String,
    pub description: String,
}

/// Generate a JSON Schema from an `ArchitectureCost`.
pub fn generate_usage_schema(arch: &ArchitectureCost) -> UsageSchema {
    let mut properties = BTreeMap::new();
    let mut required = Vec::new();

    let derivable = compute_derivable_targets(arch);

    for resource in &arch.resources {
        let logical_id = resource.logical_id.to_string();
        let prefix = format!("{logical_id}_");

        let mut resource_props = BTreeMap::new();
        let mut resource_required = Vec::new();

        for var in &resource.required_variables {
            if derivable.contains(&var.name) {
                continue;
            }
            let var_name = var.name.to_string();
            let short_name = var_name.strip_prefix(&prefix).unwrap_or(&var_name);

            resource_props.insert(
                short_name.to_string(),
                PropertySchema {
                    schema_type: "number".to_string(),
                    description: format!("{} [{}]", var.description, var.unit),
                },
            );
            resource_required.push(short_name.to_string());
        }

        if resource_props.is_empty() {
            continue;
        }

        required.push(logical_id.clone());
        properties.insert(
            logical_id,
            ResourceSchema {
                schema_type: "object".to_string(),
                description: resource.label.clone(),
                properties: resource_props,
                required: resource_required,
                additional_properties: false,
            },
        );
    }

    UsageSchema {
        schema: "https://json-schema.org/draft/2020-12/schema".to_string(),
        title: format!("Usage parameters for {}", arch.name),
        description: "Usage parameters for cost evaluation".to_string(),
        schema_type: "object".to_string(),
        properties,
        required,
        additional_properties: true,
    }
}

/// Generate a template usage YAML with placeholder values.
///
/// Binding target variables are excluded; only user-supplied usage inputs
/// appear in the template.
pub fn generate_usage_template(arch: &ArchitectureCost) -> String {
    let mut lines = Vec::new();
    lines.push(format!("# Usage parameters for: {}", arch.name));
    lines.push(format!("# Region: {}", arch.region));
    lines.push(String::new());

    let derivable = compute_derivable_targets(arch);

    for resource in &arch.resources {
        let logical_id = resource.logical_id.to_string();
        let prefix = format!("{logical_id}_");

        let vars: Vec<_> = resource
            .required_variables
            .iter()
            .filter(|v| !derivable.contains(&v.name))
            .collect();

        if vars.is_empty() {
            continue;
        }

        lines.push(format!("# {}", resource.label));
        lines.push(format!("{logical_id}:"));

        for var in vars {
            let var_name = var.name.to_string();
            let short_name = var_name.strip_prefix(&prefix).unwrap_or(&var_name);
            lines.push(format!(
                "  {short_name}: 0  # {} [{}]",
                var.description, var.unit
            ));
        }
        lines.push(String::new());
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cost::{ResourceCost, VariableBinding, VariableInfo};
    use crate::expr::Expr;
    use crate::topology::Topology;
    use crate::types::{ArchitectureName, LogicalId, Region, ResourceType};

    fn make_arch_with_binding() -> ArchitectureCost {
        let lambda = LogicalId::new("MyFunction");
        let queue = LogicalId::new("MyQueue");

        // MyFunction has two variables: requests (bound) and avg_duration_ms (not bound)
        let function_resource = ResourceCost {
            logical_id: lambda.clone(),
            resource_type: ResourceType::new("AWS::Lambda::Function"),
            label: "My Lambda Function".to_string(),
            expr: Expr::constant(0.0),
            components: vec![],
            required_variables: vec![
                VariableInfo::new(&lambda, "requests", "Invocation count", "count"),
                VariableInfo::new(&lambda, "avg_duration_ms", "Avg duration", "ms"),
            ],
        };

        // MyQueue has one variable: requests (not bound)
        let queue_resource = ResourceCost {
            logical_id: queue.clone(),
            resource_type: ResourceType::new("AWS::SQS::Queue"),
            label: "My SQS Queue".to_string(),
            expr: Expr::constant(0.0),
            components: vec![],
            required_variables: vec![VariableInfo::new(
                &queue,
                "requests",
                "Message count",
                "count",
            )],
        };

        // Binding: MyFunction_requests is derived from MyQueue_requests
        let binding = VariableBinding {
            target: lambda.var("requests"),
            expr: Expr::variable(queue.var("requests")),
            description: "SQS -> Lambda".to_string(),
            source: "SQS -> Lambda (MyQueue -> MyFunction)".to_string(),
        };

        ArchitectureCost {
            name: ArchitectureName::new("test"),
            resources: vec![function_resource, queue_resource],
            bindings: vec![binding],
            region: Region::new("ap-northeast-1"),
            topology: Topology::default(),
            diagnostics: Vec::new(),
        }
    }

    #[test]
    fn schema_excludes_binding_targets() {
        let arch = make_arch_with_binding();
        let schema = generate_usage_schema(&arch);

        // MyFunction still appears because avg_duration_ms is not bound
        assert!(
            schema.properties.contains_key("MyFunction"),
            "MyFunction should appear (has non-bound variables)"
        );
        let func = &schema.properties["MyFunction"];

        // requests is a binding target and must be absent
        assert!(
            !func.properties.contains_key("requests"),
            "binding target 'requests' must be excluded from schema"
        );

        // avg_duration_ms is not bound and must be present
        assert!(
            func.properties.contains_key("avg_duration_ms"),
            "non-bound 'avg_duration_ms' must remain in schema"
        );
        assert_eq!(
            func.required.len(),
            1,
            "only avg_duration_ms should be required"
        );

        // MyQueue is not a binding target; its requests variable must appear
        assert!(
            schema.properties.contains_key("MyQueue"),
            "MyQueue should appear"
        );
        assert!(
            schema.properties["MyQueue"]
                .properties
                .contains_key("requests"),
            "MyQueue_requests is not bound and must appear"
        );
    }

    #[test]
    fn schema_omits_resource_when_all_variables_are_bound() {
        let lambda = LogicalId::new("OnlyBound");

        let resource = ResourceCost {
            logical_id: lambda.clone(),
            resource_type: ResourceType::new("AWS::Lambda::Function"),
            label: "Fully-bound function".to_string(),
            expr: Expr::constant(0.0),
            components: vec![],
            required_variables: vec![VariableInfo::new(
                &lambda,
                "requests",
                "Invocation count",
                "count",
            )],
        };

        let binding = VariableBinding {
            target: lambda.var("requests"),
            expr: Expr::constant(1000.0),
            description: "derived".to_string(),
            source: "test".to_string(),
        };

        let arch = ArchitectureCost {
            name: ArchitectureName::new("test"),
            resources: vec![resource],
            bindings: vec![binding],
            region: Region::new("ap-northeast-1"),
            topology: Topology::default(),
            diagnostics: Vec::new(),
        };

        let schema = generate_usage_schema(&arch);
        assert!(
            !schema.properties.contains_key("OnlyBound"),
            "resource with all variables bound must be absent from schema"
        );
        assert!(
            schema.required.is_empty(),
            "no resources should be required when all are fully bound"
        );
    }

    #[test]
    fn template_excludes_binding_targets() {
        let arch = make_arch_with_binding();
        let template = generate_usage_template(&arch);

        // MyFunction section present (has avg_duration_ms)
        assert!(
            template.contains("MyFunction:"),
            "MyFunction section should appear"
        );
        // avg_duration_ms must appear
        assert!(
            template.contains("  avg_duration_ms: 0"),
            "avg_duration_ms should be in template"
        );
        // requests must NOT appear under MyFunction (it is a binding target)
        // We check that 'requests: 0' doesn't appear in the MyFunction block.
        // Since MyQueue_requests also appears in the queue section, we verify
        // that the MyFunction block specifically doesn't contain it by checking
        // the template does not include "  requests: 0" under MyFunction.
        // The simplest approach: MyQueue section does appear with requests, but
        // binding target MyFunction_requests must not appear as a placeholder.
        let function_section_start = template.find("MyFunction:").expect("MyFunction: not found");
        let after_function = &template[function_section_start..];
        let next_section = after_function[1..]
            .find('\n')
            .map_or(after_function.len(), |i| i + 1);
        // collect lines of the MyFunction block (until next top-level key)
        let block: Vec<&str> = after_function[next_section..]
            .lines()
            .take_while(|l| l.starts_with("  ") || l.is_empty())
            .collect();
        assert!(
            !block.iter().any(|l| l.contains("requests:")),
            "binding target 'requests' must not appear in MyFunction template block, block: {block:?}"
        );

        // MyQueue section must appear with requests (not a binding target)
        assert!(
            template.contains("MyQueue:"),
            "MyQueue section should appear"
        );
    }

    #[test]
    fn template_omits_resource_when_all_variables_are_bound() {
        let lambda = LogicalId::new("OnlyBound");

        let resource = ResourceCost {
            logical_id: lambda.clone(),
            resource_type: ResourceType::new("AWS::Lambda::Function"),
            label: "Fully-bound function".to_string(),
            expr: Expr::constant(0.0),
            components: vec![],
            required_variables: vec![VariableInfo::new(
                &lambda,
                "requests",
                "Invocation count",
                "count",
            )],
        };

        let binding = VariableBinding {
            target: lambda.var("requests"),
            expr: Expr::constant(1000.0),
            description: "derived".to_string(),
            source: "test".to_string(),
        };

        let arch = ArchitectureCost {
            name: ArchitectureName::new("test"),
            resources: vec![resource],
            bindings: vec![binding],
            region: Region::new("ap-northeast-1"),
            topology: Topology::default(),
            diagnostics: Vec::new(),
        };

        let template = generate_usage_template(&arch);
        assert!(
            !template.contains("OnlyBound:"),
            "resource with all variables bound must be absent from template"
        );
    }

    /// When a binding's source is an external (unmodeled) resource, the target
    /// is NOT derivable and must remain in the schema so the user can supply it.
    ///
    /// Scenario: `AWS::Lambda::EventSourceMapping` binds
    /// `Function_requests = ExternalQueue_requests`, but `ExternalQueue` is not
    /// a modeled resource (it lives outside the template).  Neither the target
    /// (`Function_requests`) nor the source (`ExternalQueue_requests`) is in
    /// `modeled_vars`, so the target cannot be derived and must stay in schema.
    #[test]
    fn schema_keeps_non_derivable_binding_target_when_source_is_external() {
        let lambda = LogicalId::new("MyFunction");
        // ExternalQueue is intentionally NOT added as a resource.
        let external_queue = LogicalId::new("ExternalQueue");

        let function_resource = ResourceCost {
            logical_id: lambda.clone(),
            resource_type: ResourceType::new("AWS::Lambda::Function"),
            label: "My Lambda Function".to_string(),
            expr: Expr::constant(0.0),
            components: vec![],
            required_variables: vec![VariableInfo::new(
                &lambda,
                "requests",
                "Invocation count",
                "count",
            )],
        };

        // Binding target is Function_requests, but its expr references
        // ExternalQueue_requests which is NOT in any resource's required_variables.
        let binding = VariableBinding {
            target: lambda.var("requests"),
            expr: Expr::variable(external_queue.var("requests")),
            description: "ESM -> Lambda".to_string(),
            source: "external SQS -> MyFunction".to_string(),
        };

        let arch = ArchitectureCost {
            name: ArchitectureName::new("test"),
            resources: vec![function_resource],
            bindings: vec![binding],
            region: Region::new("ap-northeast-1"),
            topology: Topology::default(),
            diagnostics: Vec::new(),
        };

        let schema = generate_usage_schema(&arch);

        // MyFunction must appear because its binding target is NOT derivable.
        assert!(
            schema.properties.contains_key("MyFunction"),
            "MyFunction should appear when binding source is external"
        );
        // The non-derivable target must be present for the user to supply.
        assert!(
            schema.properties["MyFunction"]
                .properties
                .contains_key("requests"),
            "non-derivable binding target 'requests' must remain in schema"
        );
    }

    /// Circular bindings: A_requests = B_requests and B_requests = A_requests,
    /// both variables are modeled.  Neither is derivable (the cycle has no
    /// external seed), so both must remain in the schema so the user can
    /// supply one to break the cycle.
    #[test]
    fn schema_keeps_both_vars_in_circular_binding() {
        let fn_a = LogicalId::new("FnA");
        let fn_b = LogicalId::new("FnB");

        let resource_a = ResourceCost {
            logical_id: fn_a.clone(),
            resource_type: ResourceType::new("AWS::Lambda::Function"),
            label: "Function A".to_string(),
            expr: Expr::constant(0.0),
            components: vec![],
            required_variables: vec![VariableInfo::new(
                &fn_a,
                "requests",
                "Invocation count",
                "count",
            )],
        };
        let resource_b = ResourceCost {
            logical_id: fn_b.clone(),
            resource_type: ResourceType::new("AWS::Lambda::Function"),
            label: "Function B".to_string(),
            expr: Expr::constant(0.0),
            components: vec![],
            required_variables: vec![VariableInfo::new(
                &fn_b,
                "requests",
                "Invocation count",
                "count",
            )],
        };

        // Circular: FnA_requests = FnB_requests, FnB_requests = FnA_requests.
        let binding_a = VariableBinding {
            target: fn_a.var("requests"),
            expr: Expr::variable(fn_b.var("requests")),
            description: "A <- B".to_string(),
            source: "circular".to_string(),
        };
        let binding_b = VariableBinding {
            target: fn_b.var("requests"),
            expr: Expr::variable(fn_a.var("requests")),
            description: "B <- A".to_string(),
            source: "circular".to_string(),
        };

        let arch = ArchitectureCost {
            name: ArchitectureName::new("test"),
            resources: vec![resource_a, resource_b],
            bindings: vec![binding_a, binding_b],
            region: Region::new("ap-northeast-1"),
            topology: Topology::default(),
            diagnostics: Vec::new(),
        };

        let schema = generate_usage_schema(&arch);

        // Both resources must appear because neither target is derivable
        // (the cycle has no seed).
        assert!(
            schema.properties.contains_key("FnA"),
            "FnA must appear in schema for circular binding"
        );
        assert!(
            schema.properties.contains_key("FnB"),
            "FnB must appear in schema for circular binding"
        );
        assert!(
            schema.properties["FnA"].properties.contains_key("requests"),
            "FnA.requests must remain in schema (circular, not derivable)"
        );
        assert!(
            schema.properties["FnB"].properties.contains_key("requests"),
            "FnB.requests must remain in schema (circular, not derivable)"
        );
    }

    /// Template counterpart of the circular binding test.
    #[test]
    fn template_keeps_both_vars_in_circular_binding() {
        let fn_a = LogicalId::new("FnA");
        let fn_b = LogicalId::new("FnB");

        let resource_a = ResourceCost {
            logical_id: fn_a.clone(),
            resource_type: ResourceType::new("AWS::Lambda::Function"),
            label: "Function A".to_string(),
            expr: Expr::constant(0.0),
            components: vec![],
            required_variables: vec![VariableInfo::new(
                &fn_a,
                "requests",
                "Invocation count",
                "count",
            )],
        };
        let resource_b = ResourceCost {
            logical_id: fn_b.clone(),
            resource_type: ResourceType::new("AWS::Lambda::Function"),
            label: "Function B".to_string(),
            expr: Expr::constant(0.0),
            components: vec![],
            required_variables: vec![VariableInfo::new(
                &fn_b,
                "requests",
                "Invocation count",
                "count",
            )],
        };

        let binding_a = VariableBinding {
            target: fn_a.var("requests"),
            expr: Expr::variable(fn_b.var("requests")),
            description: "A <- B".to_string(),
            source: "circular".to_string(),
        };
        let binding_b = VariableBinding {
            target: fn_b.var("requests"),
            expr: Expr::variable(fn_a.var("requests")),
            description: "B <- A".to_string(),
            source: "circular".to_string(),
        };

        let arch = ArchitectureCost {
            name: ArchitectureName::new("test"),
            resources: vec![resource_a, resource_b],
            bindings: vec![binding_a, binding_b],
            region: Region::new("ap-northeast-1"),
            topology: Topology::default(),
            diagnostics: Vec::new(),
        };

        let template = generate_usage_template(&arch);

        assert!(
            template.contains("FnA:"),
            "FnA section must appear in template for circular binding"
        );
        assert!(
            template.contains("FnB:"),
            "FnB section must appear in template for circular binding"
        );
    }

    /// Template counterpart of the external-source test above.
    #[test]
    fn template_keeps_non_derivable_binding_target_when_source_is_external() {
        let lambda = LogicalId::new("MyFunction");
        let external_queue = LogicalId::new("ExternalQueue");

        let function_resource = ResourceCost {
            logical_id: lambda.clone(),
            resource_type: ResourceType::new("AWS::Lambda::Function"),
            label: "My Lambda Function".to_string(),
            expr: Expr::constant(0.0),
            components: vec![],
            required_variables: vec![VariableInfo::new(
                &lambda,
                "requests",
                "Invocation count",
                "count",
            )],
        };

        let binding = VariableBinding {
            target: lambda.var("requests"),
            expr: Expr::variable(external_queue.var("requests")),
            description: "ESM -> Lambda".to_string(),
            source: "external SQS -> MyFunction".to_string(),
        };

        let arch = ArchitectureCost {
            name: ArchitectureName::new("test"),
            resources: vec![function_resource],
            bindings: vec![binding],
            region: Region::new("ap-northeast-1"),
            topology: Topology::default(),
            diagnostics: Vec::new(),
        };

        let template = generate_usage_template(&arch);

        assert!(
            template.contains("MyFunction:"),
            "MyFunction section should appear when binding source is external"
        );
        assert!(
            template.contains("  requests: 0"),
            "non-derivable binding target 'requests' must appear in template"
        );
    }
}
