//! Resolves the GitHub login used as the developer's ticket namespace.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::store::{is_namespace_name, SHARED_NAMESPACE};
use crate::fsx;
use crate::repo::utc_timestamp;
use crate::{CliError, Result};

pub const IDENTITY_ENV: &str = "GRITT_TKT_NAMESPACE";
pub const IDENTITY_RELATIVE_PATH: &str = ".agents/state/identity.local.yaml";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub github_login: String,
    pub source: String,
    pub resolved_at: String,
}

pub fn identity_file_path(repo_root: &Path) -> PathBuf {
    repo_root
        .join(".agents")
        .join("state")
        .join("identity.local.yaml")
}

pub fn parse_identity_yaml(text: &str) -> Option<Identity> {
    let field = |name: &str| -> Option<String> {
        text.lines().find_map(|line| {
            let rest = line.strip_prefix(name)?.strip_prefix(':')?;
            let value = rest.trim();
            if value.is_empty() || value.split_whitespace().count() != 1 {
                None
            } else {
                Some(value.to_owned())
            }
        })
    };
    let github_login = field("github_login")?;
    Some(Identity {
        github_login,
        source: field("source").unwrap_or_else(|| "file".to_owned()),
        resolved_at: field("resolved_at").unwrap_or_default(),
    })
}

pub fn render_identity_yaml(identity: &Identity) -> String {
    format!(
        "github_login: {}\nsource: {}\nresolved_at: {}\n",
        identity.github_login, identity.source, identity.resolved_at
    )
}

pub fn normalize_namespace(value: &str, label: &str) -> Result<String> {
    let namespace = value.trim();
    if !is_namespace_name(namespace) || namespace == SHARED_NAMESPACE {
        return Err(CliError::usage(format!(
            "invalid {label}: {value}. Use a GitHub login (letters, digits, hyphen, underscore, dot)."
        )));
    }
    Ok(namespace.to_owned())
}

pub fn persist_identity(repo_root: &Path, identity: &Identity) -> Result<PathBuf> {
    let target = identity_file_path(repo_root);
    fsx::write_text(&target, &render_identity_yaml(identity))?;
    Ok(target)
}

#[derive(Debug, Default, Clone)]
pub struct ResolveOptions {
    pub namespace: Option<String>,
    pub refresh: bool,
    pub persist: bool,
}

/// Resolution order: `--namespace`, `GRITT_TKT_NAMESPACE`, the stored
/// identity file, then `gh api user`.
pub fn resolve_ticket_identity(repo_root: &Path, options: &ResolveOptions) -> Result<Identity> {
    if let Some(value) = options
        .namespace
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        let identity = Identity {
            github_login: normalize_namespace(value, "--namespace")?,
            source: "flag".to_owned(),
            resolved_at: utc_timestamp(),
        };
        if options.persist {
            persist_identity(repo_root, &identity)?;
        }
        return Ok(identity);
    }

    if let Some(value) = env::var(IDENTITY_ENV)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
    {
        let identity = Identity {
            github_login: normalize_namespace(&value, IDENTITY_ENV)?,
            source: "env".to_owned(),
            resolved_at: utc_timestamp(),
        };
        if options.persist {
            persist_identity(repo_root, &identity)?;
        }
        return Ok(identity);
    }

    if !options.refresh {
        if let Some(stored) = read_stored_identity(repo_root)? {
            return Ok(stored);
        }
    }

    if let Some(login) = lookup_github_login(repo_root) {
        let identity = Identity {
            github_login: normalize_namespace(&login, "GitHub login")?,
            source: "gh".to_owned(),
            resolved_at: utc_timestamp(),
        };
        if options.persist {
            persist_identity(repo_root, &identity)?;
        }
        return Ok(identity);
    }

    Err(CliError::usage(
        "could not resolve a GitHub login for ticket namespacing. Run `gh auth login`, set GRITT_TKT_NAMESPACE, or pass --namespace <github-login>.",
    ))
}

fn read_stored_identity(repo_root: &Path) -> Result<Option<Identity>> {
    let target = identity_file_path(repo_root);
    if !fsx::exists(&target) {
        return Ok(None);
    }
    let Some(mut parsed) = parse_identity_yaml(&fsx::read_text(&target)?) else {
        return Ok(None);
    };
    parsed.github_login = normalize_namespace(&parsed.github_login, IDENTITY_RELATIVE_PATH)?;
    Ok(Some(parsed))
}

fn lookup_github_login(repo_root: &Path) -> Option<String> {
    let output = Command::new("gh")
        .args(["api", "user", "--jq", ".login"])
        .current_dir(repo_root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let login = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    if login.is_empty() {
        None
    } else {
        Some(login)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_yaml_round_trips() {
        let identity = Identity {
            github_login: "alice".to_owned(),
            source: "flag".to_owned(),
            resolved_at: "2026-09-03T00:00:00.000Z".to_owned(),
        };
        let text = render_identity_yaml(&identity);
        assert_eq!(parse_identity_yaml(&text), Some(identity));
        assert_eq!(parse_identity_yaml("source: gh\n"), None);
    }

    #[test]
    fn rejects_shared_and_invalid_namespaces() {
        assert!(normalize_namespace("_shared", "x").is_err());
        assert!(normalize_namespace("-bad", "x").is_err());
        assert_eq!(normalize_namespace(" alice ", "x").unwrap(), "alice");
    }
}
