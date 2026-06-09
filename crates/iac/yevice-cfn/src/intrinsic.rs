use std::collections::HashMap;

use serde_yaml_ng::Value;

use crate::error::CfnError;
use crate::sentinel;

/// Context for resolving `CloudFormation` intrinsic functions.
pub struct ResolveContext {
    /// Template parameters: name -> value.
    pub parameters: HashMap<String, String>,
    /// Cross-stack import values: export-name -> value.
    pub imports: HashMap<String, String>,
    /// Mappings from the template.
    pub mappings: HashMap<String, HashMap<String, HashMap<String, String>>>,
    /// Conditions from the template (evaluated to bool).
    pub conditions: HashMap<String, bool>,
}

impl ResolveContext {
    pub fn new(parameters: HashMap<String, String>, imports: HashMap<String, String>) -> Self {
        Self {
            parameters,
            imports,
            mappings: HashMap::new(),
            conditions: HashMap::new(),
        }
    }
}

/// Resolve `CloudFormation` intrinsic functions in a YAML value.
///
/// Handles: !Ref, !Sub, !`FindInMap`, !Select, !If, `Fn::ImportValue`,
/// !Join, !`GetAtt` (partial).
///
/// Unresolvable values are returned as-is (for forward compatibility).
pub fn resolve(value: &Value, ctx: &ResolveContext) -> Result<Value, CfnError> {
    match value {
        Value::Tagged(tagged) => resolve_tagged(tagged, ctx),
        Value::Mapping(map) => {
            // Check for long-form intrinsic functions (e.g., {"Fn::Sub": "..."})
            if map.len() == 1
                && let Some((key, val)) = map.iter().next()
                && let Value::String(fn_name) = key
            {
                match fn_name.as_str() {
                    "Ref" => return resolve_ref(val, ctx),
                    "Fn::Sub" => return resolve_sub(val, ctx),
                    "Fn::FindInMap" => return resolve_find_in_map(val, ctx),
                    "Fn::Select" => return resolve_select(val, ctx),
                    "Fn::If" => return resolve_if(val, ctx),
                    "Fn::ImportValue" => return resolve_import_value(val, ctx),
                    "Fn::Join" => return resolve_join(val, ctx),
                    "Fn::GetAtt" => return resolve_get_att(val, ctx),
                    _ => {}
                }
            }
            // Recursively resolve all values in the mapping
            let mut new_map = serde_yaml_ng::Mapping::new();
            for (k, v) in map {
                new_map.insert(k.clone(), resolve(v, ctx)?);
            }
            Ok(Value::Mapping(new_map))
        }
        Value::Sequence(seq) => {
            let resolved: Result<Vec<Value>, CfnError> =
                seq.iter().map(|v| resolve(v, ctx)).collect();
            Ok(Value::Sequence(resolved?))
        }
        _ => Ok(value.clone()),
    }
}

fn resolve_tagged(
    tagged: &serde_yaml_ng::value::TaggedValue,
    ctx: &ResolveContext,
) -> Result<Value, CfnError> {
    let tag = tagged.tag.to_string();
    match tag.as_str() {
        "!Ref" => resolve_ref(&tagged.value, ctx),
        "!Sub" => resolve_sub(&tagged.value, ctx),
        "!FindInMap" => resolve_find_in_map(&tagged.value, ctx),
        "!Select" => resolve_select(&tagged.value, ctx),
        "!If" => resolve_if(&tagged.value, ctx),
        "!ImportValue" => resolve_import_value(&tagged.value, ctx),
        "!Join" => resolve_join(&tagged.value, ctx),
        "!GetAtt" => resolve_get_att(&tagged.value, ctx),
        // Return unresolvable tagged values as-is
        _ => Ok(Value::Tagged(Box::new(tagged.clone()))),
    }
}

fn resolve_ref(value: &Value, ctx: &ResolveContext) -> Result<Value, CfnError> {
    let name = value
        .as_str()
        .ok_or_else(|| CfnError::IntrinsicError("!Ref argument must be a string".into()))?;

    // Pseudo parameters
    match name {
        "AWS::Region" => {
            if let Some(region) = ctx.parameters.get("AWS::Region") {
                return Ok(Value::String(region.clone()));
            }
            return Ok(Value::String("ap-northeast-1".into()));
        }
        "AWS::AccountId" => return Ok(Value::String("123456789012".into())),
        "AWS::StackName" => return Ok(Value::String("stack".into())),
        "AWS::NoValue" => return Ok(Value::Null),
        _ => {}
    }

    // Look up in parameters
    if let Some(val) = ctx.parameters.get(name) {
        return Ok(Value::String(val.clone()));
    }

    // If it's a resource logical ID, return as-is (we can't resolve resource IDs statically)
    Ok(Value::String(sentinel::make_ref(name)))
}

fn resolve_sub(value: &Value, ctx: &ResolveContext) -> Result<Value, CfnError> {
    match value {
        Value::String(template) => {
            let result = substitute_variables(template, &HashMap::new(), ctx)?;
            Ok(Value::String(result))
        }
        Value::Sequence(seq) => {
            if seq.len() != 2 {
                return Err(CfnError::IntrinsicError(
                    "!Sub with array must have exactly 2 elements".into(),
                ));
            }
            let template = seq[0]
                .as_str()
                .ok_or_else(|| CfnError::IntrinsicError("!Sub template must be a string".into()))?;
            let vars = match &seq[1] {
                Value::Mapping(m) => {
                    let mut map = HashMap::new();
                    for (k, v) in m {
                        if let (Value::String(key), resolved) = (k, resolve(v, ctx)?)
                            && let Some(s) = resolved.as_str()
                        {
                            map.insert(key.clone(), s.to_string());
                        }
                    }
                    map
                }
                _ => HashMap::new(),
            };
            let result = substitute_variables(template, &vars, ctx)?;
            Ok(Value::String(result))
        }
        _ => Err(CfnError::IntrinsicError(
            "!Sub value must be a string or array".into(),
        )),
    }
}

fn substitute_variables(
    template: &str,
    local_vars: &HashMap<String, String>,
    ctx: &ResolveContext,
) -> Result<String, CfnError> {
    let mut result = String::new();
    let mut chars = template.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '$' && chars.peek() == Some(&'{') {
            chars.next(); // consume '{'
            let mut var_name = String::new();
            for ch in chars.by_ref() {
                if ch == '}' {
                    break;
                }
                var_name.push(ch);
            }
            // Try local vars first, then parameters
            if let Some(val) = local_vars.get(&var_name) {
                result.push_str(val);
            } else if let Some(val) = ctx.parameters.get(&var_name) {
                result.push_str(val);
            } else {
                use std::fmt::Write;
                let _ = write!(result, "${{{var_name}}}");
            }
        } else {
            result.push(c);
        }
    }
    Ok(result)
}

fn resolve_find_in_map(value: &Value, ctx: &ResolveContext) -> Result<Value, CfnError> {
    let seq = value
        .as_sequence()
        .ok_or_else(|| CfnError::IntrinsicError("!FindInMap argument must be a sequence".into()))?;

    if seq.len() != 3 {
        return Err(CfnError::IntrinsicError(
            "!FindInMap must have exactly 3 elements".into(),
        ));
    }

    let map_name = resolve(&seq[0], ctx)?;
    let first_key = resolve(&seq[1], ctx)?;
    let second_key = resolve(&seq[2], ctx)?;

    let map_name = map_name.as_str().unwrap_or_default();
    let first_key = first_key.as_str().unwrap_or_default();
    let second_key = second_key.as_str().unwrap_or_default();

    ctx.mappings
        .get(map_name)
        .and_then(|m| m.get(first_key))
        .and_then(|m| m.get(second_key))
        .map(|v| Value::String(v.clone()))
        .ok_or_else(|| CfnError::MappingNotFound {
            map_name: map_name.to_string(),
            first_key: first_key.to_string(),
            second_key: second_key.to_string(),
        })
}

fn resolve_select(value: &Value, ctx: &ResolveContext) -> Result<Value, CfnError> {
    let seq = value
        .as_sequence()
        .ok_or_else(|| CfnError::IntrinsicError("!Select argument must be a sequence".into()))?;

    if seq.len() != 2 {
        return Err(CfnError::IntrinsicError(
            "!Select must have exactly 2 elements".into(),
        ));
    }

    let index = resolve(&seq[0], ctx)?;
    let index = match &index {
        Value::Number(n) => n.as_u64().unwrap_or(0) as usize,
        Value::String(s) => s.parse::<usize>().unwrap_or(0),
        _ => 0,
    };

    let list = resolve(&seq[1], ctx)?;
    let list = list
        .as_sequence()
        .ok_or_else(|| CfnError::IntrinsicError("!Select second arg must be a list".into()))?;

    list.get(index)
        .cloned()
        .ok_or_else(|| CfnError::IntrinsicError(format!("!Select index {index} out of bounds")))
}

fn resolve_if(value: &Value, ctx: &ResolveContext) -> Result<Value, CfnError> {
    let seq = value
        .as_sequence()
        .ok_or_else(|| CfnError::IntrinsicError("!If argument must be a sequence".into()))?;

    if seq.len() != 3 {
        return Err(CfnError::IntrinsicError(
            "!If must have exactly 3 elements".into(),
        ));
    }

    let cond_name = seq[0]
        .as_str()
        .ok_or_else(|| CfnError::IntrinsicError("!If condition must be a string".into()))?;

    let cond_value = ctx
        .conditions
        .get(cond_name)
        .ok_or_else(|| CfnError::ConditionNotFound(cond_name.to_string()))?;

    if *cond_value {
        resolve(&seq[1], ctx)
    } else {
        resolve(&seq[2], ctx)
    }
}

fn resolve_import_value(value: &Value, ctx: &ResolveContext) -> Result<Value, CfnError> {
    let resolved = resolve(value, ctx)?;
    let export_name = resolved.as_str().ok_or_else(|| {
        CfnError::IntrinsicError("Fn::ImportValue argument must resolve to a string".into())
    })?;

    ctx.imports
        .get(export_name)
        .map(|v| Value::String(v.clone()))
        .ok_or_else(|| CfnError::ImportValueNotFound(export_name.to_string()))
}

fn resolve_join(value: &Value, ctx: &ResolveContext) -> Result<Value, CfnError> {
    let seq = value
        .as_sequence()
        .ok_or_else(|| CfnError::IntrinsicError("!Join argument must be a sequence".into()))?;

    if seq.len() != 2 {
        return Err(CfnError::IntrinsicError(
            "!Join must have exactly 2 elements".into(),
        ));
    }

    let delimiter = resolve(&seq[0], ctx)?;
    let delimiter = delimiter.as_str().unwrap_or("");

    let list = resolve(&seq[1], ctx)?;
    let list = list
        .as_sequence()
        .ok_or_else(|| CfnError::IntrinsicError("!Join second arg must be a list".into()))?;

    let parts: Vec<String> = list
        .iter()
        .filter_map(|v| match v {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
        .collect();

    Ok(Value::String(parts.join(delimiter)))
}

fn resolve_get_att(value: &Value, ctx: &ResolveContext) -> Result<Value, CfnError> {
    // !GetAtt cannot be fully resolved statically.
    // Return a sentinel placeholder when we have enough information (logical_id + attr),
    // otherwise pass the value through unmodified so downstream consumers can
    // recognise it as unresolved rather than silently dropping it.
    let _ = ctx;
    match value {
        Value::String(s) => {
            // Dot-notation form: "LogicalId.Attr"
            if let Some((logical_id, attr)) = s.split_once('.') {
                return Ok(Value::String(sentinel::make_getatt(logical_id, attr)));
            }
            // No dot: cannot construct a valid sentinel (no attr present).
            // Return the value as-is so downstream sees an unresolved reference.
            tracing::warn!(value = %s, "!GetAtt string has no dot separator — treating as unresolved");
            Ok(value.clone())
        }
        Value::Sequence(seq) => {
            let parts: Vec<&str> = seq.iter().filter_map(|v| v.as_str()).collect();
            match parts.as_slice() {
                [logical_id, attr] => Ok(Value::String(sentinel::make_getatt(logical_id, attr))),
                [logical_id, rest @ ..] if !rest.is_empty() => {
                    // More than 2 elements: join remaining parts with "." as the attr.
                    let attr = rest.join(".");
                    Ok(Value::String(sentinel::make_getatt(logical_id, &attr)))
                }
                _ => {
                    // Empty or single-element sequence: unresolvable, pass through.
                    tracing::warn!(parts = ?parts, "!GetAtt sequence has fewer than 2 elements — treating as unresolved");
                    Ok(value.clone())
                }
            }
        }
        _ => Ok(value.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ctx() -> ResolveContext {
        let mut params = HashMap::new();
        params.insert("Env".to_string(), "prod".to_string());
        params.insert("InstanceType".to_string(), "t3.micro".to_string());

        let mut imports = HashMap::new();
        imports.insert("SharedVpcId".to_string(), "vpc-12345".to_string());

        let mut ctx = ResolveContext::new(params, imports);

        let mut region_map = HashMap::new();
        let mut tokyo = HashMap::new();
        tokyo.insert("AMI".to_string(), "ami-12345".to_string());
        region_map.insert("ap-northeast-1".to_string(), tokyo);
        ctx.mappings.insert("RegionMap".to_string(), region_map);

        ctx.conditions.insert("IsProd".to_string(), true);

        ctx
    }

    #[test]
    fn test_resolve_ref_parameter() {
        let ctx = make_ctx();
        let val = Value::String("Env".into());
        let result = resolve_ref(&val, &ctx).unwrap();
        assert_eq!(result, Value::String("prod".into()));
    }

    #[test]
    fn test_resolve_sub() {
        let ctx = make_ctx();
        let val = Value::String("arn:aws:s3:::${Env}-bucket".into());
        let result = resolve_sub(&val, &ctx).unwrap();
        assert_eq!(result, Value::String("arn:aws:s3:::prod-bucket".into()));
    }

    #[test]
    fn test_resolve_find_in_map() {
        let ctx = make_ctx();
        let val = Value::Sequence(vec![
            Value::String("RegionMap".into()),
            Value::String("ap-northeast-1".into()),
            Value::String("AMI".into()),
        ]);
        let result = resolve_find_in_map(&val, &ctx).unwrap();
        assert_eq!(result, Value::String("ami-12345".into()));
    }

    #[test]
    fn test_resolve_if_true() {
        let ctx = make_ctx();
        let val = Value::Sequence(vec![
            Value::String("IsProd".into()),
            Value::String("prod-value".into()),
            Value::String("dev-value".into()),
        ]);
        let result = resolve_if(&val, &ctx).unwrap();
        assert_eq!(result, Value::String("prod-value".into()));
    }

    #[test]
    fn test_resolve_import_value() {
        let ctx = make_ctx();
        let val = Value::String("SharedVpcId".into());
        let result = resolve_import_value(&val, &ctx).unwrap();
        assert_eq!(result, Value::String("vpc-12345".into()));
    }

    // --- GetAtt edge cases (#5) ---

    /// A dot-notation GetAtt must produce the correct sentinel.
    #[test]
    fn test_getatt_dot_notation_produces_sentinel() {
        let ctx = make_ctx();
        let val = Value::String("MyBucket.Arn".into());
        let result = resolve_get_att(&val, &ctx).unwrap();
        assert_eq!(
            result,
            Value::String("{{getatt:MyBucket.Arn}}".into()),
            "dot-notation GetAtt must produce sentinel"
        );
    }

    /// A string with no dot must NOT produce a sentinel — it must be passed
    /// through as-is (unresolved).
    #[test]
    fn test_getatt_no_dot_passes_through() {
        let ctx = make_ctx();
        let val = Value::String("MyBucketNoAttr".into());
        let result = resolve_get_att(&val, &ctx).unwrap();
        // Must return the original value unchanged (no sentinel generated).
        assert_eq!(
            result,
            Value::String("MyBucketNoAttr".into()),
            "GetAtt without dot must be a pass-through, not a sentinel"
        );
        // And specifically must NOT contain the getatt sentinel format.
        assert!(
            !result.as_str().unwrap_or("").contains("{{getatt:"),
            "no-dot GetAtt must not produce a getatt sentinel: {result:?}"
        );
    }

    /// A sequence with exactly 2 elements must produce the correct sentinel.
    #[test]
    fn test_getatt_sequence_two_elements_produces_sentinel() {
        let ctx = make_ctx();
        let val = Value::Sequence(vec![
            Value::String("MyQueue".into()),
            Value::String("Arn".into()),
        ]);
        let result = resolve_get_att(&val, &ctx).unwrap();
        assert_eq!(result, Value::String("{{getatt:MyQueue.Arn}}".into()));
    }

    /// A sequence with 3+ elements must produce a sentinel via `make_getatt`,
    /// joining the extra parts with ".".
    #[test]
    fn test_getatt_sequence_three_elements_uses_make_getatt() {
        let ctx = make_ctx();
        let val = Value::Sequence(vec![
            Value::String("MyResource".into()),
            Value::String("SomeNested".into()),
            Value::String("Attr".into()),
        ]);
        let result = resolve_get_att(&val, &ctx).unwrap();
        // make_getatt("MyResource", "SomeNested.Attr") = "{{getatt:MyResource.SomeNested.Attr}}"
        assert_eq!(
            result,
            Value::String("{{getatt:MyResource.SomeNested.Attr}}".into()),
            "3-element sequence must join tail with '.' via make_getatt"
        );
    }
}
