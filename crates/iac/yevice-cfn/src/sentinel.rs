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
        // Reject concatenated sentinels like "{{ref:A}}{{ref:B}}":
        // strip_suffix finds the last "}}", leaving "A}}{{ref:B" in inner.
        if inner.contains(SUFFIX) {
            return None;
        }
        return Some(CfnRef {
            logical_id: inner.to_string(),
            attr: None,
        });
    }
    if let Some(inner) = s
        .strip_prefix(GETATT_PREFIX)
        .and_then(|r| r.strip_suffix(SUFFIX))
    {
        if inner.contains(SUFFIX) {
            return None;
        }
        let (logical_id, attr) = inner.split_once('.')?;
        return Some(CfnRef {
            logical_id: logical_id.to_string(),
            attr: Some(attr.to_string()),
        });
    }
    None
}

/// Returns `true` if `s` is a *concatenation* of two or more sentinels:
/// starts with a sentinel prefix, and after the first closing `}}` the
/// remaining text immediately opens another sentinel.
///
/// ```text
/// "{{ref:A}}{{ref:B}}"         → true  (concatenation — no single target)
/// "{{getatt:Fn.Arn}}:live"     → false (sentinel + literal suffix — single target)
/// "arn:...{{ref:X}}"           → false (literal prefix — single embedded target)
/// ```
fn is_sentinel_concatenation(s: &str) -> bool {
    if !s.starts_with(REF_PREFIX) && !s.starts_with(GETATT_PREFIX) {
        return false;
    }
    // Find the first closing "}}" and inspect what follows.
    s.find(SUFFIX).is_some_and(|end| {
        let after = &s[end + SUFFIX.len()..];
        after.starts_with(REF_PREFIX) || after.starts_with(GETATT_PREFIX)
    })
}

/// Try to parse `s` as a whole-string sentinel; if that fails, search for an
/// embedded sentinel — except when `s` is a *concatenation* of sentinels.
///
/// The concatenation guard prevents extracting a partial match from strings
/// like `{{ref:A}}{{ref:B}}` where neither sub-sentinel is the definitive
/// target.  A single sentinel with a trailing literal (e.g.
/// `{{getatt:Fn.Arn}}:live` from `Fn::Sub`) is **not** a concatenation, so
/// `find_embedded` still runs and recovers the reference correctly.
pub(crate) fn parse_or_find_embedded(s: &str) -> Option<CfnRef> {
    parse(s).or_else(|| {
        if is_sentinel_concatenation(s) {
            None
        } else {
            find_embedded(s)
        }
    })
}

/// Search for the **first** embedded sentinel (`{{ref:...}}` or
/// `{{getatt:...}}`) within a larger string and parse it into a [`CfnRef`].
///
/// This handles values like
/// `"arn:aws:lambda:${AWS::Region}:${AWS::AccountId}:function:{{ref:HandlerFunction}}"`
/// that result from resolving a `Fn::Sub` containing both pseudo-parameters
/// (left verbatim) and resource logical IDs (encoded as sentinels).
///
/// Returns `None` when no recognisable sentinel prefix is found in the string.
/// When multiple sentinels are embedded, only the first one is returned
/// (pseudo-parameter placeholders such as `${AWS::Region}` are never encoded
/// as sentinels and therefore do not interfere).
pub(crate) fn find_embedded(s: &str) -> Option<CfnRef> {
    // Find the earliest occurrence of either sentinel prefix.
    let ref_pos = s.find(REF_PREFIX);
    let getatt_pos = s.find(GETATT_PREFIX);

    // Pick the prefix that starts earliest (or the one that exists if only one does).
    let start = match (ref_pos, getatt_pos) {
        (Some(r), Some(g)) => r.min(g),
        (Some(r), None) => r,
        (None, Some(g)) => g,
        (None, None) => return None,
    };

    // Find the closing `}}` after the start position.
    let end = s[start..].find(SUFFIX)?;
    // The sentinel substring (including prefix and suffix).
    let sentinel = &s[start..start + end + SUFFIX.len()];

    parse(sentinel)
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

    #[test]
    fn parse_concatenated_sentinels_returns_none() {
        // "{{ref:A}}{{ref:B}}" must NOT parse as ResourceRef("A}}{{ref:B").
        assert!(parse("{{ref:Prefix}}{{ref:Suffix}}").is_none());
        // Same for getatt.
        assert!(parse("{{getatt:A.Attr}}{{ref:B}}").is_none());
    }

    // -------------------------------------------------------------------------
    // find_embedded
    // -------------------------------------------------------------------------

    #[test]
    fn find_embedded_bare_sentinel_still_works() {
        // A string that IS a whole sentinel should also be found by find_embedded.
        let s = "{{ref:MyQueue}}";
        let result = find_embedded(s).expect("should find embedded sentinel");
        assert_eq!(result.logical_id, "MyQueue");
        assert_eq!(result.attr, None);
    }

    #[test]
    fn find_embedded_ref_in_arn_sub() {
        // Simulates Fn::Sub ARN resolve with embedded ref sentinel.
        let s = "arn:aws:lambda:us-east-1:123456789012:function:{{ref:HandlerFunction}}";
        let result = find_embedded(s).expect("should find embedded ref");
        assert_eq!(result.logical_id, "HandlerFunction");
        assert_eq!(result.attr, None);
    }

    #[test]
    fn find_embedded_getatt_in_arn_sub() {
        // Simulates Fn::Sub ARN resolve with embedded getatt sentinel.
        let s = "arn:aws:lambda:us-east-1:123456789012:function:{{getatt:MyFunction.Arn}}";
        let result = find_embedded(s).expect("should find embedded getatt");
        assert_eq!(result.logical_id, "MyFunction");
        assert_eq!(result.attr, Some("Arn".to_string()));
    }

    #[test]
    fn find_embedded_prefers_earliest_sentinel() {
        // Two sentinels embedded — only the first one is returned.
        let s = "prefix-{{ref:FirstResource}}-middle-{{ref:SecondResource}}-suffix";
        let result = find_embedded(s).expect("should find first embedded sentinel");
        assert_eq!(result.logical_id, "FirstResource");
    }

    #[test]
    fn find_embedded_no_sentinel_returns_none() {
        assert!(find_embedded("arn:aws:sqs:us-east-1:123456789012:MyQueue").is_none());
        assert!(find_embedded("some-literal-string").is_none());
        assert!(find_embedded("").is_none());
    }

    #[test]
    fn find_embedded_pseudo_param_suffix_not_confused_with_sentinel() {
        // ${AWS::Region} is NOT encoded as a sentinel — it stays verbatim.
        // Confirm that a string with only pseudo-param placeholders yields None.
        let s = "arn:aws:lambda:${AWS::Region}:${AWS::AccountId}:function:MyFunction";
        assert!(find_embedded(s).is_none());
    }

    #[test]
    fn find_embedded_pseudo_params_plus_sentinel_extracts_sentinel() {
        // Common pattern after Fn::Sub resolution: pseudo-params stay verbatim,
        // resource references become sentinels.
        let s = "arn:aws:lambda:${AWS::Region}:${AWS::AccountId}:function:{{ref:HandlerFunction}}";
        let result = find_embedded(s).expect("should find sentinel amid pseudo-params");
        assert_eq!(result.logical_id, "HandlerFunction");
        assert_eq!(result.attr, None);
    }

    // -------------------------------------------------------------------------
    // parse_or_find_embedded
    // -------------------------------------------------------------------------

    #[test]
    fn parse_or_find_embedded_whole_string_sentinel() {
        let result = parse_or_find_embedded("{{ref:MyQueue}}").expect("should match");
        assert_eq!(result.logical_id, "MyQueue");
    }

    #[test]
    fn parse_or_find_embedded_embedded_in_arn() {
        let s = "arn:aws:lambda:us-east-1:123:function:{{ref:MyFn}}";
        let result = parse_or_find_embedded(s).expect("should find embedded");
        assert_eq!(result.logical_id, "MyFn");
    }

    #[test]
    fn parse_or_find_embedded_rejects_concatenated_sentinels() {
        // A concatenation must NOT produce a spurious edge to the first resource.
        assert!(parse_or_find_embedded("{{ref:A}}{{ref:B}}").is_none());
        assert!(parse_or_find_embedded("{{getatt:A.Attr}}{{ref:B}}").is_none());
    }

    #[test]
    fn parse_or_find_embedded_sentinel_with_literal_suffix() {
        // Fn::Sub can produce "{{getatt:Fn.Arn}}:live" — a single sentinel
        // followed by literal text.  This is NOT a concatenation; the sentinel
        // reference should still be extracted.
        let result =
            parse_or_find_embedded("{{getatt:MyFunction.Arn}}:live").expect("should find ref");
        assert_eq!(result.logical_id, "MyFunction");
        assert_eq!(result.attr, Some("Arn".to_string()));
    }

    #[test]
    fn is_sentinel_concatenation_distinguishes_correctly() {
        assert!(is_sentinel_concatenation("{{ref:A}}{{ref:B}}"));
        assert!(is_sentinel_concatenation("{{getatt:A.Attr}}{{ref:B}}"));
        assert!(!is_sentinel_concatenation("{{ref:A}}:live"));
        assert!(!is_sentinel_concatenation("{{getatt:Fn.Arn}}:live"));
        assert!(!is_sentinel_concatenation("arn:aws:...{{ref:X}}"));
        assert!(!is_sentinel_concatenation("{{ref:Only}}"));
    }
}
