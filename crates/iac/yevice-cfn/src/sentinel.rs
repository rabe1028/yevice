//! Sentinel strings for `CloudFormation` intrinsic references.
//!
//! The intrinsic resolver in `intrinsic.rs` cannot fully evaluate `!Ref` or
//! `!GetAtt` for resource logical IDs at parse time, so it encodes them as
//! opaque sentinel strings that are later decoded by the connection builder in
//! `convert.rs`.  This module is the single source of truth for the wire
//! format of those sentinels, keeping all format knowledge in one place.

const REF_PREFIX: &str = "{{ref:";
const GETATT_PREFIX: &str = "{{getatt:";
const SUFFIX: &str = "}}";

/// A resolved `CloudFormation` intrinsic reference to another resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CfnRef {
    /// The logical ID of the referenced resource.
    pub logical_id: String,
    /// `Some(attr)` for `!GetAtt`, `None` for `!Ref`.
    pub attr: Option<String>,
}

/// Encode a `!Ref` as a sentinel string: `"{{ref:X}}"`.
pub(crate) fn make_ref(logical_id: &str) -> String {
    format!("{REF_PREFIX}{logical_id}{SUFFIX}")
}

/// Encode a `!GetAtt` as a sentinel string: `"{{getatt:X.Attr}}"`.
pub(crate) fn make_getatt(logical_id: &str, attr: &str) -> String {
    format!("{GETATT_PREFIX}{logical_id}.{attr}{SUFFIX}")
}

/// Parse a sentinel string into a typed [`CfnRef`].
///
/// Returns `None` for any non-sentinel string (literals, ARNs, etc.).
///
/// - `"{{ref:X}}"` → `CfnRef { logical_id: "X", attr: None }`
/// - `"{{getatt:X.Attr}}"` → `CfnRef { logical_id: "X", attr: Some("Attr") }`
/// - `"{{getatt:X.Y.Z}}"` → `CfnRef { logical_id: "X", attr: Some("Y.Z") }` (split on first `.`)
pub(crate) fn parse(s: &str) -> Option<CfnRef> {
    if let Some(inner) = s
        .strip_prefix(REF_PREFIX)
        .and_then(|r| r.strip_suffix(SUFFIX))
    {
        return Some(CfnRef {
            logical_id: inner.to_string(),
            attr: None,
        });
    }
    if let Some(inner) = s
        .strip_prefix(GETATT_PREFIX)
        .and_then(|r| r.strip_suffix(SUFFIX))
    {
        let (logical_id, attr) = inner.split_once('.')?;
        return Some(CfnRef {
            logical_id: logical_id.to_string(),
            attr: Some(attr.to_string()),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn make_ref_produces_correct_sentinel() {
        assert_eq!(make_ref("MyQueue"), "{{ref:MyQueue}}");
        assert_eq!(make_ref("SomeTable"), "{{ref:SomeTable}}");
    }

    #[test]
    fn make_getatt_produces_correct_sentinel() {
        assert_eq!(make_getatt("MyQueue", "Arn"), "{{getatt:MyQueue.Arn}}");
        assert_eq!(
            make_getatt("MyBucket", "WebsiteURL"),
            "{{getatt:MyBucket.WebsiteURL}}"
        );
    }

    #[test]
    fn parse_ref_round_trip() {
        let sentinel = make_ref("MyQueue");
        let result = parse(&sentinel).expect("should parse");
        assert_eq!(result.logical_id, "MyQueue");
        assert_eq!(result.attr, None);
    }

    #[test]
    fn parse_getatt_round_trip() {
        let sentinel = make_getatt("MyQueue", "Arn");
        let result = parse(&sentinel).expect("should parse");
        assert_eq!(result.logical_id, "MyQueue");
        assert_eq!(result.attr, Some("Arn".to_string()));
    }

    #[test]
    fn parse_getatt_deep_attr_splits_on_first_dot() {
        // "{{getatt:X.Y.Z}}" → logical_id="X", attr="Y.Z"
        let sentinel = "{{getatt:MyResource.SomeNested.Attr}}";
        let result = parse(sentinel).expect("should parse");
        assert_eq!(result.logical_id, "MyResource");
        assert_eq!(result.attr, Some("SomeNested.Attr".to_string()));
    }

    #[test]
    fn parse_arn_returns_none() {
        assert!(parse("arn:aws:sqs:us-east-1:123456789012:MyQueue").is_none());
    }

    #[test]
    fn parse_literal_returns_none() {
        assert!(parse("some-literal-value").is_none());
        assert!(parse("").is_none());
    }

    #[test]
    fn parse_partial_sentinel_returns_none() {
        // Missing suffix
        assert!(parse("{{ref:MyQueue").is_none());
        // Missing prefix
        assert!(parse("MyQueue}}").is_none());
        // getatt without dot — no logical_id/attr split possible
        assert!(parse("{{getatt:MyQueueNoAttr}}").is_none());
    }
}
