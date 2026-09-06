//! Executable lookup and the version and auth probes every connector runs
//! before a task starts.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use gritt_core::{Error, Result};

/// Finds `name` on `PATH`, or returns an explicit path as given when it
/// exists. `None` when the agent is not installed.
pub fn find_executable(name: &str) -> Option<PathBuf> {
    let candidate = Path::new(name);
    if candidate.components().count() > 1 {
        return candidate.is_file().then(|| candidate.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let direct = dir.join(name);
        if direct.is_file() {
            return Some(direct);
        }
        #[cfg(windows)]
        {
            for ext in ["exe", "cmd", "bat"] {
                let with_ext = dir.join(format!("{name}.{ext}"));
                if with_ext.is_file() {
                    return Some(with_ext);
                }
            }
        }
    }
    None
}

/// Output of a short probe such as `--version`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Runs `program args` with a timeout and captures its output. A timeout
/// or spawn failure is a connector error naming the program, never a
/// panic.
pub async fn probe(program: &Path, args: &[String], timeout: Duration) -> Result<ProbeOutput> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::time::timeout(timeout, command.output()).await;
    match output {
        Ok(Ok(output)) => Ok(ProbeOutput {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }),
        Ok(Err(error)) => Err(Error::connector(format!(
            "cannot run {}: {error}",
            program.display()
        ))),
        Err(_) => Err(Error::connector(format!(
            "{} did not answer within {}s",
            program.display(),
            timeout.as_secs()
        ))),
    }
}

/// The first token that looks like a version number in `text`.
pub fn version_token(text: &str) -> Option<String> {
    text.split_whitespace()
        .map(|token| token.trim_matches(|c: char| c == 'v' || c == '(' || c == ')' || c == ','))
        .find(|token| {
            let mut parts = token.split('.');
            matches!(parts.next(), Some(first) if first.chars().all(|c| c.is_ascii_digit()) && !first.is_empty())
                && parts.next().is_some()
        })
        .map(str::to_owned)
}

/// The numeric components of a dotted version. A leading `v` and any
/// pre-release suffix after the digits of a component are ignored.
pub fn parse_version(text: &str) -> Option<Vec<u64>> {
    text.trim()
        .trim_start_matches('v')
        .split('.')
        .map(|part| {
            part.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .ok()
        })
        .collect()
}

/// Orders two dotted versions; `None` when either does not parse.
pub fn compare_versions(left: &str, right: &str) -> Option<std::cmp::Ordering> {
    let left = parse_version(left)?;
    let right = parse_version(right)?;
    for index in 0..left.len().max(right.len()) {
        let a = left.get(index).copied().unwrap_or(0);
        let b = right.get(index).copied().unwrap_or(0);
        if a != b {
            return Some(a.cmp(&b));
        }
    }
    Some(std::cmp::Ordering::Equal)
}

/// Compares dotted versions; `None` when either does not parse.
pub fn version_at_least(found: &str, minimum: &str) -> Option<bool> {
    compare_versions(found, minimum).map(|ordering| ordering != std::cmp::Ordering::Less)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_tokens_and_comparisons() {
        assert_eq!(
            version_token("2.1.260 (Claude Code)").as_deref(),
            Some("2.1.260")
        );
        assert_eq!(
            version_token("codex-cli 0.153.2").as_deref(),
            Some("0.153.2")
        );
        assert_eq!(version_token("fake 1.0.0").as_deref(), Some("1.0.0"));
        assert_eq!(version_token("no version here"), None);
        assert_eq!(version_at_least("0.153.2", "0.150.0"), Some(true));
        assert_eq!(version_at_least("0.9", "0.10.0"), Some(false));
        assert_eq!(version_at_least("x", "1"), None);
        assert_eq!(
            compare_versions("v1.2.3", "1.2.3"),
            Some(std::cmp::Ordering::Equal)
        );
        assert_eq!(
            compare_versions("1.10.0", "1.9.9"),
            Some(std::cmp::Ordering::Greater)
        );
        assert_eq!(
            compare_versions("2.1.263", "2.1.270"),
            Some(std::cmp::Ordering::Less)
        );
        assert_eq!(
            compare_versions("1.2.3-beta", "1.2.3"),
            Some(std::cmp::Ordering::Equal)
        );
        assert_eq!(compare_versions("", "1.0"), None);
    }

    #[test]
    fn missing_executable_is_none() {
        assert!(find_executable("gritt-definitely-not-installed-xyz").is_none());
        assert!(find_executable("/definitely/not/here").is_none());
    }
}
