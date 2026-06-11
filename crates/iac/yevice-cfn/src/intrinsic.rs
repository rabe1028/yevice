use std::collections::HashMap;

use serde_yaml_ng::Value;

use crate::error::CfnError;
use crate::sentinel;

/// Maximum nesting depth for intrinsic function resolution.
///
/// Set high enough to accommodate legitimate deeply-nested CloudFormation
/// templates while still preventing stack overflows on adversarial input.
/// (The `yevice-tf` resolver applies its own, separate depth guard.)
const MAX_INTRINSIC_DEPTH: usize = 128;

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
    resolve_inner(value, ctx, 0)
}

/// Depth-aware implementation of [`resolve`].
///
/// `depth` is incremented at every recursive call site. When it exceeds
/// [`MAX_INTRINSIC_DEPTH`] an error is returned instead of recursing further,
/// preventing stack overflows on deeply-nested templates.
fn resolve_inner(value: &Value, ctx: &ResolveContext, depth: usize) -> Result<Value, CfnError> {
    if depth > MAX_INTRINSIC_DEPTH {
        return Err(CfnError::IntrinsicError(format!(
            "intrinsic nesting exceeds maximum depth ({MAX_INTRINSIC_DEPTH})"
        )));
    }

    match value {
        Value::Tagged(tagged) => resolve_tagged(tagged, ctx, depth),
        Value::Mapping(map) => {
            // Check for long-form intrinsic functions (e.g., {"Fn::Sub": "..."})
            if map.len() == 1
                && let Some((key, val)) = map.iter().next()
                && let Value::String(fn_name) = key
            {
                match fn_name.as_str() {
                    "Ref" => return resolve_ref(val, ctx),
                    "Fn::Sub" => return resolve_sub(val, ctx, depth),
                    "Fn::FindInMap" => return resolve_find_in_map(val, ctx, depth),
                    "Fn::Select" => return resolve_select(val, ctx, depth),
                    "Fn::If" => return resolve_if(val, ctx, depth),
                    "Fn::ImportValue" => return resolve_import_value(val, ctx, depth),
                    "Fn::Join" => return resolve_join(val, ctx, depth),
                    "Fn::GetAtt" => return resolve_get_att(val, ctx),
                    _ => {}
                }
            }
            // Recursively resolve all values in the mapping
            let mut new_map = serde_yaml_ng::Mapping::new();
            for (k, v) in map {
                new_map.insert(k.clone(), resolve_inner(v, ctx, depth + 1)?);
            }
            Ok(Value::Mapping(new_map))
        }
        Value::Sequence(seq) => {
            let resolved: Result<Vec<Value>, CfnError> = seq
                .iter()
                .map(|v| resolve_inner(v, ctx, depth + 1))
                .collect();
            Ok(Value::Sequence(resolved?))
        }
        _ => Ok(value.clone()),
    }
}

fn resolve_tagged(
    tagged: &serde_yaml_ng::value::TaggedValue,
    ctx: &ResolveContext,
    depth: usize,
) -> Result<Value, CfnError> {
    let tag = tagged.tag.to_string();
    match tag.as_str() {
        "!Ref" => resolve_ref(&tagged.value, ctx),
        "!Sub" => resolve_sub(&tagged.value, ctx, depth),
        "!FindInMap" => resolve_find_in_map(&tagged.value, ctx, depth),
        "!Select" => resolve_select(&tagged.value, ctx, depth),
        "!If" => resolve_if(&tagged.value, ctx, depth),
        "!ImportValue" => resolve_import_value(&tagged.value, ctx, depth),
        "!Join" => resolve_join(&tagged.value, ctx, depth),
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

fn resolve_sub(value: &Value, ctx: &ResolveContext, depth: usize) -> Result<Value, CfnError> {
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
                        if let Value::String(key) = k {
                            let resolved = resolve_inner(v, ctx, depth + 1)?;
                            // If the resolved value is a string, use it; otherwise
                            // insert an empty string so that ${Key} is replaced with
                            // "" rather than being left as a bare variable name that
                            // would fall through to the resource-ref sentinel path.
                            let s = resolved.as_str().map_or_else(String::new, str::to_string);
                            map.insert(key.clone(), s);
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
            // ${!Literal} is the documented Fn::Sub escape for a literal ${Literal}.
            if let Some(literal) = var_name.strip_prefix('!') {
                use std::fmt::Write;
                let _ = write!(result, "${{{literal}}}");
            } else if let Some(val) = local_vars.get(&var_name) {
                result.push_str(val);
            } else if let Some(val) = ctx.parameters.get(&var_name) {
                result.push_str(val);
            } else if var_name.starts_with("AWS::") {
                // Pseudo-parameter (e.g. AWS::Region) — leave verbatim
                use std::fmt::Write;
                let _ = write!(result, "${{{var_name}}}");
            } else if let Some((logical, attr)) = var_name.split_once('.') {
                // Resource attribute reference: ${Resource.Attr} → GetAtt sentinel
                result.push_str(&sentinel::make_getatt(logical, attr));
            } else {
                // Bare resource reference: ${Resource} → Ref sentinel
                result.push_str(&sentinel::make_ref(&var_name));
            }
        } else {
            result.push(c);
        }
    }
    Ok(result)
}

fn resolve_find_in_map(
    value: &Value,
    ctx: &ResolveContext,
    depth: usize,
) -> Result<Value, CfnError> {
    let seq = value
        .as_sequence()
        .ok_or_else(|| CfnError::IntrinsicError("!FindInMap argument must be a sequence".into()))?;

    if seq.len() != 3 {
        return Err(CfnError::IntrinsicError(
            "!FindInMap must have exactly 3 elements".into(),
        ));
    }

    let map_name = resolve_inner(&seq[0], ctx, depth + 1)?;
    let first_key = resolve_inner(&seq[1], ctx, depth + 1)?;
    let second_key = resolve_inner(&seq[2], ctx, depth + 1)?;

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

fn resolve_select(value: &Value, ctx: &ResolveContext, depth: usize) -> Result<Value, CfnError> {
    let seq = value
        .as_sequence()
        .ok_or_else(|| CfnError::IntrinsicError("!Select argument must be a sequence".into()))?;

    if seq.len() != 2 {
        return Err(CfnError::IntrinsicError(
            "!Select must have exactly 2 elements".into(),
        ));
    }

    let index = resolve_inner(&seq[0], ctx, depth + 1)?;
    let index: usize = match &index {
        Value::Number(n) => n.as_u64().ok_or_else(|| {
            CfnError::IntrinsicError(format!(
                "!Select index must be a non-negative integer, got {n}"
            ))
        })? as usize,
        Value::String(s) => s.parse::<usize>().map_err(|_| {
            CfnError::IntrinsicError(format!(
                "!Select index must be numeric, got \"{s}\" (is the index an unresolved parameter?)"
            ))
        })?,
        other => {
            return Err(CfnError::IntrinsicError(format!(
                "!Select index must be a number, got {other:?}"
            )));
        }
    };

    let list = resolve_inner(&seq[1], ctx, depth + 1)?;
    let list = list
        .as_sequence()
        .ok_or_else(|| CfnError::IntrinsicError("!Select second arg must be a list".into()))?;

    list.get(index)
        .cloned()
        .ok_or_else(|| CfnError::IntrinsicError(format!("!Select index {index} out of bounds")))
}

fn resolve_if(value: &Value, ctx: &ResolveContext, depth: usize) -> Result<Value, CfnError> {
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
        resolve_inner(&seq[1], ctx, depth + 1)
    } else {
        resolve_inner(&seq[2], ctx, depth + 1)
    }
}

fn resolve_import_value(
    value: &Value,
    ctx: &ResolveContext,
    depth: usize,
) -> Result<Value, CfnError> {
    let resolved = resolve_inner(value, ctx, depth + 1)?;
    let export_name = resolved.as_str().ok_or_else(|| {
        CfnError::IntrinsicError("Fn::ImportValue argument must resolve to a string".into())
    })?;

    ctx.imports
        .get(export_name)
        .map(|v| Value::String(v.clone()))
        .ok_or_else(|| CfnError::ImportValueNotFound(export_name.to_string()))
}

fn resolve_join(value: &Value, ctx: &ResolveContext, depth: usize) -> Result<Value, CfnError> {
    let seq = value
        .as_sequence()
        .ok_or_else(|| CfnError::IntrinsicError("!Join argument must be a sequence".into()))?;

    if seq.len() != 2 {
        return Err(CfnError::IntrinsicError(
            "!Join must have exactly 2 elements".into(),
        ));
    }

    let delimiter = resolve_inner(&seq[0], ctx, depth + 1)?;
    let delimiter = delimiter.as_str().unwrap_or("");

    let list = resolve_inner(&seq[1], ctx, depth + 1)?;
    let list = list
        .as_sequence()
        .ok_or_else(|| CfnError::IntrinsicError("!Join second arg must be a list".into()))?;

    let parts: Vec<String> = list
        .iter()
        .filter_map(|v| match v {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            Value::Bool(b) => Some(b.to_string()),
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
        let result = resolve_sub(&val, &ctx, 0).unwrap();
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
        let result = resolve_find_in_map(&val, &ctx, 0).unwrap();
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
        let result = resolve_if(&val, &ctx, 0).unwrap();
        assert_eq!(result, Value::String("prod-value".into()));
    }

    #[test]
    fn test_resolve_import_value() {
        let ctx = make_ctx();
        let val = Value::String("SharedVpcId".into());
        let result = resolve_import_value(&val, &ctx, 0).unwrap();
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

    // --- Sub sentinel-isation (#1) ---

    /// `!Sub '${Fn.Arn}'` (bare resource.attr) must produce a getatt sentinel,
    /// not the literal `${{HandlerFunction.Arn}}`.
    #[test]
    fn test_sub_resource_attr_becomes_getatt_sentinel() {
        let ctx = make_ctx();
        let val = Value::String("${HandlerFunction.Arn}".into());
        let result = resolve_sub(&val, &ctx, 0).unwrap();
        assert_eq!(
            result,
            Value::String("{{getatt:HandlerFunction.Arn}}".into()),
            "!Sub '${{Resource.Attr}}' must produce a getatt sentinel"
        );
    }

    /// `!Sub '${MyQueue}'` (bare resource ref, not in parameters) must produce
    /// a ref sentinel, not the literal `${{MyQueue}}`.
    #[test]
    fn test_sub_resource_ref_becomes_ref_sentinel() {
        let ctx = make_ctx();
        // "MyQueue" is not a parameter in make_ctx(), so it must sentinel-ise.
        let val = Value::String("${MyQueue}".into());
        let result = resolve_sub(&val, &ctx, 0).unwrap();
        assert_eq!(
            result,
            Value::String("{{ref:MyQueue}}".into()),
            "!Sub '${{Resource}}' must produce a ref sentinel"
        );
    }

    /// `!Sub '${AWS::Region}'` must remain verbatim — pseudo-parameters are
    /// never sentinel-ised.
    #[test]
    fn test_sub_pseudo_parameter_stays_verbatim() {
        let ctx = make_ctx();
        let val = Value::String("${AWS::Region}".into());
        let result = resolve_sub(&val, &ctx, 0).unwrap();
        assert_eq!(
            result,
            Value::String("${AWS::Region}".into()),
            "!Sub '${{AWS::Region}}' must remain verbatim (no sentinel)"
        );
    }

    /// An embedded `!Sub` such as `'arn:...:${MyQueue}/p'` puts the sentinel
    /// inside a longer string. The whole result is still a single string (not a
    /// standalone sentinel), which is expected — edge extraction requires a
    /// whole-string sentinel.
    #[test]
    fn test_sub_embedded_resource_ref_is_not_standalone_sentinel() {
        let ctx = make_ctx();
        let val = Value::String("arn:aws:sqs:us-east-1:123456789012:${MyQueue}/suffix".into());
        let result = resolve_sub(&val, &ctx, 0).unwrap();
        let s = result.as_str().unwrap();
        // The sentinel is embedded, not the whole string.
        assert!(
            s.contains("{{ref:MyQueue}}"),
            "embedded ref should contain the sentinel: {s}"
        );
        assert_ne!(
            s, "{{ref:MyQueue}}",
            "embedded ref must NOT be a standalone sentinel"
        );
    }

    // --- Fn::Sub ${!Literal} escape (#2) ---

    /// `${!NotAVar}` must resolve to the literal `${NotAVar}` (no sentinel).
    #[test]
    fn test_sub_bang_escape_produces_literal() {
        let ctx = make_ctx();
        let val = Value::String("foo-${!NotAVar}-bar".into());
        let result = resolve_sub(&val, &ctx, 0).unwrap();
        assert_eq!(
            result,
            Value::String("foo-${NotAVar}-bar".into()),
            "!Sub '${{!Literal}}' must produce literal '${{Literal}}', not a sentinel"
        );
    }

    /// The escape must NOT produce any sentinel marker.
    #[test]
    fn test_sub_bang_escape_does_not_sentinel() {
        let ctx = make_ctx();
        let val = Value::String("${!SomeVar}".into());
        let result = resolve_sub(&val, &ctx, 0).unwrap();
        let s = result.as_str().unwrap();
        assert!(
            !s.contains("{{ref:"),
            "escaped variable must not produce a ref sentinel: {s}"
        );
        assert!(
            !s.contains("{{getatt:"),
            "escaped variable must not produce a getatt sentinel: {s}"
        );
        assert_eq!(s, "${SomeVar}");
    }

    /// Normal `${Resource}` refs alongside `${!Escaped}` must both work correctly.
    #[test]
    fn test_sub_bang_escape_mixed_with_resource_ref() {
        let ctx = make_ctx();
        // MyQueue is not a parameter in make_ctx(), so it becomes a ref sentinel.
        let val = Value::String("${MyQueue}-${!NotAVar}".into());
        let result = resolve_sub(&val, &ctx, 0).unwrap();
        assert_eq!(
            result,
            Value::String("{{ref:MyQueue}}-${NotAVar}".into()),
            "resource ref must sentinel-ise while escaped var stays literal"
        );
    }

    // --- resolve_join Bool support (#5) ---

    /// `!Join` with a Bool element must include it in the result string.
    #[test]
    fn test_join_includes_bool_elements() {
        let ctx = make_ctx();
        // Join [":", [true, "suffix"]]  →  "true:suffix"
        let val = Value::Sequence(vec![
            Value::String(":".into()),
            Value::Sequence(vec![
                Value::Bool(true),
                Value::String("suffix".into()),
                Value::Bool(false),
            ]),
        ]);
        let result = resolve_join(&val, &ctx, 0).unwrap();
        assert_eq!(
            result,
            Value::String("true:suffix:false".into()),
            "Bool elements must be included in !Join result"
        );
    }

    /// `!Join` with only a Bool element must not produce an empty string.
    #[test]
    fn test_join_bool_only_element() {
        let ctx = make_ctx();
        let val = Value::Sequence(vec![
            Value::String(String::new()),
            Value::Sequence(vec![Value::Bool(false)]),
        ]);
        let result = resolve_join(&val, &ctx, 0).unwrap();
        assert_eq!(
            result,
            Value::String("false".into()),
            "single Bool element must not be silently dropped"
        );
    }

    // --- resolve_sub 2-arg non-string local var (#6) ---

    /// 2-arg `!Sub` where a local var resolves to a non-string (e.g. Number)
    /// must substitute the key with empty string, NOT produce a `{{ref:Key}}`
    /// sentinel.
    #[test]
    fn test_sub_two_arg_non_string_local_var_becomes_empty_not_sentinel() {
        let ctx = make_ctx();
        // Template: "${NumVar}-suffix"
        // vars mapping: NumVar -> !Ref InstanceType resolves to "t3.micro" (a string).
        // But we want to test a Number value: use a Number literal in the mapping.
        let val = Value::Sequence(vec![
            Value::String("prefix-${NumVar}-suffix".into()),
            Value::Mapping({
                let mut m = serde_yaml_ng::Mapping::new();
                m.insert(
                    Value::String("NumVar".into()),
                    Value::Number(serde_yaml_ng::value::Number::from(42_i64)),
                );
                m
            }),
        ]);
        let result = resolve_sub(&val, &ctx, 0).unwrap();
        let s = result.as_str().unwrap();
        // NumVar resolves to a Number, which is not a string — must be inserted
        // as empty string, producing "prefix--suffix", NOT "prefix-{{ref:NumVar}}-suffix".
        assert!(
            !s.contains("{{ref:NumVar}}"),
            "non-string local var must not produce a ref sentinel: {s}"
        );
        assert_eq!(
            s, "prefix--suffix",
            "non-string local var must become empty: {s}"
        );
    }

    /// 2-arg `!Sub` where a local var resolves to a string must still work normally.
    #[test]
    fn test_sub_two_arg_string_local_var_substituted_correctly() {
        let ctx = make_ctx();
        let val = Value::Sequence(vec![
            Value::String("hello-${Name}".into()),
            Value::Mapping({
                let mut m = serde_yaml_ng::Mapping::new();
                m.insert(Value::String("Name".into()), Value::String("world".into()));
                m
            }),
        ]);
        let result = resolve_sub(&val, &ctx, 0).unwrap();
        assert_eq!(result, Value::String("hello-world".into()));
    }

    // --- resolve_select error on bad index (Fix 3) ---

    /// `!Select` with a non-numeric string index must return an error
    /// containing the offending value, not silently use index 0.
    #[test]
    fn test_select_non_numeric_string_index_errors() {
        let ctx = make_ctx();
        // Index "abc" is not a valid usize.
        let val = Value::Sequence(vec![
            Value::String("abc".into()),
            Value::Sequence(vec![
                Value::String("first".into()),
                Value::String("second".into()),
            ]),
        ]);
        let result = resolve_select(&val, &ctx, 0);
        assert!(
            result.is_err(),
            "expected Err for non-numeric string index, got Ok"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("abc"),
            "error message must contain the offending value 'abc': {err_msg}"
        );
    }

    /// `!Select` with a numeric string index must still resolve correctly.
    #[test]
    fn test_select_numeric_string_index_succeeds() {
        let ctx = make_ctx();
        let val = Value::Sequence(vec![
            Value::String("1".into()),
            Value::Sequence(vec![
                Value::String("first".into()),
                Value::String("second".into()),
            ]),
        ]);
        let result = resolve_select(&val, &ctx, 0).unwrap();
        assert_eq!(result, Value::String("second".into()));
    }

    /// `!Select` with a valid Number index must still resolve correctly.
    #[test]
    fn test_select_number_index_succeeds() {
        let ctx = make_ctx();
        let val = Value::Sequence(vec![
            Value::Number(serde_yaml_ng::value::Number::from(0_u64)),
            Value::Sequence(vec![
                Value::String("first".into()),
                Value::String("second".into()),
            ]),
        ]);
        let result = resolve_select(&val, &ctx, 0).unwrap();
        assert_eq!(result, Value::String("first".into()));
    }

    // --- Depth guard tests ---

    /// Build a deeply nested `!Join` value that exceeds MAX_INTRINSIC_DEPTH.
    ///
    /// The structure is:
    ///   Join(["-", [Join(["-", [Join(["-", [...]])]])]])
    /// repeated MAX_INTRINSIC_DEPTH + 2 times so the recursion definitely
    /// exceeds the limit.
    fn build_deep_join(depth: usize) -> Value {
        if depth == 0 {
            return Value::String("leaf".into());
        }
        // Fn::Join long-form mapping so it goes through resolve_inner dispatch
        let mut inner_map = serde_yaml_ng::Mapping::new();
        inner_map.insert(
            Value::String("Fn::Join".into()),
            Value::Sequence(vec![
                Value::String("-".into()),
                Value::Sequence(vec![build_deep_join(depth - 1)]),
            ]),
        );
        Value::Mapping(inner_map)
    }

    /// Resolving a value nested beyond MAX_INTRINSIC_DEPTH must return
    /// `CfnError::IntrinsicError` (depth exceeded), not panic.
    #[test]
    fn test_depth_guard_exceeds_limit_returns_error() {
        let ctx = make_ctx();
        // Build MAX_INTRINSIC_DEPTH + 2 levels of nesting to guarantee we exceed 128.
        let deep = build_deep_join(MAX_INTRINSIC_DEPTH + 2);
        let result = resolve(&deep, &ctx);
        assert!(
            result.is_err(),
            "expected Err for depth > MAX_INTRINSIC_DEPTH, got Ok"
        );
        let err = result.unwrap_err();
        match err {
            CfnError::IntrinsicError(msg) => {
                assert!(
                    msg.contains("maximum depth"),
                    "error message should mention 'maximum depth', got: {msg}"
                );
            }
            other => panic!("expected CfnError::IntrinsicError, got: {other:?}"),
        }
    }

    /// A shallow nesting (well within MAX_INTRINSIC_DEPTH) must resolve normally.
    #[test]
    fn test_depth_guard_shallow_nesting_succeeds() {
        let ctx = make_ctx();
        // 3 levels of nesting: trivially within limit.
        let shallow = build_deep_join(3);
        let result = resolve(&shallow, &ctx);
        assert!(
            result.is_ok(),
            "expected Ok for shallow nesting, got: {result:?}"
        );
        // The leaf value "leaf" joined with "-" at each level is just "leaf"
        // (only one element per list), so the final string is "leaf".
        assert_eq!(result.unwrap(), Value::String("leaf".into()));
    }

    /// Exactly at MAX_INTRINSIC_DEPTH levels must still succeed (boundary case).
    ///
    /// `build_deep_join(N)` adds 2 to the depth counter per layer (one for the
    /// `Fn::Join` dispatch, one for the list element), so N=64 drives the
    /// deepest `resolve_inner` to exactly depth 128 — the largest value that
    /// still succeeds.
    #[test]
    fn test_depth_guard_at_limit_succeeds() {
        let ctx = make_ctx();
        let at_limit = build_deep_join(64);
        let result = resolve(&at_limit, &ctx);
        assert!(
            result.is_ok(),
            "expected Ok for nesting at MAX_INTRINSIC_DEPTH, got: {result:?}"
        );
    }

    /// One level past the limit (N=65 → depth 130 > 128) must error. Together
    /// with `test_depth_guard_at_limit_succeeds` this pins the exact 64-ok /
    /// 65-err boundary, catching an off-by-one in the `depth > MAX` guard.
    #[test]
    fn test_depth_guard_just_over_limit_errors() {
        let ctx = make_ctx();
        let just_over = build_deep_join(65);
        let result = resolve(&just_over, &ctx);
        assert!(
            result.is_err(),
            "expected Err one level past MAX_INTRINSIC_DEPTH, got: {result:?}"
        );
    }
}
