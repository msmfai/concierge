//! Shared lint result types. They live in core (not `concierge-lint`) so that
//! game **adapters** can produce lints via [`crate::game::GameAdapter::lints`]
//! without a dependency cycle — the adapter crates already depend on core.

/// How badly a violated invariant breaks the game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// A guaranteed crash/break — `realize` should refuse to declare success.
    Error,
    /// A likely problem or silent break — surfaced but not fatal.
    Warn,
}

/// One violated invariant: which mod/plugin, which rule, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub severity: Severity,
    /// The offending mod or plugin (file/folder name).
    pub subject: String,
    /// Stable rule id, e.g. `"missing-master"` or `"duplicate-uniqueid"`.
    pub rule: String,
    /// Human-readable explanation, ideally naming the fix.
    pub detail: String,
}

impl Violation {
    pub fn error(
        subject: impl Into<String>,
        rule: &'static str,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            severity: Severity::Error,
            subject: subject.into(),
            rule: rule.to_owned(),
            detail: detail.into(),
        }
    }
    pub fn warn(subject: impl Into<String>, rule: &'static str, detail: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warn,
            subject: subject.into(),
            rule: rule.to_owned(),
            detail: detail.into(),
        }
    }
}

/// Split violations into (errors, warnings) for callers to render.
#[must_use]
pub fn partition(violations: Vec<Violation>) -> (Vec<Violation>, Vec<Violation>) {
    violations
        .into_iter()
        .partition(|v| v.severity == Severity::Error)
}
