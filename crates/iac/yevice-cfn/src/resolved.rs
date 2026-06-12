//! Typed representation of resolved `CloudFormation` property values.
//!
//! The intrinsic resolver in `intrinsic.rs` cannot fully evaluate `!Ref` or
//! `!GetAtt` for resource logical IDs at parse time. Instead of encoding them
//! as opaque sentinel strings (the previous design, which repeatedly produced
//! parsing bugs), unresolved references are kept as typed variants of
//! [`ResolvedValue`] until the adapter boundary in `convert.rs`.

use std::collections::BTreeMap;

use serde_yaml_ng::Value;

/// A property value after intrinsic resolution.
///
/// Containers are represented structurally (`Seq` / `Map`) so that reference
/// extraction can walk the tree without re-parsing strings. `Concrete` holds
/// leaf YAML values (scalars, plus rare pass-through values such as
/// unresolvable tagged intrinsics) that contain no resource references.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedValue {
    /// A statically-resolved value containing no resource references.
    Concrete(Value),
    /// An unresolved `!Ref LogicalId` to a resource in the same template.
    Ref(String),
    /// An unresolved `!GetAtt LogicalId.Attr`.
    GetAtt { logical_id: String, attr: String },
    /// A string produced by `Fn::Sub` / `Fn::Join` in which one or more
    /// resource references are interleaved with literal text.
    Interpolated(Vec<StringPart>),
    /// A sequence whose elements may contain references.
    Seq(Vec<ResolvedValue>),
    /// A mapping whose values may contain references.
    Map(BTreeMap<String, ResolvedValue>),
}

/// One segment of an interpolated string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StringPart {
    /// Literal text (including pseudo-parameter placeholders left verbatim).
    Literal(String),
    /// A `${LogicalId}` resource reference.
    Ref(String),
    /// A `${LogicalId.Attr}` resource attribute reference.
    GetAtt { logical_id: String, attr: String },
}

/// A reference to another resource extracted from a [`ResolvedValue`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    /// The logical ID of the referenced resource.
    pub logical_id: String,
    /// `Some(attr)` for `!GetAtt`, `None` for `!Ref`.
    pub attr: Option<String>,
}

impl ResolvedValue {
    /// Build a string-valued `ResolvedValue` from interpolation parts,
    /// normalizing degenerate shapes:
    ///
    /// - adjacent literals are merged
    /// - all-literal parts collapse to `Concrete(String)`
    /// - a single bare `Ref` / `GetAtt` part is promoted to the corresponding
    ///   typed variant
    pub fn from_parts(parts: Vec<StringPart>) -> Self {
        // Merge adjacent literals.
        let mut merged: Vec<StringPart> = Vec::with_capacity(parts.len());
        for part in parts {
            match (merged.last_mut(), part) {
                (Some(StringPart::Literal(prev)), StringPart::Literal(next)) => {
                    prev.push_str(&next);
                }
                (_, part) => merged.push(part),
            }
        }

        if merged.iter().all(|p| matches!(p, StringPart::Literal(_))) {
            let joined: String = merged
                .into_iter()
                .map(|p| match p {
                    StringPart::Literal(s) => s,
                    _ => unreachable!(),
                })
                .collect();
            return Self::Concrete(Value::String(joined));
        }

        if merged.len() == 1 {
            return match merged.into_iter().next().expect("len checked") {
                StringPart::Ref(id) => Self::Ref(id),
                StringPart::GetAtt { logical_id, attr } => Self::GetAtt { logical_id, attr },
                StringPart::Literal(_) => unreachable!("all-literal handled above"),
            };
        }

        Self::Interpolated(merged)
    }

    /// Returns the string slice for `Concrete(Value::String)` values.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Concrete(Value::String(s)) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Map lookup by string key (`Map` variant only).
    pub fn get(&self, key: &str) -> Option<&ResolvedValue> {
        match self {
            Self::Map(map) => map.get(key),
            _ => None,
        }
    }

    /// Returns the elements for the `Seq` variant.
    pub fn as_seq(&self) -> Option<&[ResolvedValue]> {
        match self {
            Self::Seq(items) => Some(items.as_slice()),
            _ => None,
        }
    }

    /// Converts a sequence-shaped value into owned `ResolvedValue` elements.
    ///
    /// Handles both the structural `Seq` variant and a `Concrete` YAML
    /// sequence (which can occur via pass-through paths).
    pub fn into_seq(self) -> Option<Vec<ResolvedValue>> {
        match self {
            Self::Seq(items) => Some(items),
            Self::Concrete(Value::Sequence(items)) => {
                Some(items.into_iter().map(ResolvedValue::Concrete).collect())
            }
            _ => None,
        }
    }

    /// All resource references carried *directly* by this value:
    /// `Ref` / `GetAtt` yield one reference; `Interpolated` yields every
    /// non-literal part in order; everything else yields none.
    ///
    /// This intentionally does not recurse into `Seq` / `Map` — connection
    /// extraction walks containers explicitly with type-specific semantics.
    pub fn references(&self) -> Vec<Reference> {
        match self {
            Self::Ref(id) => vec![Reference {
                logical_id: id.clone(),
                attr: None,
            }],
            Self::GetAtt { logical_id, attr } => vec![Reference {
                logical_id: logical_id.clone(),
                attr: Some(attr.clone()),
            }],
            Self::Interpolated(parts) => parts
                .iter()
                .filter_map(|p| match p {
                    StringPart::Literal(_) => None,
                    StringPart::Ref(id) => Some(Reference {
                        logical_id: id.clone(),
                        attr: None,
                    }),
                    StringPart::GetAtt { logical_id, attr } => Some(Reference {
                        logical_id: logical_id.clone(),
                        attr: Some(attr.clone()),
                    }),
                })
                .collect(),
            _ => Vec::new(),
        }
    }
}

/// Render interpolation parts to a `CloudFormation`-native string, using the
/// `${LogicalId}` / `${LogicalId.Attr}` substitution syntax for references.
pub fn render_parts(parts: &[StringPart]) -> String {
    let mut out = String::new();
    for part in parts {
        match part {
            StringPart::Literal(s) => out.push_str(s),
            StringPart::Ref(id) => {
                out.push_str("${");
                out.push_str(id);
                out.push('}');
            }
            StringPart::GetAtt { logical_id, attr } => {
                out.push_str("${");
                out.push_str(logical_id);
                out.push('.');
                out.push_str(attr);
                out.push('}');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(s: &str) -> StringPart {
        StringPart::Literal(s.to_string())
    }

    #[test]
    fn from_parts_all_literals_collapse_to_concrete() {
        let v = ResolvedValue::from_parts(vec![lit("prefix-"), lit("-suffix")]);
        assert_eq!(
            v,
            ResolvedValue::Concrete(Value::String("prefix--suffix".into()))
        );
    }

    #[test]
    fn from_parts_empty_is_empty_string() {
        let v = ResolvedValue::from_parts(vec![]);
        assert_eq!(v, ResolvedValue::Concrete(Value::String(String::new())));
    }

    #[test]
    fn from_parts_single_ref_promotes() {
        let v = ResolvedValue::from_parts(vec![StringPart::Ref("MyQueue".into())]);
        assert_eq!(v, ResolvedValue::Ref("MyQueue".into()));
    }

    #[test]
    fn from_parts_single_getatt_promotes() {
        let v = ResolvedValue::from_parts(vec![StringPart::GetAtt {
            logical_id: "MyFn".into(),
            attr: "Arn".into(),
        }]);
        assert_eq!(
            v,
            ResolvedValue::GetAtt {
                logical_id: "MyFn".into(),
                attr: "Arn".into()
            }
        );
    }

    #[test]
    fn from_parts_mixed_stays_interpolated() {
        let v = ResolvedValue::from_parts(vec![
            lit("arn:"),
            StringPart::Ref("MyQueue".into()),
            lit("/suffix"),
        ]);
        match &v {
            ResolvedValue::Interpolated(parts) => assert_eq!(parts.len(), 3),
            other => panic!("expected Interpolated, got {other:?}"),
        }
    }

    #[test]
    fn references_returns_all_interpolated_parts() {
        let v = ResolvedValue::Interpolated(vec![
            StringPart::GetAtt {
                logical_id: "FnA".into(),
                attr: "Arn".into(),
            },
            lit(":"),
            StringPart::GetAtt {
                logical_id: "FnB".into(),
                attr: "Arn".into(),
            },
        ]);
        let refs = v.references();
        assert_eq!(refs.len(), 2, "both references must be extracted");
        assert_eq!(refs[0].logical_id, "FnA");
        assert_eq!(refs[1].logical_id, "FnB");
    }

    #[test]
    fn references_empty_for_concrete_and_containers() {
        assert!(
            ResolvedValue::Concrete(Value::String("x".into()))
                .references()
                .is_empty()
        );
        assert!(
            ResolvedValue::Seq(vec![ResolvedValue::Ref("A".into())])
                .references()
                .is_empty()
        );
    }

    #[test]
    fn render_parts_uses_cfn_native_syntax() {
        let parts = vec![
            lit("arn:aws:lambda:${AWS::Region}:fn:"),
            StringPart::Ref("MyFn".into()),
            lit(":"),
            StringPart::GetAtt {
                logical_id: "Other".into(),
                attr: "Arn".into(),
            },
        ];
        assert_eq!(
            render_parts(&parts),
            "arn:aws:lambda:${AWS::Region}:fn:${MyFn}:${Other.Arn}"
        );
    }
}
