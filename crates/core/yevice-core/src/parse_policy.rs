//! Shared types for the IaC parse-failure policy.
//!
//! Implements [ADR-0003](../../../../docs/adr/0003-iac-parse-failure-policy.md):
//! the three IaC parsers (CFN / Terraform / Wrangler) accept a [`ParsePolicy`]
//! that decides whether "soft" failures (missing parameters, unresolved
//! `var.*` / `local.*`, missing import values, …) abort parsing or are
//! collected into [`IacParseDiagnostic`] records on a [`ParseOutcome`].
//!
//! Constructional invariants:
//!
//! * Syntax / IO / programmer-error failures (`Yaml`, `Io`, `IntrinsicError`,
//!   `ParseError`) are policy-neutral and stay on the parser's `Err` channel.
//! * `code` is an ASCII snake_case identifier (`"missing_parameter"`,
//!   `"unresolved_var_ref"`, …); uniqueness is `{source}.{code}`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// How an IaC parser should treat policy-controllable failures.
///
/// Default is [`Lenient`](ParsePolicy::Lenient): unresolved references and
/// missing parameters are demoted to [`IacParseDiagnostic`] records and
/// best-effort values are returned. [`Strict`](ParsePolicy::Strict) restores
/// today's CFN semantics — any policy-controllable failure aborts parsing.
///
/// Note that "hard" parser errors (syntax, IO) are policy-neutral and abort
/// regardless of the selected mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ParsePolicy {
    /// Demote unresolved references / missing parameters to diagnostics.
    #[default]
    Lenient,
    /// Abort on any unresolved reference / missing parameter.
    Strict,
}

/// Severity of an [`IacParseDiagnostic`].
///
/// Only [`Severity::Error`] entries set [`ParseOutcome::had_errors`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Equivalent in effect to a hard error under Strict; under Lenient it
    /// is recorded and surfaces non-zero `had_errors`.
    Error,
    /// Recoverable: parsing succeeded but the resulting value is partial /
    /// best-effort.
    Warning,
    /// Informational only.
    Info,
}

/// Which IaC parser produced the diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSource {
    /// CloudFormation parser (`yevice-cfn`).
    Cfn,
    /// Terraform parser (`yevice-tf`).
    Tf,
    /// Cloudflare Wrangler parser (`yevice-wrangler`).
    Wrangler,
}

/// Optional source-location pointer attached to a diagnostic.
///
/// Line/column may be absent when the source layer cannot pinpoint the
/// offending node (e.g. a missing parameter is a whole-template concern, not a
/// single line).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLocation {
    /// Source file (best-effort: may point at the directory for multi-file
    /// inputs such as a Terraform module).
    pub file: PathBuf,
    /// 1-based line number, when known.
    pub line: Option<u32>,
    /// 1-based column number, when known.
    pub column: Option<u32>,
}

impl SourceLocation {
    /// Construct a [`SourceLocation`] with only a file path (line/column
    /// unknown).
    pub fn file_only(file: impl Into<PathBuf>) -> Self {
        Self {
            file: file.into(),
            line: None,
            column: None,
        }
    }
}

/// A single recoverable parser finding.
///
/// Diagnostics are returned through [`ParseOutcome::diagnostics`] in addition
/// to (not instead of) the parsed value. `code` is a stable ASCII snake_case
/// identifier (e.g. `"missing_parameter"`, `"unresolved_var_ref"`); the
/// `{source}.{code}` pair is unique across the workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IacParseDiagnostic {
    /// Effect on overall parse success.
    pub severity: Severity,
    /// Which parser raised the diagnostic.
    pub source: DiagnosticSource,
    /// Best-effort source pointer.
    pub location: Option<SourceLocation>,
    /// Stable ASCII snake_case identifier; pair with `source` for uniqueness.
    pub code: String,
    /// Human-readable message body. Should not embed the code or source name
    /// (those are reported separately by renderers).
    pub message: String,
}

impl IacParseDiagnostic {
    /// Build an error-severity diagnostic with no source location.
    pub fn error(
        source: DiagnosticSource,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity: Severity::Error,
            source,
            location: None,
            code: code.into(),
            message: message.into(),
        }
    }

    /// Attach a [`SourceLocation`] to a diagnostic (builder-style).
    #[must_use]
    pub fn with_location(mut self, location: SourceLocation) -> Self {
        self.location = Some(location);
        self
    }
}

/// A parser's structured success result.
///
/// Even when [`Self::had_errors`] is true, [`Self::value`] is populated with a
/// best-effort parsed value: it is the caller's responsibility to decide
/// whether to surface the value or abort based on `had_errors` /
/// [`Severity::Error`] presence in `diagnostics`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParseOutcome<T> {
    /// The parsed value (best-effort under Lenient).
    pub value: T,
    /// Recoverable findings collected during parsing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<IacParseDiagnostic>,
    /// Convenience flag: any [`Severity::Error`] entry present in
    /// `diagnostics`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub had_errors: bool,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_false(b: &bool) -> bool {
    !*b
}

impl<T> ParseOutcome<T> {
    /// Construct an outcome with no diagnostics (i.e. a clean parse).
    pub fn clean(value: T) -> Self {
        Self {
            value,
            diagnostics: Vec::new(),
            had_errors: false,
        }
    }

    /// Construct an outcome from a value and an explicit diagnostics list;
    /// `had_errors` is derived from any [`Severity::Error`] entries.
    pub fn with_diagnostics(value: T, diagnostics: Vec<IacParseDiagnostic>) -> Self {
        let had_errors = diagnostics
            .iter()
            .any(|d| matches!(d.severity, Severity::Error));
        Self {
            value,
            diagnostics,
            had_errors,
        }
    }

    /// Append a diagnostic, updating `had_errors` if the new entry is an
    /// error.
    pub fn push(&mut self, diagnostic: IacParseDiagnostic) {
        if matches!(diagnostic.severity, Severity::Error) {
            self.had_errors = true;
        }
        self.diagnostics.push(diagnostic);
    }

    /// Apply a transformation to the held value, preserving diagnostics.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> ParseOutcome<U> {
        ParseOutcome {
            value: f(self.value),
            diagnostics: self.diagnostics,
            had_errors: self.had_errors,
        }
    }

    /// Merge another outcome's diagnostics into this one (ignoring the
    /// other outcome's value).
    pub fn merge_diagnostics<U>(&mut self, other: ParseOutcome<U>) {
        if other.had_errors {
            self.had_errors = true;
        }
        self.diagnostics.extend(other.diagnostics);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_policy_default_is_lenient() {
        assert_eq!(ParsePolicy::default(), ParsePolicy::Lenient);
    }

    #[test]
    fn with_diagnostics_sets_had_errors_when_any_error_present() {
        let diags = vec![
            IacParseDiagnostic {
                severity: Severity::Warning,
                source: DiagnosticSource::Cfn,
                location: None,
                code: "x".into(),
                message: "warn".into(),
            },
            IacParseDiagnostic::error(DiagnosticSource::Cfn, "y", "boom"),
        ];
        let outcome = ParseOutcome::with_diagnostics((), diags);
        assert!(outcome.had_errors);
    }

    #[test]
    fn with_diagnostics_keeps_had_errors_false_without_errors() {
        let diags = vec![IacParseDiagnostic {
            severity: Severity::Warning,
            source: DiagnosticSource::Tf,
            location: None,
            code: "w".into(),
            message: "warn".into(),
        }];
        let outcome = ParseOutcome::with_diagnostics(42, diags);
        assert!(!outcome.had_errors);
        assert_eq!(outcome.value, 42);
    }

    #[test]
    fn push_marks_had_errors_only_for_error_severity() {
        let mut outcome: ParseOutcome<()> = ParseOutcome::clean(());
        outcome.push(IacParseDiagnostic {
            severity: Severity::Warning,
            source: DiagnosticSource::Wrangler,
            location: None,
            code: "w".into(),
            message: "m".into(),
        });
        assert!(!outcome.had_errors);
        outcome.push(IacParseDiagnostic::error(
            DiagnosticSource::Wrangler,
            "e",
            "m",
        ));
        assert!(outcome.had_errors);
    }

    #[test]
    fn serializes_diagnostics_only_when_non_empty() {
        let clean: ParseOutcome<u32> = ParseOutcome::clean(7);
        let json = serde_json::to_string(&clean).unwrap();
        assert!(
            !json.contains("diagnostics"),
            "empty diagnostics must be elided from JSON; got: {json}"
        );
        assert!(
            !json.contains("had_errors"),
            "had_errors=false must be elided from JSON; got: {json}"
        );
    }

    #[test]
    fn serializes_diagnostics_when_present() {
        let outcome = ParseOutcome::with_diagnostics(
            "v",
            vec![IacParseDiagnostic::error(
                DiagnosticSource::Cfn,
                "missing_parameter",
                "TableName",
            )],
        );
        let json = serde_json::to_string(&outcome).unwrap();
        assert!(json.contains("\"diagnostics\""));
        assert!(json.contains("\"had_errors\":true"));
        assert!(json.contains("missing_parameter"));
    }
}
