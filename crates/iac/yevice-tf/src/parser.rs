use std::{collections::HashMap, path::Path};

use hcl::{Body, Structure};

use crate::error::TfError;

#[derive(Debug, Clone, PartialEq)]
pub enum TfValue {
    String(String),
    Number(f64),
    Bool(bool),
    VarRef(String),
    LocalRef(String),
    Unknown,
}

impl TfValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value.as_str()),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Number(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub fn is_concrete(&self) -> bool {
        matches!(self, Self::String(_) | Self::Number(_) | Self::Bool(_))
    }
}

#[derive(Debug, Clone)]
pub struct TfResource {
    pub resource_type: String,
    pub name: String,
    pub attrs: HashMap<String, TfValue>,
    pub blocks: HashMap<String, Vec<HashMap<String, TfValue>>>,
}

#[derive(Debug, Clone)]
pub struct TfVariable {
    pub default: Option<TfValue>,
}

#[derive(Debug, Default)]
pub struct TfConfig {
    pub variables: HashMap<String, TfVariable>,
    pub locals: HashMap<String, TfValue>,
    pub resources: Vec<TfResource>,
}

pub fn parse_tf_dir(dir: &Path) -> Result<TfConfig, TfError> {
    if !dir.is_dir() {
        return Err(TfError::Io(std::io::Error::new(
            std::io::ErrorKind::NotADirectory,
            format!("not a directory: {}", dir.display()),
        )));
    }

    let mut config = TfConfig::default();

    let mut tf_files: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "tf"))
        .collect();
    tf_files.sort();

    for path in tf_files {
        let content = std::fs::read_to_string(&path)?;
        let body: Body = hcl::parse(&content)?;
        parse_body_into(&body, &mut config);
    }

    Ok(config)
}

pub fn parse_tfvars(path: &Path) -> Result<HashMap<String, TfValue>, TfError> {
    let content = std::fs::read_to_string(path)?;
    let body: Body = hcl::parse(&content)?;
    let mut vars = HashMap::new();

    for structure in &body {
        if let Structure::Attribute(attr) = structure {
            vars.insert(attr.key().to_string(), expr_to_tf_value(attr.expr()));
        }
    }

    Ok(vars)
}

fn parse_body_into(body: &Body, config: &mut TfConfig) {
    for structure in body {
        let Structure::Block(block) = structure else {
            continue;
        };

        match block.identifier() {
            "resource" => {
                let labels: Vec<&str> =
                    block.labels().iter().map(hcl::BlockLabel::as_str).collect();
                if let [resource_type, name, ..] = labels.as_slice() {
                    config
                        .resources
                        .push(parse_resource_block(resource_type, name, block.body()));
                }
            }
            "variable" => {
                if let Some(name) = block.labels().first() {
                    config.variables.insert(
                        name.as_str().to_string(),
                        parse_variable_block(block.body()),
                    );
                }
            }
            "locals" => {
                for structure in block.body() {
                    if let Structure::Attribute(attr) = structure {
                        config
                            .locals
                            .insert(attr.key().to_string(), expr_to_tf_value(attr.expr()));
                    }
                }
            }
            _ => {}
        }
    }
}

fn parse_resource_block(resource_type: &str, name: &str, body: &Body) -> TfResource {
    let mut attrs = HashMap::new();
    let mut blocks = HashMap::new();

    for structure in body {
        match structure {
            Structure::Attribute(attr) => {
                attrs.insert(attr.key().to_string(), expr_to_tf_value(attr.expr()));
            }
            Structure::Block(block) => {
                let mut block_attrs = HashMap::new();
                // Capture this block's direct attributes plus those of any
                // nested blocks (e.g. `template { scaling { min_instance_count }}`
                // or `template { containers { resources { limits { cpu }}}}`),
                // merged into one map. The single-level block API can then reach
                // them; a parent attribute always wins over a nested one.
                collect_block_attrs(block.body(), &mut block_attrs);
                blocks
                    .entry(block.identifier().to_string())
                    .or_insert_with(Vec::new)
                    .push(block_attrs);
            }
        }
    }

    TfResource {
        resource_type: resource_type.to_string(),
        name: name.to_string(),
        attrs,
        blocks,
    }
}

/// Recursively merge a block body's attributes (and those of nested blocks)
/// into `out`. Attributes already present (closer to the parent) take
/// precedence over more deeply nested ones with the same key.
fn collect_block_attrs(body: &Body, out: &mut HashMap<String, TfValue>) {
    for structure in body {
        match structure {
            Structure::Attribute(attr) => {
                out.entry(attr.key().to_string())
                    .or_insert_with(|| expr_to_tf_value(attr.expr()));
            }
            Structure::Block(block) => collect_block_attrs(block.body(), out),
        }
    }
}

fn parse_variable_block(body: &Body) -> TfVariable {
    let default = body.iter().find_map(|structure| match structure {
        Structure::Attribute(attr) if attr.key() == "default" => {
            Some(expr_to_tf_value(attr.expr()))
        }
        _ => None,
    });

    TfVariable { default }
}

pub fn expr_to_tf_value(expr: &hcl::expr::Expression) -> TfValue {
    use hcl::expr::{Expression, TraversalOperator};

    match expr {
        Expression::String(value) => TfValue::String(value.clone()),
        Expression::Number(value) => value.as_f64().map_or(TfValue::Unknown, TfValue::Number),
        Expression::Bool(value) => TfValue::Bool(*value),
        Expression::Traversal(traversal) => {
            if let Expression::Variable(variable) = &traversal.expr
                && let Some(TraversalOperator::GetAttr(attr)) = traversal.operators.first()
            {
                return match variable.as_ref() {
                    "var" => TfValue::VarRef(attr.as_ref().to_string()),
                    "local" => TfValue::LocalRef(attr.as_ref().to_string()),
                    _ => TfValue::Unknown,
                };
            }
            TfValue::Unknown
        }
        _ => TfValue::Unknown,
    }
}
