use std::collections::{BTreeMap, HashMap};

use serde_yaml_ng::Value;

use crate::error::CfnError;
use crate::resolved::{ResolvedValue, StringPart};

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
/// References to resource logical IDs (which cannot be resolved statically)
/// are preserved as typed [`ResolvedValue`] variants. Unresolvable values are
/// returned as `Concrete` pass-throughs (for forward compatibility).
pub fn resolve(value: &Value, ctx: &ResolveContext) -> Result<ResolvedValue, CfnError> {
    resolve_inner(value, ctx, 0)
}

/// Normalize an intrinsic function name to its canonical long-form, or return
/// `None` if the name is not a known intrinsic.
///
/// Both tag-form (`!Sub`, `!Ref`, …) and long-form (`Fn::Sub`, `Ref`, …) are
/// accepted and mapped to the canonical long-form name. The returned string is
/// always one of the keys handled in [`dispatch_intrinsic`].
///
/// This helper is the single source of truth for "is this name a known
/// intrinsic?" and is extracted so it can be unit-tested independently.
fn normalize_intrinsic_name(name: &str) -> Option<&'static str> {
    match name {
        "!Ref" | "Ref" => Some("Ref"),
        "!Sub" | "Fn::Sub" => Some("Fn::Sub"),
        "!FindInMap" | "Fn::FindInMap" => Some("Fn::FindInMap"),
        "!Select" | "Fn::Select" => Some("Fn::Select"),
        "!If" | "Fn::If" => Some("Fn::If"),
        "!ImportValue" | "Fn::ImportValue" => Some("Fn::ImportValue"),
        "!Join" | "Fn::Join" => Some("Fn::Join"),
        "!GetAtt" | "Fn::GetAtt" => Some("Fn::GetAtt"),
        "!Split" | "Fn::Split" => Some("Fn::Split"),
        _ => None,
    }
}

/// Dispatch a resolved canonical intrinsic name to its handler.
///
/// `canonical` must be a value returned by [`normalize_intrinsic_name`].
/// `value` is the argument node of the intrinsic.
fn dispatch_intrinsic(
    canonical: &str,
    value: &Value,
    ctx: &ResolveContext,
    depth: usize,
) -> Result<ResolvedValue, CfnError> {
    match canonical {
        "Ref" => resolve_ref(value, ctx),
        "Fn::Sub" => resolve_sub(value, ctx, depth),
        "Fn::FindInMap" => resolve_find_in_map(value, ctx, depth),
        "Fn::Select" => resolve_select(value, ctx, depth),
        "Fn::If" => resolve_if(value, ctx, depth),
        "Fn::ImportValue" => resolve_import_value(value, ctx, depth),
        "Fn::Join" => resolve_join(value, ctx, depth),
        "Fn::GetAtt" => resolve_get_att(value),
        "Fn::Split" => resolve_split(value, ctx, depth),
        // Safety: callers must only pass canonical names from normalize_intrinsic_name.
        other => unreachable!("unknown canonical intrinsic: {other}"),
    }
}

/// Depth-aware implementation of [`resolve`].
///
/// `depth` is incremented at every recursive call site. When it exceeds
/// [`MAX_INTRINSIC_DEPTH`] an error is returned instead of recursing further,
/// preventing stack overflows on deeply-nested templates.
fn resolve_inner(
    value: &Value,
    ctx: &ResolveContext,
    depth: usize,
) -> Result<ResolvedValue, CfnError> {
    if depth > MAX_INTRINSIC_DEPTH {
        return Err(CfnError::IntrinsicError(format!(
            "intrinsic nesting exceeds maximum depth ({MAX_INTRINSIC_DEPTH})"
        )));
    }

    match value {
        Value::Tagged(tagged) => {
            let tag = tagged.tag.to_string();
            if let Some(canonical) = normalize_intrinsic_name(tag.as_str()) {
                dispatch_intrinsic(canonical, &tagged.value, ctx, depth)
            } else {
                // Unknown tag — warn and pass through.
                tracing::warn!(
                    tag = %tag,
                    "unknown intrinsic tag encountered; passing through as-is"
                );
                Ok(ResolvedValue::Concrete(Value::Tagged(tagged.clone())))
            }
        }
        Value::Mapping(map) => {
            // Check for long-form intrinsic functions (e.g., {"Fn::Sub": "..."})
            if map.len() == 1
                && let Some((key, val)) = map.iter().next()
                && let Value::String(fn_name) = key
            {
                if let Some(canonical) = normalize_intrinsic_name(fn_name.as_str()) {
                    return dispatch_intrinsic(canonical, val, ctx, depth);
                } else if fn_name.starts_with("Fn::") {
                    // Single-key map whose key looks like an intrinsic but is
                    // not one we handle — warn and fall through to the generic
                    // map resolver below.
                    tracing::warn!(
                        fn_name = %fn_name,
                        "unknown intrinsic function encountered; passing through as-is"
                    );
                }
            }
            // Recursively resolve all values in the mapping
            let mut new_map = BTreeMap::new();
            for (k, v) in map {
                let Some(key) = k.as_str() else {
                    tracing::warn!(key = ?k, "non-string mapping key in properties; skipping");
                    continue;
                };
                new_map.insert(key.to_string(), resolve_inner(v, ctx, depth + 1)?);
            }
            Ok(ResolvedValue::Map(new_map))
        }
        Value::Sequence(seq) => {
            let resolved: Result<Vec<ResolvedValue>, CfnError> = seq
                .iter()
                .map(|v| resolve_inner(v, ctx, depth + 1))
                .collect();
            Ok(ResolvedValue::Seq(resolved?))
        }
        _ => Ok(ResolvedValue::Concrete(value.clone())),
    }
}

fn resolve_ref(value: &Value, ctx: &ResolveContext) -> Result<ResolvedValue, CfnError> {
    let name = value
        .as_str()
        .ok_or_else(|| CfnError::IntrinsicError("!Ref argument must be a string".into()))?;

    // Pseudo parameters
    match name {
        "AWS::Region" => {
            if let Some(region) = ctx.parameters.get("AWS::Region") {
                return Ok(ResolvedValue::Concrete(Value::String(region.clone())));
            }
            return Ok(ResolvedValue::Concrete(Value::String(
                "ap-northeast-1".into(),
            )));
        }
        "AWS::AccountId" => {
            return Ok(ResolvedValue::Concrete(Value::String(
                "123456789012".into(),
            )));
        }
        "AWS::StackName" => return Ok(ResolvedValue::Concrete(Value::String("stack".into()))),
        "AWS::NoValue" => return Ok(ResolvedValue::Concrete(Value::Null)),
        _ => {}
    }

    // Look up in parameters
    if let Some(val) = ctx.parameters.get(name) {
        return Ok(ResolvedValue::Concrete(Value::String(val.clone())));
    }

    // It's a resource logical ID — keep it as a typed reference (we can't
    // resolve resource IDs statically).
    Ok(ResolvedValue::Ref(name.to_string()))
}

fn resolve_sub(
    value: &Value,
    ctx: &ResolveContext,
    depth: usize,
) -> Result<ResolvedValue, CfnError> {
    match value {
        Value::String(template) => {
            let parts = substitute_variables(template, &HashMap::new(), ctx)?;
            Ok(ResolvedValue::from_parts(parts))
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
                            map.insert(key.clone(), resolved);
                        }
                    }
                    map
                }
                _ => HashMap::new(),
            };
            let parts = substitute_variables(template, &vars, ctx)?;
            Ok(ResolvedValue::from_parts(parts))
        }
        _ => Err(CfnError::IntrinsicError(
            "!Sub value must be a string or array".into(),
        )),
    }
}

/// Split a `Fn::Sub` template into interpolation parts, substituting local
/// variables and template parameters and keeping resource references typed.
fn substitute_variables(
    template: &str,
    local_vars: &HashMap<String, ResolvedValue>,
    ctx: &ResolveContext,
) -> Result<Vec<StringPart>, CfnError> {
    let mut parts: Vec<StringPart> = Vec::new();
    let mut literal = String::new();
    let mut chars = template.chars().peekable();

    let flush = |literal: &mut String, parts: &mut Vec<StringPart>| {
        if !literal.is_empty() {
            parts.push(StringPart::Literal(std::mem::take(literal)));
        }
    };

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
            if let Some(escaped) = var_name.strip_prefix('!') {
                use std::fmt::Write;
                let _ = write!(literal, "${{{escaped}}}");
            } else if let Some(val) = local_vars.get(&var_name) {
                // Local variables may themselves carry resource references.
                match val {
                    ResolvedValue::Concrete(Value::String(s)) => literal.push_str(s),
                    ResolvedValue::Ref(id) => {
                        flush(&mut literal, &mut parts);
                        parts.push(StringPart::Ref(id.clone()));
                    }
                    ResolvedValue::GetAtt { logical_id, attr } => {
                        flush(&mut literal, &mut parts);
                        parts.push(StringPart::GetAtt {
                            logical_id: logical_id.clone(),
                            attr: attr.clone(),
                        });
                    }
                    ResolvedValue::Interpolated(sub_parts) => {
                        flush(&mut literal, &mut parts);
                        parts.extend(sub_parts.iter().cloned());
                    }
                    // Non-string values substitute as empty string so that
                    // ${Key} is replaced with "" rather than falling through
                    // to the resource-ref path below.
                    _ => {}
                }
            } else if let Some(val) = ctx.parameters.get(&var_name) {
                literal.push_str(val);
            } else if var_name.starts_with("AWS::") {
                // Pseudo-parameter (e.g. AWS::Region) — leave verbatim
                use std::fmt::Write;
                let _ = write!(literal, "${{{var_name}}}");
            } else if let Some((logical, attr)) = var_name.split_once('.') {
                // Resource attribute reference: ${Resource.Attr}
                flush(&mut literal, &mut parts);
                parts.push(StringPart::GetAtt {
                    logical_id: logical.to_string(),
                    attr: attr.to_string(),
                });
            } else {
                // Bare resource reference: ${Resource}
                flush(&mut literal, &mut parts);
                parts.push(StringPart::Ref(var_name));
            }
        } else {
            literal.push(c);
        }
    }
    flush(&mut literal, &mut parts);
    Ok(parts)
}

fn resolve_find_in_map(
    value: &Value,
    ctx: &ResolveContext,
    depth: usize,
) -> Result<ResolvedValue, CfnError> {
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
        .map(|v| ResolvedValue::Concrete(Value::String(v.clone())))
        .ok_or_else(|| CfnError::MappingNotFound {
            map_name: map_name.to_string(),
            first_key: first_key.to_string(),
            second_key: second_key.to_string(),
        })
}

fn resolve_select(
    value: &Value,
    ctx: &ResolveContext,
    depth: usize,
) -> Result<ResolvedValue, CfnError> {
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
        ResolvedValue::Concrete(Value::Number(n)) => n.as_u64().ok_or_else(|| {
            CfnError::IntrinsicError(format!(
                "!Select index must be a non-negative integer, got {n}"
            ))
        })? as usize,
        ResolvedValue::Concrete(Value::String(s)) => s.parse::<usize>().map_err(|_| {
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

    let list_resolved = resolve_inner(&seq[1], ctx, depth + 1)?;

    // If the list resolved to a tagged pass-through (e.g. an unresolvable
    // Fn::Split whose source contains references), we cannot select from it
    // statically.  Warn and preserve the *original* !Select expression so
    // downstream consumers see the full unresolved expression (including the
    // index), not just the inner list argument.
    if let ResolvedValue::Concrete(Value::Tagged(ref tagged)) = list_resolved {
        tracing::warn!(
            index = index,
            tag = %tagged.tag,
            "!Select list resolved to an unresolvable tagged value; passing through as-is"
        );
        return Ok(ResolvedValue::Concrete(Value::Tagged(Box::new(
            serde_yaml_ng::value::TaggedValue {
                tag: serde_yaml_ng::value::Tag::new("!Select"),
                value: value.clone(),
            },
        ))));
    }

    let list = list_resolved
        .into_seq()
        .ok_or_else(|| CfnError::IntrinsicError("!Select second arg must be a list".into()))?;

    list.into_iter()
        .nth(index)
        .ok_or_else(|| CfnError::IntrinsicError(format!("!Select index {index} out of bounds")))
}

fn resolve_if(
    value: &Value,
    ctx: &ResolveContext,
    depth: usize,
) -> Result<ResolvedValue, CfnError> {
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
) -> Result<ResolvedValue, CfnError> {
    let resolved = resolve_inner(value, ctx, depth + 1)?;
    let export_name = resolved.as_str().ok_or_else(|| {
        CfnError::IntrinsicError("Fn::ImportValue argument must resolve to a string".into())
    })?;

    ctx.imports
        .get(export_name)
        .map(|v| ResolvedValue::Concrete(Value::String(v.clone())))
        .ok_or_else(|| CfnError::ImportValueNotFound(export_name.to_string()))
}

fn resolve_join(
    value: &Value,
    ctx: &ResolveContext,
    depth: usize,
) -> Result<ResolvedValue, CfnError> {
    let seq = value
        .as_sequence()
        .ok_or_else(|| CfnError::IntrinsicError("!Join argument must be a sequence".into()))?;

    if seq.len() != 2 {
        return Err(CfnError::IntrinsicError(
            "!Join must have exactly 2 elements".into(),
        ));
    }

    let delimiter = resolve_inner(&seq[0], ctx, depth + 1)?;
    let delimiter = delimiter.as_str().unwrap_or("").to_string();

    let list = resolve_inner(&seq[1], ctx, depth + 1)?;
    let list = list
        .into_seq()
        .ok_or_else(|| CfnError::IntrinsicError("!Join second arg must be a list".into()))?;

    let mut parts: Vec<StringPart> = Vec::new();
    let mut first = true;
    for item in list {
        // Scalars become literals; references stay typed parts. Anything else
        // (containers, null, …) is skipped, mirroring the previous behavior.
        let item_parts: Option<Vec<StringPart>> = match item {
            ResolvedValue::Concrete(Value::String(s)) => Some(vec![StringPart::Literal(s)]),
            ResolvedValue::Concrete(Value::Number(n)) => {
                Some(vec![StringPart::Literal(n.to_string())])
            }
            ResolvedValue::Concrete(Value::Bool(b)) => {
                Some(vec![StringPart::Literal(b.to_string())])
            }
            ResolvedValue::Ref(id) => Some(vec![StringPart::Ref(id)]),
            ResolvedValue::GetAtt { logical_id, attr } => {
                Some(vec![StringPart::GetAtt { logical_id, attr }])
            }
            ResolvedValue::Interpolated(ps) => Some(ps),
            _ => None,
        };
        if let Some(ps) = item_parts {
            if !first {
                parts.push(StringPart::Literal(delimiter.clone()));
            }
            parts.extend(ps);
            first = false;
        }
    }

    Ok(ResolvedValue::from_parts(parts))
}

fn resolve_get_att(value: &Value) -> Result<ResolvedValue, CfnError> {
    // !GetAtt cannot be fully resolved statically.
    // Return a typed reference when we have enough information
    // (logical_id + attr), otherwise pass the value through unmodified so
    // downstream consumers can recognise it as unresolved rather than
    // silently dropping it.
    match value {
        Value::String(s) => {
            // Dot-notation form: "LogicalId.Attr"
            if let Some((logical_id, attr)) = s.split_once('.') {
                return Ok(ResolvedValue::GetAtt {
                    logical_id: logical_id.to_string(),
                    attr: attr.to_string(),
                });
            }
            // No dot: cannot construct a reference (no attr present).
            // Return the value as-is so downstream sees an unresolved value.
            tracing::warn!(value = %s, "!GetAtt string has no dot separator — treating as unresolved");
            Ok(ResolvedValue::Concrete(value.clone()))
        }
        Value::Sequence(seq) => {
            let parts: Vec<&str> = seq.iter().filter_map(|v| v.as_str()).collect();
            match parts.as_slice() {
                [logical_id, attr] => Ok(ResolvedValue::GetAtt {
                    logical_id: (*logical_id).to_string(),
                    attr: (*attr).to_string(),
                }),
                [logical_id, rest @ ..] if !rest.is_empty() => {
                    // More than 2 elements: join remaining parts with "." as the attr.
                    Ok(ResolvedValue::GetAtt {
                        logical_id: (*logical_id).to_string(),
                        attr: rest.join("."),
                    })
                }
                _ => {
                    // Empty or single-element sequence: unresolvable, pass through.
                    tracing::warn!(parts = ?parts, "!GetAtt sequence has fewer than 2 elements — treating as unresolved");
                    Ok(ResolvedValue::Concrete(value.clone()))
                }
            }
        }
        _ => Ok(ResolvedValue::Concrete(value.clone())),
    }
}

/// Resolve `Fn::Split` / `!Split`.
///
/// Argument format: `[separator, source_string]`.
///
/// Resolution rules:
/// - If `source` resolves to a fully-concrete `String` (i.e. contains no
///   resource references), the split is performed immediately and the result
///   is returned as `ResolvedValue::Seq`.
/// - If `source` is `Interpolated` (contains unresolved resource references),
///   the split positions are indeterminate at static-analysis time, so we
///   emit a warning and pass the value through unchanged as `Concrete`.
fn resolve_split(
    value: &Value,
    ctx: &ResolveContext,
    depth: usize,
) -> Result<ResolvedValue, CfnError> {
    let seq = value
        .as_sequence()
        .ok_or_else(|| CfnError::IntrinsicError("Fn::Split argument must be a sequence".into()))?;

    if seq.len() != 2 {
        return Err(CfnError::IntrinsicError(
            "Fn::Split must have exactly 2 elements".into(),
        ));
    }

    let separator = resolve_inner(&seq[0], ctx, depth + 1)?;
    let separator = separator.as_str().ok_or_else(|| {
        CfnError::IntrinsicError(
            "Fn::Split separator (first element) must resolve to a string".into(),
        )
    })?;
    let separator = separator.to_string();

    let source = resolve_inner(&seq[1], ctx, depth + 1)?;

    match &source {
        ResolvedValue::Concrete(Value::String(s)) => {
            // Source is fully known — split immediately.
            let parts: Vec<ResolvedValue> = s
                .split(separator.as_str())
                .map(|part| ResolvedValue::Concrete(Value::String(part.to_string())))
                .collect();
            Ok(ResolvedValue::Seq(parts))
        }
        ResolvedValue::Interpolated(_)
        | ResolvedValue::Ref(_)
        | ResolvedValue::GetAtt { .. }
        | ResolvedValue::Concrete(Value::Tagged(_)) => {
            // Source contains unresolved resource references or an unknown
            // pass-through intrinsic (e.g. !Base64 "a,b").  The exact split
            // positions cannot be determined statically.  Warn and pass through
            // the original !Split expression so downstream logic sees the full
            // unresolved value rather than silently producing incorrect results.
            tracing::warn!(
                separator = %separator,
                source = ?source,
                "Fn::Split source cannot be resolved statically; \
                 passing through as-is"
            );
            Ok(ResolvedValue::Concrete(Value::Tagged(Box::new(
                serde_yaml_ng::value::TaggedValue {
                    tag: serde_yaml_ng::value::Tag::new("!Split"),
                    value: value.clone(),
                },
            ))))
        }
        _ => {
            // Any other non-string source (null, map, seq, …) — error out.
            Err(CfnError::IntrinsicError(format!(
                "Fn::Split source (second element) must be a string, got {source:?}"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn concrete_str(s: &str) -> ResolvedValue {
        ResolvedValue::Concrete(Value::String(s.into()))
    }

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
        assert_eq!(result, concrete_str("prod"));
    }

    #[test]
    fn test_resolve_ref_resource_becomes_typed_ref() {
        let ctx = make_ctx();
        let val = Value::String("MyQueue".into());
        let result = resolve_ref(&val, &ctx).unwrap();
        assert_eq!(result, ResolvedValue::Ref("MyQueue".into()));
    }

    #[test]
    fn test_resolve_sub() {
        let ctx = make_ctx();
        let val = Value::String("arn:aws:s3:::${Env}-bucket".into());
        let result = resolve_sub(&val, &ctx, 0).unwrap();
        assert_eq!(result, concrete_str("arn:aws:s3:::prod-bucket"));
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
        assert_eq!(result, concrete_str("ami-12345"));
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
        assert_eq!(result, concrete_str("prod-value"));
    }

    #[test]
    fn test_resolve_import_value() {
        let ctx = make_ctx();
        let val = Value::String("SharedVpcId".into());
        let result = resolve_import_value(&val, &ctx, 0).unwrap();
        assert_eq!(result, concrete_str("vpc-12345"));
    }

    // --- GetAtt edge cases (#5) ---

    /// A dot-notation GetAtt must produce a typed GetAtt.
    #[test]
    fn test_getatt_dot_notation_produces_typed_getatt() {
        let val = Value::String("MyBucket.Arn".into());
        let result = resolve_get_att(&val).unwrap();
        assert_eq!(
            result,
            ResolvedValue::GetAtt {
                logical_id: "MyBucket".into(),
                attr: "Arn".into()
            },
            "dot-notation GetAtt must produce a typed GetAtt"
        );
    }

    /// A string with no dot must NOT produce a reference — it must be passed
    /// through as-is (unresolved).
    #[test]
    fn test_getatt_no_dot_passes_through() {
        let val = Value::String("MyBucketNoAttr".into());
        let result = resolve_get_att(&val).unwrap();
        // Must return the original value unchanged (no reference generated).
        assert_eq!(
            result,
            concrete_str("MyBucketNoAttr"),
            "GetAtt without dot must be a pass-through, not a reference"
        );
    }

    /// A sequence with exactly 2 elements must produce the typed GetAtt.
    #[test]
    fn test_getatt_sequence_two_elements_produces_typed_getatt() {
        let val = Value::Sequence(vec![
            Value::String("MyQueue".into()),
            Value::String("Arn".into()),
        ]);
        let result = resolve_get_att(&val).unwrap();
        assert_eq!(
            result,
            ResolvedValue::GetAtt {
                logical_id: "MyQueue".into(),
                attr: "Arn".into()
            }
        );
    }

    /// A sequence with 3+ elements must join the extra parts with "." as the attr.
    #[test]
    fn test_getatt_sequence_three_elements_joins_attr() {
        let val = Value::Sequence(vec![
            Value::String("MyResource".into()),
            Value::String("SomeNested".into()),
            Value::String("Attr".into()),
        ]);
        let result = resolve_get_att(&val).unwrap();
        assert_eq!(
            result,
            ResolvedValue::GetAtt {
                logical_id: "MyResource".into(),
                attr: "SomeNested.Attr".into()
            },
            "3-element sequence must join tail with '.'"
        );
    }

    // --- Sub reference typing (#1, reworked for #14) ---

    /// `!Sub '${Fn.Arn}'` (bare resource.attr) must produce a typed GetAtt,
    /// not the literal `${HandlerFunction.Arn}`.
    #[test]
    fn test_sub_resource_attr_becomes_typed_getatt() {
        let ctx = make_ctx();
        let val = Value::String("${HandlerFunction.Arn}".into());
        let result = resolve_sub(&val, &ctx, 0).unwrap();
        assert_eq!(
            result,
            ResolvedValue::GetAtt {
                logical_id: "HandlerFunction".into(),
                attr: "Arn".into()
            },
            "!Sub '${{Resource.Attr}}' must produce a typed GetAtt"
        );
    }

    /// `!Sub '${MyQueue}'` (bare resource ref, not in parameters) must produce
    /// a typed Ref, not the literal `${MyQueue}`.
    #[test]
    fn test_sub_resource_ref_becomes_typed_ref() {
        let ctx = make_ctx();
        // "MyQueue" is not a parameter in make_ctx(), so it must become a Ref.
        let val = Value::String("${MyQueue}".into());
        let result = resolve_sub(&val, &ctx, 0).unwrap();
        assert_eq!(
            result,
            ResolvedValue::Ref("MyQueue".into()),
            "!Sub '${{Resource}}' must produce a typed Ref"
        );
    }

    /// `!Sub '${AWS::Region}'` must remain verbatim — pseudo-parameters are
    /// never turned into references.
    #[test]
    fn test_sub_pseudo_parameter_stays_verbatim() {
        let ctx = make_ctx();
        let val = Value::String("${AWS::Region}".into());
        let result = resolve_sub(&val, &ctx, 0).unwrap();
        assert_eq!(
            result,
            concrete_str("${AWS::Region}"),
            "!Sub '${{AWS::Region}}' must remain verbatim (no reference)"
        );
    }

    /// An embedded `!Sub` such as `'arn:...:${MyQueue}/p'` keeps the reference
    /// inside an Interpolated value together with the literal text.
    #[test]
    fn test_sub_embedded_resource_ref_is_interpolated() {
        let ctx = make_ctx();
        let val = Value::String("arn:aws:sqs:us-east-1:123456789012:${MyQueue}/suffix".into());
        let result = resolve_sub(&val, &ctx, 0).unwrap();
        assert_eq!(
            result,
            ResolvedValue::Interpolated(vec![
                StringPart::Literal("arn:aws:sqs:us-east-1:123456789012:".into()),
                StringPart::Ref("MyQueue".into()),
                StringPart::Literal("/suffix".into()),
            ]),
            "embedded ref must be kept as an Interpolated part"
        );
    }

    /// `!Sub` with multiple resource references must keep ALL of them as parts
    /// (the previous sentinel design silently dropped the second one).
    #[test]
    fn test_sub_multiple_references_all_kept() {
        let ctx = make_ctx();
        let val = Value::String("${Fn1.Arn}:${Fn2.Arn}".into());
        let result = resolve_sub(&val, &ctx, 0).unwrap();
        match &result {
            ResolvedValue::Interpolated(parts) => {
                let refs: Vec<_> = parts
                    .iter()
                    .filter(|p| !matches!(p, StringPart::Literal(_)))
                    .collect();
                assert_eq!(refs.len(), 2, "both references must be kept: {parts:?}");
            }
            other => panic!("expected Interpolated, got {other:?}"),
        }
        let refs = result.references();
        assert_eq!(refs[0].logical_id, "Fn1");
        assert_eq!(refs[1].logical_id, "Fn2");
    }

    // --- Fn::Sub ${!Literal} escape (#2) ---

    /// `${!NotAVar}` must resolve to the literal `${NotAVar}` (no reference).
    #[test]
    fn test_sub_bang_escape_produces_literal() {
        let ctx = make_ctx();
        let val = Value::String("foo-${!NotAVar}-bar".into());
        let result = resolve_sub(&val, &ctx, 0).unwrap();
        assert_eq!(
            result,
            concrete_str("foo-${NotAVar}-bar"),
            "!Sub '${{!Literal}}' must produce literal '${{Literal}}', not a reference"
        );
    }

    /// The escape must NOT produce any reference.
    #[test]
    fn test_sub_bang_escape_does_not_create_reference() {
        let ctx = make_ctx();
        let val = Value::String("${!SomeVar}".into());
        let result = resolve_sub(&val, &ctx, 0).unwrap();
        assert!(
            result.references().is_empty(),
            "escaped variable must not produce a reference: {result:?}"
        );
        assert_eq!(result, concrete_str("${SomeVar}"));
    }

    /// Normal `${Resource}` refs alongside `${!Escaped}` must both work correctly.
    #[test]
    fn test_sub_bang_escape_mixed_with_resource_ref() {
        let ctx = make_ctx();
        // MyQueue is not a parameter in make_ctx(), so it becomes a Ref part.
        let val = Value::String("${MyQueue}-${!NotAVar}".into());
        let result = resolve_sub(&val, &ctx, 0).unwrap();
        assert_eq!(
            result,
            ResolvedValue::Interpolated(vec![
                StringPart::Ref("MyQueue".into()),
                StringPart::Literal("-${NotAVar}".into()),
            ]),
            "resource ref must stay typed while escaped var stays literal"
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
            concrete_str("true:suffix:false"),
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
            concrete_str("false"),
            "single Bool element must not be silently dropped"
        );
    }

    /// `!Join` over resource references must keep every reference as a typed
    /// part — joining two refs must NOT collapse into an ambiguous string.
    #[test]
    fn test_join_keeps_all_references() {
        let ctx = make_ctx();
        // Join [":", [{"Ref": A}, {"Ref": B}]] (long form)
        let long_ref = |id: &str| {
            let mut m = serde_yaml_ng::Mapping::new();
            m.insert(Value::String("Ref".into()), Value::String(id.into()));
            Value::Mapping(m)
        };
        let val = Value::Sequence(vec![
            Value::String(":".into()),
            Value::Sequence(vec![long_ref("ResourceA"), long_ref("ResourceB")]),
        ]);
        let result = resolve_join(&val, &ctx, 0).unwrap();
        let refs = result.references();
        assert_eq!(refs.len(), 2, "both join refs must survive: {result:?}");
        assert_eq!(refs[0].logical_id, "ResourceA");
        assert_eq!(refs[1].logical_id, "ResourceB");
    }

    // --- resolve_sub 2-arg non-string local var (#6) ---

    /// 2-arg `!Sub` where a local var resolves to a non-string (e.g. Number)
    /// must substitute the key with empty string, NOT produce a reference.
    #[test]
    fn test_sub_two_arg_non_string_local_var_becomes_empty_not_reference() {
        let ctx = make_ctx();
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
        assert!(
            result.references().is_empty(),
            "non-string local var must not produce a reference: {result:?}"
        );
        assert_eq!(
            result,
            concrete_str("prefix--suffix"),
            "non-string local var must become empty"
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
        assert_eq!(result, concrete_str("hello-world"));
    }

    /// 2-arg `!Sub` where a local var resolves to a resource reference must
    /// keep the reference typed.
    #[test]
    fn test_sub_two_arg_ref_local_var_stays_typed() {
        let ctx = make_ctx();
        let val = Value::Sequence(vec![
            Value::String("arn:${V}/x".into()),
            Value::Mapping({
                let mut m = serde_yaml_ng::Mapping::new();
                m.insert(Value::String("V".into()), {
                    let mut r = serde_yaml_ng::Mapping::new();
                    r.insert(Value::String("Ref".into()), Value::String("MyQueue".into()));
                    Value::Mapping(r)
                });
                m
            }),
        ]);
        let result = resolve_sub(&val, &ctx, 0).unwrap();
        assert_eq!(
            result,
            ResolvedValue::Interpolated(vec![
                StringPart::Literal("arn:".into()),
                StringPart::Ref("MyQueue".into()),
                StringPart::Literal("/x".into()),
            ])
        );
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
        assert_eq!(result, concrete_str("second"));
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
        assert_eq!(result, concrete_str("first"));
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
        assert_eq!(result.unwrap(), concrete_str("leaf"));
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

    // --- normalize_intrinsic_name unit tests ---

    /// All tag-form and long-form variants of known intrinsics must return a
    /// canonical name (not `None`).
    #[test]
    fn test_normalize_known_intrinsics_returns_some() {
        let known = [
            ("!Ref", "Ref"),
            ("Ref", "Ref"),
            ("!Sub", "Fn::Sub"),
            ("Fn::Sub", "Fn::Sub"),
            ("!FindInMap", "Fn::FindInMap"),
            ("Fn::FindInMap", "Fn::FindInMap"),
            ("!Select", "Fn::Select"),
            ("Fn::Select", "Fn::Select"),
            ("!If", "Fn::If"),
            ("Fn::If", "Fn::If"),
            ("!ImportValue", "Fn::ImportValue"),
            ("Fn::ImportValue", "Fn::ImportValue"),
            ("!Join", "Fn::Join"),
            ("Fn::Join", "Fn::Join"),
            ("!GetAtt", "Fn::GetAtt"),
            ("Fn::GetAtt", "Fn::GetAtt"),
            ("!Split", "Fn::Split"),
            ("Fn::Split", "Fn::Split"),
        ];
        for (input, expected) in known {
            assert_eq!(
                normalize_intrinsic_name(input),
                Some(expected),
                "normalize_intrinsic_name({input:?}) should be Some({expected:?})"
            );
        }
    }

    /// Unknown names must return `None`.
    #[test]
    fn test_normalize_unknown_intrinsic_returns_none() {
        let unknown = [
            "Fn::Base64",
            "Fn::Cidr",
            "Fn::Transform",
            "Fn::Length",
            "Fn::ToJsonString",
            "Fn::ForEach",
            "!Base64",
            "SomeRandomKey",
        ];
        for name in unknown {
            assert_eq!(
                normalize_intrinsic_name(name),
                None,
                "normalize_intrinsic_name({name:?}) should be None"
            );
        }
    }

    // --- Fn::Split tests ---

    /// `Fn::Split` on a concrete string must produce a `Seq` of the parts.
    #[test]
    fn test_split_concrete_string() {
        let ctx = make_ctx();
        let val = Value::Sequence(vec![
            Value::String(",".into()),
            Value::String("a,b,c".into()),
        ]);
        let result = resolve_split(&val, &ctx, 0).unwrap();
        assert_eq!(
            result,
            ResolvedValue::Seq(vec![
                concrete_str("a"),
                concrete_str("b"),
                concrete_str("c"),
            ]),
            "Fn::Split on a concrete string must produce a Seq of parts"
        );
    }

    /// `Fn::Split` long-form via `resolve` must also work.
    #[test]
    fn test_split_long_form_via_resolve() {
        let ctx = make_ctx();
        let mut m = serde_yaml_ng::Mapping::new();
        m.insert(
            Value::String("Fn::Split".into()),
            Value::Sequence(vec![Value::String(",".into()), Value::String("x,y".into())]),
        );
        let result = resolve(&Value::Mapping(m), &ctx).unwrap();
        assert_eq!(
            result,
            ResolvedValue::Seq(vec![concrete_str("x"), concrete_str("y")]),
        );
    }

    /// `!Split` tag-form via `resolve` must also work.
    #[test]
    fn test_split_tag_form_via_resolve() {
        let ctx = make_ctx();
        let tagged = Value::Tagged(Box::new(serde_yaml_ng::value::TaggedValue {
            tag: serde_yaml_ng::value::Tag::new("!Split"),
            value: Value::Sequence(vec![
                Value::String("|".into()),
                Value::String("one|two|three".into()),
            ]),
        }));
        let result = resolve(&tagged, &ctx).unwrap();
        assert_eq!(
            result,
            ResolvedValue::Seq(vec![
                concrete_str("one"),
                concrete_str("two"),
                concrete_str("three"),
            ]),
        );
    }

    /// `!Select [1, !Split [",", "a,b,c"]]` must resolve to "b".
    #[test]
    fn test_select_with_split_nested() {
        let ctx = make_ctx();
        // Build: !Select [1, !Split [",", "a,b,c"]]
        let split_tagged = Value::Tagged(Box::new(serde_yaml_ng::value::TaggedValue {
            tag: serde_yaml_ng::value::Tag::new("!Split"),
            value: Value::Sequence(vec![
                Value::String(",".into()),
                Value::String("a,b,c".into()),
            ]),
        }));
        let select_tagged = Value::Tagged(Box::new(serde_yaml_ng::value::TaggedValue {
            tag: serde_yaml_ng::value::Tag::new("!Select"),
            value: Value::Sequence(vec![
                Value::Number(serde_yaml_ng::value::Number::from(1_u64)),
                split_tagged,
            ]),
        }));
        let result = resolve(&select_tagged, &ctx).unwrap();
        assert_eq!(
            result,
            concrete_str("b"),
            "!Select [1, !Split [\",\", \"a,b,c\"]] must yield \"b\""
        );
    }

    /// `!Select [0, !Split [",", !Sub "${Queue},x"]]` — when the Split source
    /// is Interpolated the whole expression must pass through without error
    /// (both Split and the parent Select degrade gracefully).
    #[test]
    fn test_select_with_unresolvable_split_passes_through() {
        let ctx = make_ctx();
        // !Sub "${MyQueue},x" → Interpolated (MyQueue is not a parameter)
        let sub_tagged = Value::Tagged(Box::new(serde_yaml_ng::value::TaggedValue {
            tag: serde_yaml_ng::value::Tag::new("!Sub"),
            value: Value::String("${MyQueue},x".into()),
        }));
        let split_tagged = Value::Tagged(Box::new(serde_yaml_ng::value::TaggedValue {
            tag: serde_yaml_ng::value::Tag::new("!Split"),
            value: Value::Sequence(vec![Value::String(",".into()), sub_tagged]),
        }));
        let select_tagged = Value::Tagged(Box::new(serde_yaml_ng::value::TaggedValue {
            tag: serde_yaml_ng::value::Tag::new("!Select"),
            value: Value::Sequence(vec![
                Value::Number(serde_yaml_ng::value::Number::from(0_u64)),
                split_tagged,
            ]),
        }));
        let result = resolve(&select_tagged, &ctx);
        assert!(
            result.is_ok(),
            "!Select over unresolvable !Split must not error, got: {result:?}"
        );
        // The pass-through must be the outer !Select expression, not the inner
        // !Split — callers must see the full expression including the index.
        match result.unwrap() {
            ResolvedValue::Concrete(Value::Tagged(t)) => {
                assert_eq!(
                    t.tag.to_string(),
                    "!Select",
                    "pass-through must preserve the outer !Select tag, got {:?}",
                    t.tag
                );
            }
            other => panic!("expected Concrete(Tagged(!Select ...)), got {other:?}"),
        }
    }

    /// `!Split [",", !Base64 "a,b"]` — when the source is an unknown
    /// pass-through intrinsic (Concrete(Tagged)), Split must not error.
    #[test]
    fn test_split_unknown_intrinsic_source_passes_through() {
        let ctx = make_ctx();
        let base64_tagged = Value::Tagged(Box::new(serde_yaml_ng::value::TaggedValue {
            tag: serde_yaml_ng::value::Tag::new("!Base64"),
            value: Value::String("a,b".into()),
        }));
        let val = Value::Sequence(vec![Value::String(",".into()), base64_tagged]);
        let result = resolve_split(&val, &ctx, 0);
        assert!(
            result.is_ok(),
            "Fn::Split with unknown intrinsic source must not error, got: {result:?}"
        );
        // Must be a Concrete(Tagged(!Split ...)) pass-through, not a Seq.
        match result.unwrap() {
            ResolvedValue::Concrete(Value::Tagged(t)) => {
                assert_eq!(
                    t.tag.to_string(),
                    "!Split",
                    "pass-through must be tagged !Split, got {:?}",
                    t.tag
                );
            }
            other => panic!("expected Concrete(Tagged(!Split ...)), got {other:?}"),
        }
    }

    /// `Fn::Split` where source is an `Interpolated` value (unresolved ref)
    /// must pass through as Concrete (not error) and emit a warning.
    #[test]
    fn test_split_interpolated_source_passes_through() {
        let ctx = make_ctx();
        // Build a source that resolves to an Interpolated value: !Sub "a,${MyQueue},c"
        let sub_val = Value::Tagged(Box::new(serde_yaml_ng::value::TaggedValue {
            tag: serde_yaml_ng::value::Tag::new("!Sub"),
            value: Value::String("a,${MyQueue},c".into()),
        }));
        // We call resolve_split directly with the pre-built argument sequence.
        // The second element is the !Sub expression.
        let val = Value::Sequence(vec![Value::String(",".into()), sub_val]);
        let result = resolve_split(&val, &ctx, 0);
        assert!(
            result.is_ok(),
            "Fn::Split on an Interpolated source must not error, got: {result:?}"
        );
        // Must be a pass-through (Concrete), not a Seq.
        assert!(
            !matches!(result.unwrap(), ResolvedValue::Seq(_)),
            "Fn::Split on Interpolated source must not produce a Seq"
        );
    }

    // --- Unknown intrinsic pass-through (warn) ---

    /// An unknown `!Tag` must pass through as `Concrete` without returning an
    /// error (silent degradation, but a warning is emitted at runtime).
    #[test]
    fn test_unknown_tag_passes_through() {
        let ctx = make_ctx();
        let tagged_val = Value::Tagged(Box::new(serde_yaml_ng::value::TaggedValue {
            tag: serde_yaml_ng::value::Tag::new("!Base64"),
            value: Value::String("hello".into()),
        }));
        let result = resolve(&tagged_val, &ctx);
        assert!(
            result.is_ok(),
            "unknown tag must not return an error, got: {result:?}"
        );
        // The returned value must be Concrete (pass-through), not a reference.
        assert!(
            result.unwrap().references().is_empty(),
            "unknown tag pass-through must carry no references"
        );
    }

    /// An unknown `Fn::*` single-key map must pass through as a resolved map
    /// (the key and value are kept) without returning an error.
    #[test]
    fn test_unknown_fn_map_passes_through() {
        let ctx = make_ctx();
        let mut m = serde_yaml_ng::Mapping::new();
        m.insert(
            Value::String("Fn::Transform".into()),
            Value::String("some-macro".into()),
        );
        let result = resolve(&Value::Mapping(m), &ctx);
        assert!(
            result.is_ok(),
            "unknown Fn::* map must not return an error, got: {result:?}"
        );
    }
}
