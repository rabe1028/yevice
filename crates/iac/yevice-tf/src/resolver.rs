use std::collections::HashMap;

use crate::{
    error::TfError,
    parser::{TfConfig, TfResource, TfValue},
};

#[derive(Debug)]
pub struct ResolvedConfig {
    pub resources: Vec<TfResource>,
    pub vars: HashMap<String, TfValue>,
    pub locals: HashMap<String, TfValue>,
}

#[allow(clippy::implicit_hasher)]
pub fn resolve_config(
    config: TfConfig,
    tfvars: Option<HashMap<String, TfValue>>,
) -> Result<ResolvedConfig, TfError> {
    let mut vars: HashMap<String, TfValue> = config
        .variables
        .iter()
        .filter_map(|(name, variable)| variable.default.clone().map(|value| (name.clone(), value)))
        .collect();

    if let Some(overrides) = tfvars {
        vars.extend(overrides);
    }

    let vars = vars
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

    let mut resources = config.resources;
    for resource in &mut resources {
        resolve_resource(resource, &vars, &locals);
    }

    Ok(ResolvedConfig {
        resources,
        vars,
        locals,
    })
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
