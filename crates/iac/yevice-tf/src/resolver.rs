use std::collections::{BTreeSet, HashMap};

use yevice_core::parse_policy::{DiagnosticSource, IacParseDiagnostic, ParseOutcome, ParsePolicy};

use crate::{
    error::{TfError, UnresolvedSymbolKind},
    parser::{TfConfig, TfResource, TfValue},
};

#[derive(Debug)]
pub struct ResolvedConfig {
    pub resources: Vec<TfResource>,
    pub vars: HashMap<String, TfValue>,
    pub locals: HashMap<String, TfValue>,
}

/// Resolve a Terraform configuration under Strict policy (backward-compatible
/// shim).
///
/// Unresolved `var.*` / `local.*` references are tolerated (the resolver
/// leaves them as `VarRef` / `LocalRef`) and reported only via
/// `tracing::warn!`. To collect them as structured
/// [`IacParseDiagnostic`] /
/// hard error, use [`resolve_config_with_policy`].
#[allow(clippy::implicit_hasher)]
pub fn resolve_config(
    config: TfConfig,
    tfvars: Option<HashMap<String, TfValue>>,
) -> Result<ResolvedConfig, TfError> {
    // Lenient + dropping diagnostics matches the pre-#38 behaviour for callers
    // that have not yet been updated to consume `ParseOutcome`.
    let outcome = resolve_config_with_policy(config, tfvars, ParsePolicy::Lenient)?;
    Ok(outcome.value)
}

/// Resolve a Terraform configuration under the given [`ParsePolicy`].
///
/// Unresolved `var.*` / `local.*` references — i.e. variables with no default
/// and no tfvars override, and locals that cannot reach a concrete value after
/// fixed-point iteration — are policy-controlled (ADR-0003 Phase 1):
///
/// * [`ParsePolicy::Lenient`]: emit one
///   [`IacParseDiagnostic`] per
///   distinct symbol, leave the reference as `VarRef` / `LocalRef`, and let
///   adapters fall back to defaults.
/// * [`ParsePolicy::Strict`]: fail with
///   [`TfError::UnresolvedSymbol`] on
///   the first such symbol.
///
/// `TfValue::ResourceRef` (cross-resource references) is **not** considered
/// unresolved by either policy — see ADR-0003 for the rationale.
#[allow(clippy::implicit_hasher)]
pub fn resolve_config_with_policy(
    config: TfConfig,
    tfvars: Option<HashMap<String, TfValue>>,
    policy: ParsePolicy,
) -> Result<ParseOutcome<ResolvedConfig>, TfError> {
    let mut vars: HashMap<String, TfValue> = config
        .variables
        .iter()
        .filter_map(|(name, variable)| variable.default.clone().map(|value| (name.clone(), value)))
        .collect();

    if let Some(overrides) = tfvars {
        vars.extend(overrides);
    }

    let mut diagnostics: Vec<IacParseDiagnostic> = Vec::new();

    // Variables declared with no default and no tfvars override. We do **not**
    // flag these eagerly: a module may legitimately declare an unused
    // required variable (e.g. consumed by another module). The set is
    // retained so the post-resolution scan below can attach the legacy
    // "no default, no tfvars value" rendering when these names actually
    // appear in a surviving `VarRef`.
    let declared_defaultless_vars: BTreeSet<String> = config
        .variables
        .iter()
        .filter(|(name, variable)| variable.default.is_none() && !vars.contains_key(name.as_str()))
        .map(|(name, _)| name.clone())
        .collect();
    for name in &declared_defaultless_vars {
        tracing::warn!(
            variable = %name,
            "variable has no default and no tfvars value; \
             references to var.{name} will not resolve"
        );
    }

    let vars: HashMap<String, TfValue> = vars
        .iter()
        .map(|(name, value)| {
            let resolved = resolve_value(value, &vars, &HashMap::new(), 0)
                .filter(TfValue::is_concrete)
                .unwrap_or_else(|| value.clone());
            (name.clone(), resolved)
        })
        .collect();

    let mut locals = HashMap::new();
    let mut remaining = config.locals;

    loop {
        let mut progress = false;
        let keys: Vec<_> = remaining.keys().cloned().collect();
        for key in keys {
            let Some(value) = remaining.get(&key) else {
                continue;
            };

            let Some(resolved) = resolve_value(value, &vars, &locals, 0).filter(|v| {
                v.is_concrete()
                    || v.contains_resource_ref()
                    || matches!(v, TfValue::Object(_) | TfValue::Array(_))
            }) else {
                continue;
            };

            locals.insert(key.clone(), resolved);
            remaining.remove(&key);
            progress = true;
        }

        if !progress {
            break;
        }
    }

    // Locals that never reached a concrete value — either due to a cycle or
    // a reference to an unresolved var/local. Likewise retained for use
    // after the resource-level scan so we only fire diagnostics on locals
    // that actually feed into a resource.
    let unresolvable_locals: BTreeSet<String> = remaining.keys().cloned().collect();
    for name in &unresolvable_locals {
        tracing::warn!(
            local = %name,
            "local could not be resolved; references to local.{name} will not resolve"
        );
    }

    let mut resources = config.resources;
    for resource in &mut resources {
        resolve_resource(resource, &vars, &locals);
    }

    // Surviving `VarRef` / `LocalRef` after resource-level resolution.
    // These are the references that would *actually* feed a cost-model
    // adapter as `None` and produce silently-wrong numbers — they are the
    // only references worth surfacing as diagnostics. Declared-but-unused
    // required variables are deliberately excluded.
    let mut surviving_vars: BTreeSet<String> = BTreeSet::new();
    let mut surviving_locals: BTreeSet<String> = BTreeSet::new();
    for resource in &resources {
        collect_unresolved_refs(
            &resource.attrs,
            &resource.blocks,
            &vars,
            &locals,
            &mut surviving_vars,
            &mut surviving_locals,
        );
    }

    for name in &surviving_vars {
        // Render two flavors of message: the legacy "no default and no
        // tfvars" wording when the variable is in fact declared without a
        // default (this is the historical resolver warning); otherwise the
        // generic "reference to undefined variable" wording for typos /
        // cross-module refs.
        let message = if declared_defaultless_vars.contains(name) {
            format!("variable has no default and no tfvars value: var.{name} will not resolve")
        } else {
            format!("reference to undefined variable: var.{name}")
        };
        match policy {
            ParsePolicy::Strict => {
                return Err(TfError::UnresolvedSymbol {
                    kind: UnresolvedSymbolKind::Variable,
                    name: name.clone(),
                    location: None,
                });
            }
            ParsePolicy::Lenient => {
                diagnostics.push(IacParseDiagnostic::error(
                    DiagnosticSource::Tf,
                    "unresolved_var_ref",
                    message,
                ));
            }
        }
    }
    for name in &surviving_locals {
        let message = if unresolvable_locals.contains(name) {
            format!("local could not be resolved: local.{name} will not resolve")
        } else {
            format!("reference to undefined local: local.{name}")
        };
        match policy {
            ParsePolicy::Strict => {
                return Err(TfError::UnresolvedSymbol {
                    kind: UnresolvedSymbolKind::Local,
                    name: name.clone(),
                    location: None,
                });
            }
            ParsePolicy::Lenient => {
                diagnostics.push(IacParseDiagnostic::error(
                    DiagnosticSource::Tf,
                    "unresolved_local_ref",
                    message,
                ));
            }
        }
    }

    Ok(ParseOutcome::with_diagnostics(
        ResolvedConfig {
            resources,
            vars,
            locals,
        },
        diagnostics,
    ))
}

fn collect_unresolved_refs(
    attrs: &HashMap<String, TfValue>,
    blocks: &HashMap<String, Vec<HashMap<String, TfValue>>>,
    vars: &HashMap<String, TfValue>,
    locals: &HashMap<String, TfValue>,
    out_vars: &mut BTreeSet<String>,
    out_locals: &mut BTreeSet<String>,
) {
    for v in attrs.values() {
        scan_value(v, vars, locals, out_vars, out_locals);
    }
    for block_list in blocks.values() {
        for attrs in block_list {
            for v in attrs.values() {
                scan_value(v, vars, locals, out_vars, out_locals);
            }
        }
    }
}

fn scan_value(
    value: &TfValue,
    vars: &HashMap<String, TfValue>,
    locals: &HashMap<String, TfValue>,
    out_vars: &mut BTreeSet<String>,
    out_locals: &mut BTreeSet<String>,
) {
    match value {
        TfValue::VarRef(name) => {
            if !vars.contains_key(name) {
                out_vars.insert(name.clone());
            }
        }
        TfValue::LocalRef(name) => {
            if !locals.contains_key(name) {
                out_locals.insert(name.clone());
            }
        }
        TfValue::Object(map) => {
            for v in map.values() {
                scan_value(v, vars, locals, out_vars, out_locals);
            }
        }
        TfValue::Array(items) => {
            for v in items {
                scan_value(v, vars, locals, out_vars, out_locals);
            }
        }
        _ => {}
    }
}

impl ResolvedConfig {
    pub fn get_str<'a>(&self, resource: &'a TfResource, key: &str) -> Option<&'a str> {
        resource.attrs.get(key).and_then(TfValue::as_str)
    }

    pub fn get_f64(&self, resource: &TfResource, key: &str) -> Option<f64> {
        resource.attrs.get(key).and_then(TfValue::as_f64)
    }

    pub fn get_bool(&self, resource: &TfResource, key: &str) -> Option<bool> {
        resource.attrs.get(key).and_then(TfValue::as_bool)
    }
}

fn resolve_resource(
    resource: &mut TfResource,
    vars: &HashMap<String, TfValue>,
    locals: &HashMap<String, TfValue>,
) {
    for value in resource.attrs.values_mut() {
        if let Some(resolved) = resolve_value(value, vars, locals, 0)
            && (resolved.is_concrete()
                || resolved.contains_resource_ref()
                || matches!(resolved, TfValue::Object(_) | TfValue::Array(_)))
        {
            *value = resolved;
        }
    }

    for blocks in resource.blocks.values_mut() {
        for attrs in blocks {
            for value in attrs.values_mut() {
                if let Some(resolved) = resolve_value(value, vars, locals, 0)
                    && (resolved.is_concrete()
                        || resolved.contains_resource_ref()
                        || matches!(resolved, TfValue::Object(_) | TfValue::Array(_)))
                {
                    *value = resolved;
                }
            }
        }
    }
}

fn resolve_value(
    value: &TfValue,
    vars: &HashMap<String, TfValue>,
    locals: &HashMap<String, TfValue>,
    depth: usize,
) -> Option<TfValue> {
    if depth > 16 {
        return None;
    }

    match value {
        TfValue::String(_) | TfValue::Number(_) | TfValue::Bool(_) => Some(value.clone()),
        TfValue::VarRef(name) => vars
            .get(name)
            .and_then(|next| resolve_value(next, vars, locals, depth + 1)),
        TfValue::LocalRef(name) => locals
            .get(name)
            .and_then(|next| resolve_value(next, vars, locals, depth + 1)),
        // ResourceRef is a cross-resource reference; it cannot be resolved to a
        // concrete scalar value, but it must be preserved in the resource attrs for
        // connection building. Pass it through so that a local aliasing a
        // ResourceRef (e.g. `fn_arn = aws_lambda_function.fn.arn`) is stored in
        // the locals map and can later be resolved by `resolve_resource`.
        TfValue::ResourceRef { .. } => Some(value.clone()),
        TfValue::Unknown => None,
        // Recursively resolve Object/Array values. Each inner value is resolved
        // independently; unresolvable non-ref values stay as-is (the map/vec is
        // rebuilt with every entry preserved, letting the caller decide what to
        // do with remaining Unknown entries).
        TfValue::Object(map) => {
            let resolved_map = map
                .iter()
                .map(|(k, v)| {
                    let resolved =
                        resolve_value(v, vars, locals, depth + 1).unwrap_or_else(|| *v.clone());
                    (k.clone(), Box::new(resolved))
                })
                .collect();
            Some(TfValue::Object(resolved_map))
        }
        TfValue::Array(items) => {
            let resolved_items = items
                .iter()
                .map(|v| resolve_value(v, vars, locals, depth + 1).unwrap_or_else(|| v.clone()))
                .collect();
            Some(TfValue::Array(resolved_items))
        }
    }
}

#[cfg(test)]
mod defaultless_var_tests {
    use super::*;
    use crate::parser::{TfConfig, TfResource, TfValue, TfVariable};

    /// A variable declared with no default and supplied no tfvars value must
    /// remain as `TfValue::VarRef` in the resource attrs after `resolve_config`.
    /// This pins the behavior that adapters will see `None` for that attr and
    /// fall back to their hardcoded defaults (and warn accordingly).
    #[test]
    fn defaultless_var_without_tfvars_stays_unresolved() {
        let mut variables = HashMap::new();
        variables.insert("instance_type".to_string(), TfVariable { default: None });

        let mut attrs = HashMap::new();
        attrs.insert(
            "instance_type".to_string(),
            TfValue::VarRef("instance_type".to_string()),
        );

        let config = TfConfig {
            variables,
            locals: HashMap::new(),
            resources: vec![TfResource {
                resource_type: "aws_instance".to_string(),
                name: "web".to_string(),
                attrs,
                blocks: HashMap::new(),
            }],
        };

        let resolved = resolve_config(config, None).unwrap();

        let resource = &resolved.resources[0];
        let val = resource
            .attrs
            .get("instance_type")
            .expect("instance_type attr must be present");
        assert!(
            matches!(val, TfValue::VarRef(_)),
            "defaultless var without tfvars must stay as VarRef; got {val:?}"
        );
    }

    /// A variable declared with no default but supplied via tfvars resolves to
    /// the tfvars value (concrete string).
    #[test]
    fn defaultless_var_with_tfvars_resolves() {
        let mut variables = HashMap::new();
        variables.insert("instance_type".to_string(), TfVariable { default: None });

        let mut attrs = HashMap::new();
        attrs.insert(
            "instance_type".to_string(),
            TfValue::VarRef("instance_type".to_string()),
        );

        let config = TfConfig {
            variables,
            locals: HashMap::new(),
            resources: vec![TfResource {
                resource_type: "aws_instance".to_string(),
                name: "web".to_string(),
                attrs,
                blocks: HashMap::new(),
            }],
        };

        let mut tfvars = HashMap::new();
        tfvars.insert(
            "instance_type".to_string(),
            TfValue::String("m5.4xlarge".to_string()),
        );

        let resolved = resolve_config(config, Some(tfvars)).unwrap();

        let resource = &resolved.resources[0];
        let val = resource
            .attrs
            .get("instance_type")
            .expect("instance_type attr must be present");
        assert_eq!(
            val,
            &TfValue::String("m5.4xlarge".to_string()),
            "var supplied via tfvars must resolve to the concrete string; got {val:?}"
        );
    }
}

#[cfg(test)]
mod parse_policy_tests {
    use super::*;
    use crate::error::TfError;
    use crate::parser::{TfConfig, TfResource, TfValue, TfVariable};

    fn config_with_var_without_default() -> TfConfig {
        let mut variables = HashMap::new();
        variables.insert("instance_type".to_string(), TfVariable { default: None });
        let mut attrs = HashMap::new();
        attrs.insert(
            "instance_type".to_string(),
            TfValue::VarRef("instance_type".to_string()),
        );
        TfConfig {
            variables,
            locals: HashMap::new(),
            resources: vec![TfResource {
                resource_type: "aws_instance".to_string(),
                name: "web".to_string(),
                attrs,
                blocks: HashMap::new(),
            }],
        }
    }

    /// Under Lenient an undefined `var.*` is collected as a diagnostic with
    /// the stable `unresolved_var_ref` code and the resource attr stays as
    /// `VarRef` (so adapters can fall back to defaults).
    #[test]
    fn lenient_collects_unresolved_var_as_diagnostic() {
        let cfg = config_with_var_without_default();
        let outcome = resolve_config_with_policy(cfg, None, ParsePolicy::Lenient)
            .expect("lenient must not hard-error on unresolved var");
        assert!(outcome.had_errors);
        assert!(
            outcome
                .diagnostics
                .iter()
                .any(|d| d.code == "unresolved_var_ref"),
            "diagnostics: {:?}",
            outcome.diagnostics
        );
        let resource = &outcome.value.resources[0];
        assert!(
            matches!(
                resource.attrs.get("instance_type"),
                Some(TfValue::VarRef(_))
            ),
            "unresolved VarRef must be preserved for adapter fallback"
        );
    }

    /// Under Strict the same template yields `TfError::UnresolvedSymbol`.
    #[test]
    fn strict_unresolved_var_returns_unresolved_symbol_error() {
        let cfg = config_with_var_without_default();
        let err = resolve_config_with_policy(cfg, None, ParsePolicy::Strict)
            .expect_err("strict must hard-error on unresolved var");
        match err {
            TfError::UnresolvedSymbol { kind, name, .. } => {
                assert_eq!(kind, UnresolvedSymbolKind::Variable);
                assert_eq!(name, "instance_type");
            }
            other => panic!("expected UnresolvedSymbol; got {other:?}"),
        }
    }

    /// `TfValue::ResourceRef` is policy-neutral (ADR-0003): a cross-resource
    /// reference must not be treated as an unresolved symbol under Strict.
    #[test]
    fn resource_ref_is_not_treated_as_unresolved_under_strict() {
        let mut attrs = HashMap::new();
        attrs.insert(
            "subnet_id".to_string(),
            TfValue::ResourceRef {
                resource_type: "aws_subnet".to_string(),
                name: "public".to_string(),
                attr: "id".to_string(),
            },
        );
        let cfg = TfConfig {
            variables: HashMap::new(),
            locals: HashMap::new(),
            resources: vec![TfResource {
                resource_type: "aws_instance".to_string(),
                name: "web".to_string(),
                attrs,
                blocks: HashMap::new(),
            }],
        };
        let outcome = resolve_config_with_policy(cfg, None, ParsePolicy::Strict)
            .expect("ResourceRef must not be flagged as unresolved under Strict");
        assert!(!outcome.had_errors);
        assert!(outcome.diagnostics.is_empty());
    }

    /// A required (defaultless, no-tfvars) variable that is **declared but
    /// never referenced** by any resource must NOT trip Strict mode or
    /// produce a diagnostic. Pin this — the previous implementation
    /// short-circuited on declaration alone and broke modules with unused
    /// inputs.
    #[test]
    fn declared_but_unused_defaultless_var_does_not_trip_strict() {
        let mut variables = HashMap::new();
        variables.insert("unused_input".to_string(), TfVariable { default: None });
        // Declared variable is never referenced by the resource.
        let cfg = TfConfig {
            variables,
            locals: HashMap::new(),
            resources: vec![TfResource {
                resource_type: "aws_s3_bucket".to_string(),
                name: "logs".to_string(),
                attrs: HashMap::new(),
                blocks: HashMap::new(),
            }],
        };
        // Strict must succeed because no resource consumes the unset var.
        let outcome = resolve_config_with_policy(cfg, None, ParsePolicy::Strict)
            .expect("unused defaultless var must not abort Strict");
        assert!(
            outcome.diagnostics.is_empty(),
            "no diagnostics expected for unused declared var; got {:?}",
            outcome.diagnostics
        );
        assert!(!outcome.had_errors);
    }

    /// A reference to a `var.*` symbol that was never declared (typo or
    /// out-of-module ref) is reported as an unresolved diagnostic too.
    #[test]
    fn transitive_unresolved_var_is_reported() {
        let mut attrs = HashMap::new();
        attrs.insert(
            "instance_type".to_string(),
            TfValue::VarRef("undeclared_typo".to_string()),
        );
        let cfg = TfConfig {
            variables: HashMap::new(),
            locals: HashMap::new(),
            resources: vec![TfResource {
                resource_type: "aws_instance".to_string(),
                name: "web".to_string(),
                attrs,
                blocks: HashMap::new(),
            }],
        };
        let outcome = resolve_config_with_policy(cfg, None, ParsePolicy::Lenient)
            .expect("lenient must not hard-error");
        assert!(outcome.had_errors);
        assert!(
            outcome
                .diagnostics
                .iter()
                .any(|d| d.code == "unresolved_var_ref" && d.message.contains("undeclared_typo")),
            "diagnostics: {:?}",
            outcome.diagnostics
        );
    }
}

#[cfg(test)]
mod resolve_resource_object_tests {
    use std::collections::BTreeMap;

    use super::*;

    /// A `var.*` that resolves to an Object (map of scalars) must be stored in
    /// the resource attr after `resolve_resource`, not left as a `VarRef`.
    #[test]
    fn var_resolving_to_object_is_stored() {
        let mut env_vars_map: BTreeMap<String, Box<TfValue>> = BTreeMap::new();
        env_vars_map.insert(
            "KEY".to_string(),
            Box::new(TfValue::String("value".to_string())),
        );
        let env_vars_obj = TfValue::Object(env_vars_map);

        let mut vars = HashMap::new();
        vars.insert("env_vars".to_string(), env_vars_obj.clone());

        let mut attrs = HashMap::new();
        attrs.insert(
            "variables".to_string(),
            TfValue::VarRef("env_vars".to_string()),
        );

        let mut resource = TfResource {
            resource_type: "aws_lambda_function".to_string(),
            name: "fn".to_string(),
            attrs,
            blocks: HashMap::new(),
        };

        resolve_resource(&mut resource, &vars, &HashMap::new());

        let resolved = resource
            .attrs
            .get("variables")
            .expect("attr must be present");
        assert!(
            matches!(resolved, TfValue::Object(_)),
            "expected Object after resolving var.env_vars; got {resolved:?}",
        );
        // Verify the scalar inside the object is intact.
        if let TfValue::Object(map) = resolved {
            assert_eq!(
                map.get("KEY").map(Box::as_ref),
                Some(&TfValue::String("value".to_string())),
            );
        }
    }

    /// Same check for a block attr: a `var.*` → Object must be stored in
    /// the block attrs map (not stay as VarRef).
    #[test]
    fn var_resolving_to_object_is_stored_in_block() {
        let mut env_vars_map: BTreeMap<String, Box<TfValue>> = BTreeMap::new();
        env_vars_map.insert(
            "FOO".to_string(),
            Box::new(TfValue::String("bar".to_string())),
        );
        let env_vars_obj = TfValue::Object(env_vars_map);

        let mut vars = HashMap::new();
        vars.insert("env_map".to_string(), env_vars_obj);

        let mut block_attrs = HashMap::new();
        block_attrs.insert(
            "variables".to_string(),
            TfValue::VarRef("env_map".to_string()),
        );

        let mut resource = TfResource {
            resource_type: "aws_lambda_function".to_string(),
            name: "fn2".to_string(),
            attrs: HashMap::new(),
            blocks: {
                let mut b = HashMap::new();
                b.insert("environment".to_string(), vec![block_attrs]);
                b
            },
        };

        resolve_resource(&mut resource, &vars, &HashMap::new());

        let block_list = resource
            .blocks
            .get("environment")
            .expect("block must be present");
        let resolved = block_list[0]
            .get("variables")
            .expect("variables attr must be present");
        assert!(
            matches!(resolved, TfValue::Object(_)),
            "expected Object in block attr after resolving var.env_map; got {resolved:?}",
        );
    }
}
