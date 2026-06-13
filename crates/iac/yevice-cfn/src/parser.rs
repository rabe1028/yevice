use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde_yaml_ng::Value;
use yevice_core::io::read_iac_file;
use yevice_core::parse_policy::{
    DiagnosticSource, IacParseDiagnostic, ParseOutcome, ParsePolicy, SourceLocation,
};

use crate::error::CfnError;
use crate::intrinsic::{ResolveContext, resolve};
use crate::resolved::ResolvedValue;

/// Parsed `CloudFormation` template.
///
/// Generic over the resource property representation `P`:
/// - freshly parsed templates use the default `serde_yaml_ng::Value`
///   (raw YAML, intrinsics still tagged),
/// - after [`resolve_template`] the properties become [`ResolvedValue`]
///   (intrinsics evaluated, resource references typed).
pub struct CfnTemplate<P = Value> {
    pub parameters: HashMap<String, ParameterDef>,
    pub mappings: HashMap<String, HashMap<String, HashMap<String, String>>>,
    pub conditions: HashMap<String, Value>,
    pub resources: BTreeMap<String, CfnResource<P>>,
}

/// A `CloudFormation` parameter definition.
pub struct ParameterDef {
    pub param_type: String,
    pub default: Option<String>,
    pub allowed_values: Vec<String>,
}

/// A `CloudFormation` resource.
///
/// Generic over the property representation `P` (see [`CfnTemplate`]).
#[derive(Clone, Debug)]
pub struct CfnResource<P = Value> {
    pub logical_id: String,
    pub resource_type: String,
    pub properties: P,
    pub condition: Option<String>,
    /// Logical IDs listed in `DependsOn` (strings or arrays of strings).
    /// Parsed for future use; not converted to edges (no `DependsOn` `ConnectionType`).
    #[allow(dead_code)]
    pub depends_on: Vec<String>,
}

/// A resource whose intrinsic functions have been resolved.
pub type ResolvedResource = CfnResource<ResolvedValue>;

/// A template whose resource properties have been resolved.
pub type ResolvedTemplate = CfnTemplate<ResolvedValue>;

/// Parse a `CloudFormation` YAML template from a file.
pub fn parse_template(path: &Path) -> Result<CfnTemplate, CfnError> {
    let content = read_iac_file(path)?;
    parse_template_str(&content)
}

/// Parse a `CloudFormation` YAML template from a string.
pub fn parse_template_str(content: &str) -> Result<CfnTemplate, CfnError> {
    let root: Value = serde_yaml_ng::from_str(content)?;
    let root_map = root
        .as_mapping()
        .ok_or_else(|| CfnError::ParseError("template root must be a mapping".into()))?;

    let parameters = parse_parameters(root_map);
    let mappings = parse_mappings(root_map);
    let conditions = parse_conditions(root_map);
    let resources = parse_resources(root_map)?;

    Ok(CfnTemplate {
        parameters,
        mappings,
        conditions,
        resources,
    })
}

fn parse_parameters(root: &serde_yaml_ng::Mapping) -> HashMap<String, ParameterDef> {
    let mut params = HashMap::new();

    let Some(Value::Mapping(section)) = root.get(Value::String("Parameters".into())) else {
        return params;
    };

    for (key, val) in section {
        let Some(name) = key.as_str() else { continue };
        let Some(def) = val.as_mapping() else {
            continue;
        };

        let param_type = def
            .get(Value::String("Type".into()))
            .and_then(|v| v.as_str())
            .unwrap_or("String")
            .to_string();

        let default = def
            .get(Value::String("Default".into()))
            .and_then(|v| match v {
                Value::String(s) => Some(s.clone()),
                Value::Number(n) => Some(n.to_string()),
                Value::Bool(b) => Some(b.to_string()),
                _ => None,
            });

        let allowed_values = def
            .get(Value::String("AllowedValues".into()))
            .and_then(|v| v.as_sequence())
            .map(|seq| {
                seq.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        params.insert(
            name.to_string(),
            ParameterDef {
                param_type,
                default,
                allowed_values,
            },
        );
    }

    params
}

fn parse_mappings(
    root: &serde_yaml_ng::Mapping,
) -> HashMap<String, HashMap<String, HashMap<String, String>>> {
    let mut mappings = HashMap::new();

    let Some(Value::Mapping(section)) = root.get(Value::String("Mappings".into())) else {
        return mappings;
    };

    for (map_name, map_val) in section {
        let Some(name) = map_name.as_str() else {
            continue;
        };
        let Some(first_level) = map_val.as_mapping() else {
            continue;
        };

        let mut first_map = HashMap::new();
        for (fk, fv) in first_level {
            let Some(first_key) = fk.as_str() else {
                continue;
            };
            let Some(second_level) = fv.as_mapping() else {
                continue;
            };

            let mut second_map = HashMap::new();
            for (sk, sv) in second_level {
                let Some(second_key) = sk.as_str() else {
                    continue;
                };
                let val_str = match sv {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    _ => continue,
                };
                second_map.insert(second_key.to_string(), val_str);
            }
            first_map.insert(first_key.to_string(), second_map);
        }
        mappings.insert(name.to_string(), first_map);
    }

    mappings
}

fn parse_conditions(root: &serde_yaml_ng::Mapping) -> HashMap<String, Value> {
    let mut conditions = HashMap::new();

    let Some(Value::Mapping(section)) = root.get(Value::String("Conditions".into())) else {
        return conditions;
    };

    for (key, val) in section {
        if let Some(name) = key.as_str() {
            conditions.insert(name.to_string(), val.clone());
        }
    }

    conditions
}

fn parse_resources(
    root: &serde_yaml_ng::Mapping,
) -> Result<BTreeMap<String, CfnResource>, CfnError> {
    let mut resources = BTreeMap::new();

    let section = root
        .get(Value::String("Resources".into()))
        .and_then(|v| v.as_mapping())
        .ok_or_else(|| CfnError::ParseError("template must have a Resources section".into()))?;

    for (key, val) in section {
        let Some(name) = key.as_str() else { continue };
        let Some(resource_def) = val.as_mapping() else {
            continue;
        };

        let resource_type = resource_def
            .get(Value::String("Type".into()))
            .and_then(|v| v.as_str())
            .ok_or_else(|| CfnError::ParseError(format!("resource {name} missing Type")))?
            .to_string();

        let properties = resource_def
            .get(Value::String("Properties".into()))
            .cloned()
            .unwrap_or(Value::Mapping(serde_yaml_ng::Mapping::new()));

        let condition = resource_def
            .get(Value::String("Condition".into()))
            .and_then(|v| v.as_str())
            .map(String::from);

        let depends_on = match resource_def.get(Value::String("DependsOn".into())) {
            Some(Value::String(s)) => vec![s.clone()],
            Some(Value::Sequence(seq)) => seq
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect(),
            _ => Vec::new(),
        };

        resources.insert(
            name.to_string(),
            CfnResource {
                logical_id: name.to_string(),
                resource_type,
                properties,
                condition,
                depends_on,
            },
        );
    }

    Ok(resources)
}

/// Resolve all intrinsic functions in a template's resource properties.
///
/// Defaults to [`ParsePolicy::Strict`] for backward compatibility — i.e.
/// preserves the historical CFN behaviour where missing parameters / unresolved
/// `Fn::ImportValue` / unknown mapping keys abort the resolve. Use
/// [`resolve_template_with_policy`] to opt into Lenient mode.
pub fn resolve_template(
    template: &CfnTemplate,
    param_values: &HashMap<String, String>,
    import_values: &HashMap<String, String>,
) -> Result<BTreeMap<String, ResolvedResource>, CfnError> {
    let outcome = resolve_template_with_policy(
        template,
        param_values,
        import_values,
        ParsePolicy::Strict,
        None,
    )?;
    Ok(outcome.value)
}

/// Resolve all intrinsic functions in a template's resource properties under
/// the given [`ParsePolicy`].
///
/// Under [`ParsePolicy::Lenient`] the four policy-controllable variants
/// ([`CfnError::MissingParameters`], [`CfnError::ParameterNotFound`],
/// [`CfnError::ImportValueNotFound`], [`CfnError::MappingNotFound`]) are
/// demoted to [`IacParseDiagnostic`] entries on the returned [`ParseOutcome`]
/// and resolution continues best-effort.
///
/// Syntax / IO / programmer-error variants ([`CfnError::Yaml`],
/// [`CfnError::Io`], [`CfnError::IntrinsicError`], [`CfnError::ParseError`])
/// stay policy-neutral and abort.
///
/// `template_path`, when supplied, is attached to each diagnostic's
/// [`SourceLocation`].
pub fn resolve_template_with_policy(
    template: &CfnTemplate,
    param_values: &HashMap<String, String>,
    import_values: &HashMap<String, String>,
    policy: ParsePolicy,
    template_path: Option<&Path>,
) -> Result<ParseOutcome<BTreeMap<String, ResolvedResource>>, CfnError> {
    let mut diagnostics: Vec<IacParseDiagnostic> = Vec::new();

    // Build effective parameters: supplied values override defaults
    let mut effective_params = HashMap::new();
    for (name, def) in &template.parameters {
        if let Some(val) = param_values.get(name) {
            effective_params.insert(name.clone(), val.clone());
        } else if let Some(default) = &def.default {
            effective_params.insert(name.clone(), default.clone());
        }
    }

    // Validate that every declared parameter without a Default was supplied.
    let mut missing: Vec<String> = template
        .parameters
        .iter()
        .filter(|(name, def)| def.default.is_none() && !param_values.contains_key(*name))
        .map(|(name, _)| name.clone())
        .collect();
    if !missing.is_empty() {
        missing.sort();
        match policy {
            ParsePolicy::Strict => {
                return Err(CfnError::MissingParameters(missing.join(", ")));
            }
            ParsePolicy::Lenient => {
                // Best-effort: leave the parameter unbound (downstream `!Ref`
                // will then raise ParameterNotFound, also demoted below).
                diagnostics.push(missing_parameters_diagnostic(&missing, template_path));
            }
        }
    }

    // Evaluate conditions
    let conditions =
        evaluate_conditions(&template.conditions, &effective_params, &template.mappings);

    let mut ctx = ResolveContext::new(effective_params, import_values.clone());
    ctx.mappings.clone_from(&template.mappings);
    ctx.conditions.clone_from(&conditions);

    let mut resolved_resources = BTreeMap::new();
    for (id, resource) in &template.resources {
        // Skip resources with a false condition
        if let Some(cond_name) = &resource.condition
            && let Some(false) = conditions.get(cond_name)
        {
            continue;
        }

        let resolved_props = match resolve(&resource.properties, &ctx) {
            Ok(v) => v,
            Err(e) if policy == ParsePolicy::Lenient && is_policy_controllable(&e) => {
                diagnostics.push(cfn_error_to_diagnostic(&e, template_path));
                // Best-effort: skip the offending resource. Including a
                // partially-resolved value here would risk handing wrong data
                // to downstream adapters; downstream is more forgiving of a
                // missing resource than a silently-wrong one.
                continue;
            }
            Err(e) => return Err(e),
        };
        resolved_resources.insert(
            id.clone(),
            CfnResource {
                logical_id: resource.logical_id.clone(),
                resource_type: resource.resource_type.clone(),
                properties: resolved_props,
                condition: resource.condition.clone(),
                depends_on: resource.depends_on.clone(),
            },
        );
    }

    Ok(ParseOutcome::with_diagnostics(
        resolved_resources,
        diagnostics,
    ))
}

/// Whether a `CfnError` is one of the four ADR-0003 policy-controllable
/// variants (Phase 1).
fn is_policy_controllable(err: &CfnError) -> bool {
    matches!(
        err,
        CfnError::MissingParameters(_)
            | CfnError::ParameterNotFound(_)
            | CfnError::ImportValueNotFound(_)
            | CfnError::MappingNotFound { .. }
    )
}

fn missing_parameters_diagnostic(
    missing: &[String],
    template_path: Option<&Path>,
) -> IacParseDiagnostic {
    let mut d = IacParseDiagnostic::error(
        DiagnosticSource::Cfn,
        "missing_parameter",
        format!("Parameters: [{}] must have values", missing.join(", ")),
    );
    if let Some(p) = template_path {
        d = d.with_location(SourceLocation::file_only(PathBuf::from(p)));
    }
    d
}

fn cfn_error_to_diagnostic(err: &CfnError, template_path: Option<&Path>) -> IacParseDiagnostic {
    let (code, message): (&str, String) = match err {
        CfnError::MissingParameters(names) => (
            "missing_parameter",
            format!("Parameters: [{names}] must have values"),
        ),
        CfnError::ParameterNotFound(name) => (
            "parameter_not_found",
            format!("parameter not found: {name}"),
        ),
        CfnError::ImportValueNotFound(name) => (
            "import_value_not_found",
            format!("import value not found: {name}"),
        ),
        CfnError::MappingNotFound {
            map_name,
            first_key,
            second_key,
        } => (
            "mapping_not_found",
            format!("mapping not found: {map_name}.{first_key}.{second_key}"),
        ),
        // Defensive: the caller guards via `is_policy_controllable`, but keep
        // a fallback so adding new variants does not silently produce empty
        // diagnostics.
        other => ("cfn_error", other.to_string()),
    };
    let mut d = IacParseDiagnostic::error(DiagnosticSource::Cfn, code, message);
    if let Some(p) = template_path {
        d = d.with_location(SourceLocation::file_only(PathBuf::from(p)));
    }
    d
}

/// Context for condition evaluation.
struct ConditionContext<'a> {
    params: &'a HashMap<String, String>,
    mappings: &'a HashMap<String, HashMap<String, HashMap<String, String>>>,
}

/// Evaluate conditions.
fn evaluate_conditions(
    conditions: &HashMap<String, Value>,
    params: &HashMap<String, String>,
    mappings: &HashMap<String, HashMap<String, HashMap<String, String>>>,
) -> HashMap<String, bool> {
    let ctx = ConditionContext { params, mappings };
    let mut result = HashMap::new();

    for (name, value) in conditions {
        let evaluated = evaluate_condition(value, &ctx);
        result.insert(name.clone(), evaluated);
    }

    result
}

fn evaluate_condition(value: &Value, ctx: &ConditionContext) -> bool {
    match value {
        Value::Tagged(tagged) => {
            let tag = tagged.tag.to_string();
            match tag.as_str() {
                "!Equals" => {
                    if let Some(seq) = tagged.value.as_sequence()
                        && seq.len() == 2
                    {
                        let Ok(a) = resolve_condition_value(&seq[0], ctx) else {
                            return false;
                        };
                        let Ok(b) = resolve_condition_value(&seq[1], ctx) else {
                            return false;
                        };
                        return a == b;
                    }
                    false
                }
                "!Not" => {
                    if let Some(seq) = tagged.value.as_sequence()
                        && seq.len() == 1
                    {
                        return !evaluate_condition(&seq[0], ctx);
                    }
                    false
                }
                "!And" => {
                    if let Some(seq) = tagged.value.as_sequence() {
                        return seq.iter().all(|v| evaluate_condition(v, ctx));
                    }
                    false
                }
                "!Or" => {
                    if let Some(seq) = tagged.value.as_sequence() {
                        return seq.iter().any(|v| evaluate_condition(v, ctx));
                    }
                    false
                }
                _ => false,
            }
        }
        Value::Mapping(map) => {
            if let Some(seq) = map.get(Value::String("Fn::Equals".into()))
                && let Some(seq) = seq.as_sequence()
                && seq.len() == 2
            {
                let Ok(a) = resolve_condition_value(&seq[0], ctx) else {
                    return false;
                };
                let Ok(b) = resolve_condition_value(&seq[1], ctx) else {
                    return false;
                };
                return a == b;
            }
            false
        }
        _ => false,
    }
}

fn resolve_condition_value(value: &Value, ctx: &ConditionContext) -> Result<String, CfnError> {
    match value {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        Value::Tagged(tagged) => {
            let tag = tagged.tag.to_string();
            match tag.as_str() {
                "!Ref" => {
                    if let Some(name) = tagged.value.as_str() {
                        return ctx
                            .params
                            .get(name)
                            .cloned()
                            .ok_or_else(|| CfnError::ParameterNotFound(name.to_string()));
                    }
                    Err(CfnError::IntrinsicError(
                        "!Ref in condition must reference a string name".into(),
                    ))
                }
                "!FindInMap" => {
                    if let Some(seq) = tagged.value.as_sequence()
                        && seq.len() == 3
                    {
                        let map_name = resolve_condition_value(&seq[0], ctx)?;
                        let first_key = resolve_condition_value(&seq[1], ctx)?;
                        let second_key = resolve_condition_value(&seq[2], ctx)?;
                        return ctx
                            .mappings
                            .get(&map_name)
                            .and_then(|m| m.get(&first_key))
                            .and_then(|m| m.get(&second_key))
                            .cloned()
                            .ok_or_else(|| CfnError::MappingNotFound {
                                map_name: map_name.clone(),
                                first_key: first_key.clone(),
                                second_key: second_key.clone(),
                            });
                    }
                    Err(CfnError::IntrinsicError(
                        "!FindInMap in condition must have exactly 3 elements".into(),
                    ))
                }
                _ => Err(CfnError::IntrinsicError(format!(
                    "unsupported intrinsic {tag} in condition value"
                ))),
            }
        }
        Value::Mapping(map) => {
            if let Some(name) = map.get(Value::String("Ref".into()))
                && let Some(name) = name.as_str()
            {
                return ctx
                    .params
                    .get(name)
                    .cloned()
                    .ok_or_else(|| CfnError::ParameterNotFound(name.to_string()));
            }
            if let Some(seq) = map.get(Value::String("Fn::FindInMap".into()))
                && let Some(seq) = seq.as_sequence()
                && seq.len() == 3
            {
                let map_name = resolve_condition_value(&seq[0], ctx)?;
                let first_key = resolve_condition_value(&seq[1], ctx)?;
                let second_key = resolve_condition_value(&seq[2], ctx)?;
                return ctx
                    .mappings
                    .get(&map_name)
                    .and_then(|m| m.get(&first_key))
                    .and_then(|m| m.get(&second_key))
                    .cloned()
                    .ok_or_else(|| CfnError::MappingNotFound {
                        map_name: map_name.clone(),
                        first_key: first_key.clone(),
                        second_key: second_key.clone(),
                    });
            }
            Err(CfnError::IntrinsicError(
                "unsupported mapping form in condition value".into(),
            ))
        }
        _ => Err(CfnError::IntrinsicError(
            "unsupported value type in condition".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Integration (wiring): a file exceeding `MAX_IAC_FILE_BYTES` is rejected
    /// by the parser's read path (`read_to_string_capped`). Ignored by default
    /// because it writes a >16 MiB temp file.
    #[test]
    #[ignore = "writes a >16 MiB temp file; run with `cargo test -- --ignored`"]
    fn parse_template_rejects_oversized_file() {
        use yevice_core::io::MAX_IAC_FILE_BYTES;
        let path =
            std::env::temp_dir().join(format!("yevice_cfn_oversized_{}.yaml", std::process::id()));
        std::fs::write(&path, vec![b' '; (MAX_IAC_FILE_BYTES + 1) as usize]).unwrap();
        let result = parse_template(&path);
        let _ = std::fs::remove_file(&path);
        assert!(result.is_err(), "oversized template file must be rejected");
    }

    const SAMPLE_TEMPLATE: &str = r#"
AWSTemplateFormatVersion: "2010-09-09"
Parameters:
  Env:
    Type: String
    Default: dev
  InstanceType:
    Type: String
    Default: t3.micro
Mappings:
  RegionMap:
    ap-northeast-1:
      AMI: ami-12345
Conditions:
  IsProd: !Equals
    - !Ref Env
    - prod
Resources:
  MyInstance:
    Type: AWS::EC2::Instance
    Properties:
      InstanceType: !Ref InstanceType
      ImageId: !FindInMap [RegionMap, ap-northeast-1, AMI]
  ProdOnlyBucket:
    Type: AWS::S3::Bucket
    Condition: IsProd
    Properties:
      BucketName: !Sub "${Env}-data-bucket"
"#;

    #[test]
    fn test_parse_template() {
        let tmpl = parse_template_str(SAMPLE_TEMPLATE).unwrap();
        assert_eq!(tmpl.parameters.len(), 2);
        assert_eq!(tmpl.resources.len(), 2);
        assert!(tmpl.mappings.contains_key("RegionMap"));
    }

    #[test]
    fn test_resolve_with_defaults() {
        let tmpl = parse_template_str(SAMPLE_TEMPLATE).unwrap();
        let resources = resolve_template(&tmpl, &HashMap::new(), &HashMap::new()).unwrap();

        // IsProd is false (default Env=dev), so ProdOnlyBucket should be skipped
        assert!(!resources.contains_key("ProdOnlyBucket"));

        // MyInstance should have resolved InstanceType
        let instance = &resources["MyInstance"];
        let inst_type = instance.properties.get("InstanceType").unwrap();
        assert_eq!(
            inst_type,
            &ResolvedValue::Concrete(Value::String("t3.micro".into()))
        );
    }

    #[test]
    fn test_resolve_with_prod_params() {
        let tmpl = parse_template_str(SAMPLE_TEMPLATE).unwrap();
        let mut params = HashMap::new();
        params.insert("Env".to_string(), "prod".to_string());
        let resources = resolve_template(&tmpl, &params, &HashMap::new()).unwrap();

        // IsProd is true, so ProdOnlyBucket should be present
        assert!(resources.contains_key("ProdOnlyBucket"));
    }

    // -----------------------------------------------------------------------
    // Tests based on representative CloudFormation patterns
    // -----------------------------------------------------------------------

    /// SQS template: !Sub with multiple parameters, FIFO queues
    const SQS_TEMPLATE: &str = r#"
AWSTemplateFormatVersion: "2010-09-09"
Parameters:
  AppName:
    Type: String
  Stage:
    Type: String
    AllowedValues: ["dev", "stg", "prd"]
  Module:
    Type: String
Resources:
  OrderQueueFIFO:
    Type: AWS::SQS::Queue
    Properties:
      QueueName: !Sub ${AppName}-${Stage}-${Module}-order-queue.fifo
      MessageRetentionPeriod: 1209600
      FifoQueue: true
  OrderDeadLetterQueue:
    Type: AWS::SQS::Queue
    Properties:
      QueueName: !Sub ${AppName}-${Stage}-${Module}-order-dlq
      MessageRetentionPeriod: 1209600
"#;

    #[test]
    fn test_sqs_template_parse_and_resolve() {
        let tmpl = parse_template_str(SQS_TEMPLATE).unwrap();
        assert_eq!(tmpl.resources.len(), 2);

        let mut params = HashMap::new();
        params.insert("AppName".into(), "acme".into());
        params.insert("Stage".into(), "prd".into());
        params.insert("Module".into(), "base".into());

        let resources = resolve_template(&tmpl, &params, &HashMap::new()).unwrap();
        assert_eq!(resources.len(), 2);

        // Verify FIFO queue property is preserved
        let fifo = &resources["OrderQueueFIFO"];
        let fifo_val = fifo.properties.get("FifoQueue").unwrap();
        assert_eq!(fifo_val, &ResolvedValue::Concrete(Value::Bool(true)));
    }

    /// Kinesis with Mappings: environment-dependent `ShardCount` via !`FindInMap`
    const KINESIS_TEMPLATE: &str = r#"
AWSTemplateFormatVersion: "2010-09-09"
Parameters:
  Stage:
    Type: String
    AllowedValues: ["dev", "stg", "prd"]
Mappings:
  StageMap:
    dev:
      ShardCount: 1
    stg:
      ShardCount: 1
    prd:
      ShardCount: 2
Resources:
  DataStream:
    Type: AWS::Kinesis::Stream
    Properties:
      ShardCount: !FindInMap [StageMap, !Ref Stage, ShardCount]
      RetentionPeriodHours: 24
"#;

    #[test]
    fn test_kinesis_findinmap_resolves_number() {
        let tmpl = parse_template_str(KINESIS_TEMPLATE).unwrap();

        // prd -> ShardCount=2
        let mut params = HashMap::new();
        params.insert("Stage".into(), "prd".into());
        let resources = resolve_template(&tmpl, &params, &HashMap::new()).unwrap();
        let stream = &resources["DataStream"];
        let shard_count = stream.properties.get("ShardCount").unwrap();
        assert_eq!(
            shard_count,
            &ResolvedValue::Concrete(Value::String("2".into()))
        );

        // dev -> ShardCount=1
        params.insert("Stage".into(), "dev".into());
        let resources = resolve_template(&tmpl, &params, &HashMap::new()).unwrap();
        let stream = &resources["DataStream"];
        let shard_count = stream.properties.get("ShardCount").unwrap();
        assert_eq!(
            shard_count,
            &ResolvedValue::Concrete(Value::String("1".into()))
        );
    }

    /// `OpenSearch` Serverless with Condition using !`FindInMap` boolean
    const AOSS_TEMPLATE: &str = r#"
AWSTemplateFormatVersion: "2010-09-09"
Parameters:
  Stage:
    Type: String
Mappings:
  StageMap:
    dev:
      StandbyReplicas: ENABLED
      EnableIndexRetention: false
    prd:
      StandbyReplicas: ENABLED
      EnableIndexRetention: true
Conditions:
  EnableIndexRetention: !Equals
    - !FindInMap [StageMap, !Ref Stage, EnableIndexRetention]
    - "true"
Resources:
  Collection:
    Type: AWS::OpenSearchServerless::Collection
    Properties:
      Name: test-collection
      Type: TIMESERIES
  RetentionPolicy:
    Type: AWS::OpenSearchServerless::LifecyclePolicy
    Condition: EnableIndexRetention
    Properties:
      Name: test-retention
      Type: retention
"#;

    #[test]
    fn test_condition_with_findinmap_boolean() {
        let tmpl = parse_template_str(AOSS_TEMPLATE).unwrap();

        // dev: EnableIndexRetention=false -> RetentionPolicy should be skipped
        let mut params = HashMap::new();
        params.insert("Stage".into(), "dev".into());
        let resources = resolve_template(&tmpl, &params, &HashMap::new()).unwrap();
        assert!(resources.contains_key("Collection"));
        assert!(!resources.contains_key("RetentionPolicy"));

        // prd: EnableIndexRetention=true -> RetentionPolicy should be present
        params.insert("Stage".into(), "prd".into());
        let resources = resolve_template(&tmpl, &params, &HashMap::new()).unwrap();
        assert!(resources.contains_key("Collection"));
        assert!(resources.contains_key("RetentionPolicy"));
    }

    /// SAM template: `AWS::Serverless::Function` + `Fn::ImportValue` + !Sub nested
    const SAM_TEMPLATE: &str = r#"
AWSTemplateFormatVersion: "2010-09-09"
Transform: AWS::Serverless-2016-10-31
Parameters:
  AppName:
    Type: String
  Stage:
    Type: String
  Module:
    Type: String
Mappings:
  StageMap:
    dev:
      LogLevel: DEBUG
    prd:
      LogLevel: INFO
Resources:
  IngestFunction:
    Type: AWS::Serverless::Function
    Properties:
      FunctionName: !Sub ${AppName}-${Stage}-${Module}-ingest
      MemorySize: 256
      Timeout: 900
      Environment:
        Variables:
          LOG_LEVEL: !FindInMap [StageMap, !Ref Stage, LogLevel]
          STREAM_ARN:
            Fn::ImportValue: !Sub ${AppName}-${Stage}-${Module}-kds-StreamArn
  IngestLogGroup:
    Type: AWS::Logs::LogGroup
    Properties:
      LogGroupName: !Sub /${AppName}-${Stage}-${Module}/ingest/
      RetentionInDays: 7
  DeadLetterQueue:
    Type: AWS::SQS::Queue
    Properties:
      QueueName: !Sub ${AppName}-${Stage}-${Module}-dlq
      MessageRetentionPeriod: 1209600
"#;

    #[test]
    fn test_sam_template_with_imports() {
        let tmpl = parse_template_str(SAM_TEMPLATE).unwrap();
        assert_eq!(tmpl.resources.len(), 3);

        // Check resource types
        assert_eq!(
            tmpl.resources["IngestFunction"].resource_type,
            "AWS::Serverless::Function"
        );
        assert_eq!(
            tmpl.resources["IngestLogGroup"].resource_type,
            "AWS::Logs::LogGroup"
        );

        let mut params = HashMap::new();
        params.insert("AppName".into(), "acme".into());
        params.insert("Stage".into(), "prd".into());
        params.insert("Module".into(), "ingest".into());

        let mut imports = HashMap::new();
        imports.insert(
            "acme-prd-ingest-kds-StreamArn".into(),
            "arn:aws:kinesis:ap-northeast-1:123:stream/test".into(),
        );

        let resources = resolve_template(&tmpl, &params, &imports).unwrap();
        assert_eq!(resources.len(), 3);

        // Verify Fn::ImportValue with !Sub resolved
        let func = &resources["IngestFunction"];
        let env_vars = func
            .properties
            .get("Environment")
            .unwrap()
            .get("Variables")
            .unwrap();

        let stream_arn = env_vars.get("STREAM_ARN").unwrap();
        assert_eq!(
            stream_arn,
            &ResolvedValue::Concrete(Value::String(
                "arn:aws:kinesis:ap-northeast-1:123:stream/test".into()
            ))
        );

        // Verify FindInMap resolved
        let log_level = env_vars.get("LOG_LEVEL").unwrap();
        assert_eq!(
            log_level,
            &ResolvedValue::Concrete(Value::String("INFO".into()))
        );
    }

    /// `DynamoDB` with many tables (like base/dynamodb.yml with 18+ tables)
    const MULTI_DDB_TEMPLATE: &str = r#"
AWSTemplateFormatVersion: "2010-09-09"
Parameters:
  AppName:
    Type: String
    Default: acme
  Stage:
    Type: String
    Default: prd
Resources:
  TableA:
    Type: AWS::DynamoDB::Table
    Properties:
      TableName: !Sub "${AppName}-${Stage}-TableA"
      BillingMode: PAY_PER_REQUEST
      AttributeDefinitions:
        - AttributeName: pk
          AttributeType: S
      KeySchema:
        - AttributeName: pk
          KeyType: HASH
  TableB:
    Type: AWS::DynamoDB::Table
    Properties:
      TableName: !Sub "${AppName}-${Stage}-TableB"
      BillingMode: PAY_PER_REQUEST
      AttributeDefinitions:
        - AttributeName: pk
          AttributeType: S
        - AttributeName: sk
          AttributeType: S
      KeySchema:
        - AttributeName: pk
          KeyType: HASH
        - AttributeName: sk
          KeyType: RANGE
      GlobalSecondaryIndexes:
        - IndexName: GSI1
          KeySchema:
            - AttributeName: sk
              KeyType: HASH
          Projection:
            ProjectionType: ALL
      StreamSpecification:
        StreamViewType: NEW_AND_OLD_IMAGES
"#;

    #[test]
    fn test_multi_dynamodb_tables() {
        let tmpl = parse_template_str(MULTI_DDB_TEMPLATE).unwrap();
        let resources = resolve_template(&tmpl, &HashMap::new(), &HashMap::new()).unwrap();
        assert_eq!(resources.len(), 2);
        assert_eq!(resources["TableA"].resource_type, "AWS::DynamoDB::Table");
    }

    // -----------------------------------------------------------------------
    // Fix 1: Required-parameter validation
    // -----------------------------------------------------------------------

    /// A template with a parameter that has no Default and is not supplied
    /// must return MissingParameters containing the parameter name.
    #[test]
    fn test_resolve_missing_required_parameter_errors() {
        const TEMPLATE: &str = r#"
AWSTemplateFormatVersion: "2010-09-09"
Parameters:
  Env:
    Type: String
  InstanceTypeParam:
    Type: String
Resources:
  MyInstance:
    Type: AWS::EC2::Instance
    Properties:
      InstanceType: !Ref InstanceTypeParam
"#;
        let tmpl = parse_template_str(TEMPLATE).unwrap();
        let result = resolve_template(&tmpl, &HashMap::new(), &HashMap::new());
        let err = result.expect_err("expected Err for missing required parameters");
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("Env"),
            "error message must contain parameter name 'Env': {err_msg}"
        );
        assert!(
            err_msg.contains("InstanceTypeParam"),
            "error message must contain parameter name 'InstanceTypeParam': {err_msg}"
        );
    }

    /// A template with a parameter that has no Default but IS supplied must succeed.
    #[test]
    fn test_resolve_supplied_required_parameter_succeeds() {
        const TEMPLATE: &str = r#"
AWSTemplateFormatVersion: "2010-09-09"
Parameters:
  Env:
    Type: String
Resources:
  MyBucket:
    Type: AWS::S3::Bucket
    Properties:
      BucketName: !Sub "${Env}-bucket"
"#;
        let tmpl = parse_template_str(TEMPLATE).unwrap();
        let mut params = HashMap::new();
        params.insert("Env".to_string(), "prod".to_string());
        let result = resolve_template(&tmpl, &params, &HashMap::new());
        assert!(
            result.is_ok(),
            "expected Ok when required parameter is supplied"
        );
    }

    // -----------------------------------------------------------------------
    // ADR-0003 / Issue #38 — ParsePolicy
    // -----------------------------------------------------------------------

    /// Under Lenient the missing-parameter case becomes a diagnostic and the
    /// resolved value carries a `had_errors=true` ParseOutcome instead of an
    /// Err. The offending resource (which depends on the unbound `!Ref`) is
    /// best-effort skipped, but the rest of the template still resolves.
    #[test]
    fn lenient_demotes_missing_parameter_to_diagnostic() {
        const TEMPLATE: &str = r#"
AWSTemplateFormatVersion: "2010-09-09"
Parameters:
  Env:
    Type: String
Resources:
  MyBucket:
    Type: AWS::S3::Bucket
    Properties:
      BucketName: !Sub "${Env}-bucket"
"#;
        let tmpl = parse_template_str(TEMPLATE).unwrap();
        let outcome = resolve_template_with_policy(
            &tmpl,
            &HashMap::new(),
            &HashMap::new(),
            ParsePolicy::Lenient,
            None,
        )
        .expect("lenient must not hard-error on missing parameter");
        assert!(
            outcome.had_errors,
            "missing parameter must surface had_errors=true under Lenient"
        );
        assert!(
            outcome
                .diagnostics
                .iter()
                .any(|d| d.code == "missing_parameter"),
            "diagnostics should include 'missing_parameter'; got {:?}",
            outcome.diagnostics
        );
    }

    /// Under Strict the same template must surface a hard CfnError.
    #[test]
    fn strict_preserves_missing_parameter_hard_error() {
        const TEMPLATE: &str = r#"
AWSTemplateFormatVersion: "2010-09-09"
Parameters:
  Env:
    Type: String
Resources:
  MyBucket:
    Type: AWS::S3::Bucket
    Properties:
      BucketName: !Sub "${Env}-bucket"
"#;
        let tmpl = parse_template_str(TEMPLATE).unwrap();
        let err = resolve_template_with_policy(
            &tmpl,
            &HashMap::new(),
            &HashMap::new(),
            ParsePolicy::Strict,
            None,
        )
        .expect_err("strict must error on missing parameter");
        assert!(
            matches!(err, CfnError::MissingParameters(_)),
            "expected MissingParameters; got {err:?}"
        );
    }

    /// `Fn::ImportValue` against an unknown export becomes a diagnostic
    /// under Lenient (instead of `CfnError::ImportValueNotFound`).
    #[test]
    fn lenient_demotes_import_value_not_found() {
        const TEMPLATE: &str = r#"
AWSTemplateFormatVersion: "2010-09-09"
Resources:
  Func:
    Type: AWS::Lambda::Function
    Properties:
      FunctionName: example
      Environment:
        Variables:
          ARN:
            Fn::ImportValue: missing-export
"#;
        let tmpl = parse_template_str(TEMPLATE).unwrap();
        let outcome = resolve_template_with_policy(
            &tmpl,
            &HashMap::new(),
            &HashMap::new(),
            ParsePolicy::Lenient,
            None,
        )
        .expect("lenient must demote ImportValueNotFound to diagnostic");
        assert!(outcome.had_errors);
        assert!(
            outcome
                .diagnostics
                .iter()
                .any(|d| d.code == "import_value_not_found"),
            "expected import_value_not_found diagnostic; got {:?}",
            outcome.diagnostics
        );
    }

    /// Strict preserves the historical `CfnError::ImportValueNotFound` on the
    /// same template.
    #[test]
    fn strict_preserves_import_value_not_found() {
        const TEMPLATE: &str = r#"
AWSTemplateFormatVersion: "2010-09-09"
Resources:
  Func:
    Type: AWS::Lambda::Function
    Properties:
      FunctionName: example
      Environment:
        Variables:
          ARN:
            Fn::ImportValue: missing-export
"#;
        let tmpl = parse_template_str(TEMPLATE).unwrap();
        let err = resolve_template_with_policy(
            &tmpl,
            &HashMap::new(),
            &HashMap::new(),
            ParsePolicy::Strict,
            None,
        )
        .expect_err("strict must error");
        assert!(matches!(err, CfnError::ImportValueNotFound(_)));
    }
}
