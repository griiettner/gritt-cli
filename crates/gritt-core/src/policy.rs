//! Permission policy data. Evaluation lives in the harness (TKT-0011);
//! this module only defines the rules and the workspace-aware defaults.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyOutcome {
    Allow,
    Ask,
    Deny,
}

/// Resource patterns use `*` as a wildcard. `workspace:` prefixed patterns
/// are resolved against the session workspace by the evaluator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyRule {
    pub tool: String,
    pub resource: String,
    pub outcome: PolicyOutcome,
    /// Shown in the approval prompt.
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyConfig {
    /// Evaluated in order; the first match wins.
    pub rules: Vec<PolicyRule>,
    /// Applied when no rule matches.
    pub fallback: PolicyOutcome,
}

impl PolicyConfig {
    /// Workspace-aware defaults: reads inside the workspace allow, writes
    /// ask, shell and network ask, anything outside the workspace is denied.
    pub fn workspace_defaults() -> Self {
        let rule = |tool: &str, resource: &str, outcome, reason: &str| PolicyRule {
            tool: tool.into(),
            resource: resource.into(),
            outcome,
            reason: reason.into(),
        };
        Self {
            rules: vec![
                rule(
                    crate::tool::native::FILE_READ,
                    "workspace:**",
                    PolicyOutcome::Allow,
                    "read inside the workspace",
                ),
                rule(
                    crate::tool::native::FILE_WRITE,
                    "workspace:**",
                    PolicyOutcome::Ask,
                    "write inside the workspace",
                ),
                rule(
                    crate::tool::native::SHELL,
                    "*",
                    PolicyOutcome::Ask,
                    "run a shell command",
                ),
                rule("network", "*", PolicyOutcome::Ask, "network access"),
                rule("*", "*", PolicyOutcome::Deny, "outside the workspace"),
            ],
            fallback: PolicyOutcome::Deny,
        }
    }
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self::workspace_defaults()
    }
}
