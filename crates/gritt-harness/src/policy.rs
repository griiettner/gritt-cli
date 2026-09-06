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
/// absolute path elsewhere, a drive-letter path, a `..` component, or a
/// path the shell would expand (`~`, `~user`, `$VAR`, `${VAR}`, `%VAR%`),
/// which is treated conservatively as outside because the expansion is
/// unknown here. The shell itself is not confined (ADR-009 runs it under
/// approval, and only the file tools are workspace-bounded), so such a
/// command is forced to at least `ask` with the stronger prompt.
/// `/dev/null` is exempt.
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
            if shell_expands(&token) {
                return true;
            }
            let bytes = token.as_bytes();
            let absolute = token.starts_with('/')
                || (bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic());
            absolute && token != root && !token.starts_with(&format!("{root}/"))
        })
}

/// True for a token the shell would expand into a path Gritt cannot see:
/// `~`, `~user`, `$VAR`, `${VAR}`, or `%VAR%`.
fn shell_expands(token: &str) -> bool {
    if token.starts_with('~') {
        return true;
    }
    let mut chars = token.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '$' => match chars.peek() {
                Some('{') => return true,
                Some(next) if next.is_ascii_alphanumeric() || *next == '_' => return true,
                _ => {}
            },
            '%' => {
                let rest: String = chars.clone().collect();
                if let Some(end) = rest.find('%') {
                    let name = &rest[..end];
                    if !name.is_empty()
                        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    {
                        return true;
                    }
                }
            }
            _ => {}
        }
    }
    false
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

    pub fn evaluate_mode(
        &self,
        tool: &str,
        resource: &Resource,
        mode: gritt_core::session::ExecutionMode,
    ) -> Decision {
        let mut decision = self.evaluate(tool, resource);
        if mode == gritt_core::session::ExecutionMode::FullAccess {
            decision.outcome = PolicyOutcome::Allow;
            decision.reason = "full access selected for this run".into();
        }
        decision
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

    /// The gate. First matching rule wins; otherwise the fallback. A
    /// shell command that reaches outside the workspace is never allowed
    /// silently: an `allow` outcome becomes `ask` with the stronger
    /// prompt, whichever rule produced it. `deny` stays `deny`.
    pub fn evaluate(&self, tool: &str, resource: &Resource) -> Decision {
        let (destructive, outside) = match resource {
            Resource::Command(command) => (
                looks_destructive(command),
                reaches_outside_workspace(command, &self.workspace),
            ),
            _ => (false, false),
        };
        let destructive = destructive || outside;
        let (outcome, reason, rule) = match self
            .config
            .rules
            .iter()
            .enumerate()
            .find(|(_, rule)| self.rule_matches(rule, tool, resource))
        {
            Some((index, rule)) => {
                let reason = if rule.reason.is_empty() {
                    format!("rule {index} for `{}`", rule.tool)
                } else {
                    rule.reason.clone()
                };
                (rule.outcome, reason, Some(index))
            }
            None => (
                self.config.fallback,
                "no policy rule matched".to_string(),
                None,
            ),
        };
        let (outcome, reason) = if outside && outcome == PolicyOutcome::Allow {
            (
                PolicyOutcome::Ask,
                format!("{reason}; the command names a path outside the workspace, so it must be approved"),
            )
        } else if outcome == PolicyOutcome::Ask && outside {
            (
                outcome,
                format!("{reason}; the command names a path outside the workspace"),
            )
        } else if outcome == PolicyOutcome::Ask && destructive {
            (outcome, format!("{reason}; the command looks destructive"))
        } else {
            (outcome, reason)
        };
        Decision {
            outcome,
            reason,
            destructive,
            rule,
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
    fn shell_expansions_count_as_outside_the_workspace() {
        for command in [
            "cat ~/secrets",
            "ls ~bob/x",
            "cat $HOME/.ssh/id_rsa",
            "cat ${HOME}/x",
            "type %USERPROFILE%\\x",
            "cp a ~",
        ] {
            let decision = engine().evaluate(native::SHELL, &Resource::Command(command.into()));
            assert_eq!(decision.outcome, PolicyOutcome::Ask, "{command}");
            assert!(decision.destructive, "{command}");
            assert!(
                decision.reason.contains("outside the workspace"),
                "{command}: {}",
                decision.reason
            );
        }
        for command in ["echo 100%", "echo a$ b", "printf '%s' x", "ls /ws/src"] {
            let decision = engine().evaluate(native::SHELL, &Resource::Command(command.into()));
            assert!(!decision.destructive, "{command}: {}", decision.reason);
        }
    }

    #[test]
    fn an_allow_rule_cannot_let_a_command_outside_the_workspace_run_silently() {
        let mut config = PolicyConfig::workspace_defaults();
        config.rules.insert(
            0,
            PolicyRule {
                tool: native::SHELL.into(),
                resource: "*".into(),
                outcome: PolicyOutcome::Allow,
                reason: "shell is free".into(),
            },
        );
        let engine = PolicyEngine::new(config, "/ws");
        let inside = engine.evaluate(native::SHELL, &Resource::Command("cargo test".into()));
        assert_eq!(inside.outcome, PolicyOutcome::Allow);
        for command in ["cat /etc/passwd", "ls ../x", "cat ~/.netrc", "cat $HOME/x"] {
            let outside = engine.evaluate(native::SHELL, &Resource::Command(command.into()));
            assert_eq!(outside.outcome, PolicyOutcome::Ask, "{command}");
            assert!(outside.destructive, "{command}");
            assert!(
                outside.reason.contains("must be approved"),
                "{}",
                outside.reason
            );
            assert_eq!(outside.rule, Some(0));
        }
        // A deny stays a deny.
        let mut config = PolicyConfig::workspace_defaults();
        config.rules.insert(
            0,
            PolicyRule {
                tool: native::SHELL.into(),
                resource: "*".into(),
                outcome: PolicyOutcome::Deny,
                reason: "no shell".into(),
            },
        );
        let engine = PolicyEngine::new(config, "/ws");
        let denied = engine.evaluate(native::SHELL, &Resource::Command("cat /etc/passwd".into()));
        assert_eq!(denied.outcome, PolicyOutcome::Deny);
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
