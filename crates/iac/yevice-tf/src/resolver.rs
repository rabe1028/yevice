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

            let Some(resolved) =
                resolve_value(value, &vars, &locals, 0).filter(TfValue::is_concrete)
            else {
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
        if let Some(resolved) = resolve_value(value, vars, locals, 0).filter(TfValue::is_concrete) {
            *value = resolved;
        }
    }

    for blocks in resource.blocks.values_mut() {
        for attrs in blocks {
            for value in attrs.values_mut() {
                if let Some(resolved) =
                    resolve_value(value, vars, locals, 0).filter(TfValue::is_concrete)
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
        TfValue::Unknown => None,
    }
}
