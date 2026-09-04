//! Permission engine (ADR-009). Evaluates the `PolicyConfig` rules from
//! `gritt-core` against a tool name and a resource, returning `allow`,
//! `ask`, or `deny`. It runs before every native tool execution; there is
//! no path around it.

use std::path::{Path, PathBuf};

use gritt_core::policy::{PolicyConfig, PolicyOutcome, PolicyRule};

/// What a tool wants to touch. Paths are already resolved and absolute;
/// the engine decides whether they sit inside the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resource {
    Path(PathBuf),
    Command(String),
    Other(String),
}

impl Resource {
    pub fn display(&self) -> String {
        match self {
            Resource::Path(path) => path.display().to_string(),
            Resource::Command(command) | Resource::Other(command) => command.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub outcome: PolicyOutcome,
    /// One line shown in the approval prompt.
    pub reason: String,
    /// True when the resource looks destructive; the prompt is stronger.
    pub destructive: bool,
    /// Index of the matching rule, `None` when the fallback applied.
    pub rule: Option<usize>,
}

/// Shell fragments treated as destructive. Matching any of them upgrades
/// an `ask` to a stronger prompt; it never downgrades a `deny`.
pub const DESTRUCTIVE_FRAGMENTS: [&str; 12] = [
    "rm -r",
    "rm -f",
    "rm -fr",
    "git push --force",
    "git push -f",
    "git reset --hard",
    "git clean",
    "git checkout --",
    "drop table",
    "mkfs",
    "truncate",
    "> /dev/",
];

pub fn looks_destructive(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    DESTRUCTIVE_FRAGMENTS
        .iter()
        .any(|fragment| lower.contains(fragment))
}

/// True when a shell command names a path outside the workspace: an
/// absolute path elsewhere, a drive-letter path, or a `..` component. The
/// shell itself is not confined (ADR-009 runs it under approval, and only
/// the file tools are workspace-bounded), so such a command gets the
/// stronger prompt instead of a different outcome. `/dev/null` is exempt.
pub fn reaches_outside_workspace(command: &str, workspace: &Path) -> bool {
    let root = workspace.to_string_lossy().replace('\\', "/");
    let root = root.trim_end_matches('/');
    command
        .split(|c: char| {
            c.is_whitespace()
                || matches!(
                    c,
                    '"' | '\'' | '=' | ';' | '|' | '&' | '(' | ')' | '<' | '>' | '`'
                )
        })
        .filter(|token| !token.is_empty())
        .any(|token| {
            let token = token.replace('\\', "/");
            if token.split('/').any(|part| part == "..") {
                return true;
            }
            if token == "/dev/null" {
                return false;
            }
            let bytes = token.as_bytes();
            let absolute = token.starts_with('/')
                || (bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic());
            absolute && token != root && !token.starts_with(&format!("{root}/"))
        })
}

/// Matches `pattern` against `text` where `*` spans anything except `/`,
/// `**` spans anything, and `?` is one character.
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    fn go(p: &[char], t: &[char]) -> bool {
        match p.split_first() {
            None => t.is_empty(),
            Some(('*', rest)) if rest.first() == Some(&'*') => {
                let rest = &rest[1..];
                // `**` may also swallow a following separator.
                let rest = if rest.first() == Some(&'/') {
                    &rest[1..]
                } else {
                    rest
                };
                (0..=t.len()).any(|i| go(rest, &t[i..]))
            }
            Some(('*', rest)) => (0..=t.len())
                .take_while(|&i| i == 0 || t[i - 1] != '/')
                .any(|i| go(rest, &t[i..])),
            Some(('?', rest)) => !t.is_empty() && go(rest, &t[1..]),
            Some((c, rest)) => t.first() == Some(c) && go(rest, &t[1..]),
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    go(&p, &t)
}

pub struct PolicyEngine {
    config: PolicyConfig,
    workspace: PathBuf,
}

impl PolicyEngine {
    /// `workspace` must already be canonical; the tools resolve paths
    /// against the same root.
    pub fn new(config: PolicyConfig, workspace: impl Into<PathBuf>) -> Self {
        Self {
            config,
            workspace: workspace.into(),
        }
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    pub fn config(&self) -> &PolicyConfig {
        &self.config
    }

    fn inside_workspace<'a>(&self, path: &'a Path) -> Option<&'a Path> {
        path.strip_prefix(&self.workspace).ok()
    }

    fn rule_matches(&self, rule: &PolicyRule, tool: &str, resource: &Resource) -> bool {
        if !wildcard_match(&rule.tool, tool) {
            return false;
        }
        if let Some(pattern) = rule.resource.strip_prefix("workspace:") {
            return match resource {
                Resource::Path(path) => self.inside_workspace(path).is_some_and(|relative| {
                    let relative = relative.to_string_lossy().replace('\\', "/");
                    pattern == "**" || wildcard_match(pattern, &relative)
                }),
                _ => false,
            };
        }
        // A bare `*` means any resource, including commands and URLs that
        // contain slashes. Longer patterns keep path semantics.
        rule.resource == "*"
            || wildcard_match(&rule.resource, &resource.display().replace('\\', "/"))
    }

    /// The gate. First matching rule wins; otherwise the fallback.
    pub fn evaluate(&self, tool: &str, resource: &Resource) -> Decision {
        let (destructive, outside) = match resource {
            Resource::Command(command) => (
                looks_destructive(command),
                reaches_outside_workspace(command, &self.workspace),
            ),
            _ => (false, false),
        };
        let destructive = destructive || outside;
        for (index, rule) in self.config.rules.iter().enumerate() {
            if self.rule_matches(rule, tool, resource) {
                let reason = if rule.reason.is_empty() {
                    format!("rule {index} for `{}`", rule.tool)
                } else {
                    rule.reason.clone()
                };
                let reason = if rule.outcome == PolicyOutcome::Ask && outside {
                    format!("{reason}; the command names a path outside the workspace")
                } else if rule.outcome == PolicyOutcome::Ask && destructive {
                    format!("{reason}; the command looks destructive")
                } else {
                    reason
                };
                return Decision {
                    outcome: rule.outcome,
                    reason,
                    destructive,
                    rule: Some(index),
                };
            }
        }
        Decision {
            outcome: self.config.fallback,
            reason: "no policy rule matched".into(),
            destructive,
            rule: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gritt_core::tool::native;

    fn engine() -> PolicyEngine {
        PolicyEngine::new(PolicyConfig::workspace_defaults(), "/ws")
    }

    #[test]
    fn wildcard_semantics() {
        assert!(wildcard_match("*", "anything at all"));
        assert!(wildcard_match("**", "a/b/c"));
        assert!(wildcard_match("src/*.rs", "src/main.rs"));
        assert!(!wildcard_match("src/*.rs", "src/nested/main.rs"));
        assert!(wildcard_match("src/**/*.rs", "src/nested/deep/main.rs"));
        assert!(wildcard_match("src/**", "src/main.rs"));
        assert!(wildcard_match("file_?", "file_a"));
        assert!(!wildcard_match("file_?", "file_ab"));
    }

    #[test]
    fn read_inside_workspace_allows() {
        let decision = engine().evaluate(native::FILE_READ, &Resource::Path("/ws/a.txt".into()));
        assert_eq!(decision.outcome, PolicyOutcome::Allow);
        assert_eq!(decision.rule, Some(0));
    }

    #[test]
    fn write_inside_workspace_asks() {
        let decision = engine().evaluate(native::FILE_WRITE, &Resource::Path("/ws/a.txt".into()));
        assert_eq!(decision.outcome, PolicyOutcome::Ask);
        assert!(!decision.destructive);
    }

    #[test]
    fn shell_asks_and_destructive_is_flagged() {
        let plain = engine().evaluate(native::SHELL, &Resource::Command("ls -la".into()));
        assert_eq!(plain.outcome, PolicyOutcome::Ask);
        assert!(!plain.destructive);
        let scary = engine().evaluate(native::SHELL, &Resource::Command("rm -rf build".into()));
        assert_eq!(scary.outcome, PolicyOutcome::Ask);
        assert!(scary.destructive);
        assert!(scary.reason.contains("destructive"));
    }

    #[test]
    fn shell_paths_outside_the_workspace_get_the_stronger_prompt() {
        let plain = engine().evaluate(native::SHELL, &Resource::Command("cargo test -p x".into()));
        assert_eq!(plain.outcome, PolicyOutcome::Ask);
        assert!(!plain.destructive);
        let inside =
            engine().evaluate(native::SHELL, &Resource::Command("cat /ws/src/a.rs".into()));
        assert!(!inside.destructive);
        let absolute =
            engine().evaluate(native::SHELL, &Resource::Command("cat /etc/passwd".into()));
        assert_eq!(absolute.outcome, PolicyOutcome::Ask);
        assert!(absolute.destructive);
        assert!(absolute.reason.contains("outside the workspace"));
        let parent = engine().evaluate(native::SHELL, &Resource::Command("ls ../other".into()));
        assert!(parent.destructive);
        let quoted = engine().evaluate(
            native::SHELL,
            &Resource::Command("cp \"a/../../b\" c".into()),
        );
        assert!(quoted.destructive);
        let devnull = engine().evaluate(native::SHELL, &Resource::Command("cat /dev/null".into()));
        assert!(!devnull.destructive);
        assert!(reaches_outside_workspace(
            "type C:\\Windows\\x",
            Path::new("/ws")
        ));
    }

    #[test]
    fn network_asks() {
        let decision = engine().evaluate("network", &Resource::Other("https://x".into()));
        assert_eq!(decision.outcome, PolicyOutcome::Ask);
    }

    #[test]
    fn outside_workspace_denies() {
        for tool in [native::FILE_READ, native::FILE_WRITE] {
            let decision = engine().evaluate(tool, &Resource::Path("/etc/passwd".into()));
            assert_eq!(decision.outcome, PolicyOutcome::Deny, "{tool}");
        }
    }

    #[test]
    fn fallback_applies_when_no_rule_matches() {
        let engine = PolicyEngine::new(
            PolicyConfig {
                rules: Vec::new(),
                fallback: PolicyOutcome::Deny,
            },
            "/ws",
        );
        let decision = engine.evaluate(native::FILE_READ, &Resource::Path("/ws/a".into()));
        assert_eq!(decision.outcome, PolicyOutcome::Deny);
        assert_eq!(decision.rule, None);
    }

    #[test]
    fn custom_rule_order_is_first_match() {
        let mut config = PolicyConfig::workspace_defaults();
        config.rules.insert(
            0,
            PolicyRule {
                tool: native::FILE_WRITE.into(),
                resource: "workspace:docs/**".into(),
                outcome: PolicyOutcome::Allow,
                reason: "docs are free".into(),
            },
        );
        let engine = PolicyEngine::new(config, "/ws");
        let docs = engine.evaluate(native::FILE_WRITE, &Resource::Path("/ws/docs/a.md".into()));
        assert_eq!(docs.outcome, PolicyOutcome::Allow);
        let src = engine.evaluate(native::FILE_WRITE, &Resource::Path("/ws/src/a.rs".into()));
        assert_eq!(src.outcome, PolicyOutcome::Ask);
    }
}
