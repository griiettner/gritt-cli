//! Repository root discovery and date helpers.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{CliError, Result};

/// Resolves the repository root.
///
/// Order: an explicit path, the nearest ancestor of the working directory
/// that contains `.agents/`, then `git rev-parse --show-toplevel`.
pub fn resolve_root(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        let resolved = path.canonicalize().map_err(|error| {
            CliError::usage(format!("invalid --repo-root {}: {error}", path.display()))
        })?;
        return Ok(resolved);
    }
    let cwd = env::current_dir()?;
    for candidate in cwd.ancestors() {
        if candidate.join(".agents").is_dir() {
            return Ok(candidate.to_path_buf());
        }
    }
    if let Some(top) = git_toplevel(&cwd) {
        return Ok(top);
    }
    Err(CliError::usage(
        "could not find a repository root containing .agents; pass --repo-root <path>",
    ))
}

fn git_toplevel(start: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(start)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(PathBuf::from(trimmed))
}

pub fn tasks_root(repo: &Path) -> PathBuf {
    repo.join(".agents").join("tasks")
}

pub fn memory_root(repo: &Path) -> PathBuf {
    repo.join(".agents").join("memory")
}

pub fn skills_root(repo: &Path) -> PathBuf {
    repo.join(".agents").join("skills")
}

/// Today's date in the local timezone as `YYYY-MM-DD`.
pub fn local_date() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// Current UTC time as an ISO 8601 timestamp with milliseconds.
pub fn utc_timestamp() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}
