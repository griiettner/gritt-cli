//! Executable lookup and the version and auth probes every connector runs
//! before a task starts.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::process::{self, Launch, Line, ProcessGuard};
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

/// Runs `program args` in this process's directory with a timeout and
/// captures its output. A timeout or spawn failure is a connector error
/// naming the program, never a panic.
pub async fn probe(program: &Path, args: &[String], timeout: Duration) -> Result<ProbeOutput> {
    let cwd =
        std::env::current_dir().map_err(|_| Error::connector("cannot locate probe directory"))?;
    probe_in(program, args, &cwd, timeout).await
}

/// [`probe`] with an explicit working directory, for a command whose
/// answer depends on project-scoped configuration.
pub async fn probe_in(
    program: &Path,
    args: &[String],
    cwd: &Path,
    timeout: Duration,
) -> Result<ProbeOutput> {
    let launch = Launch {
        program: program.to_owned(),
        args: args.to_vec(),
        cwd: cwd.to_owned(),
        env_remove: Vec::new(),
        transport: gritt_core::connector::Transport::MachineReadable,
    };
    let child = process::spawn(&launch)
        .await
        .map_err(|_| Error::connector(format!("cannot run {}", program.display())))?;
    let mut guard = ProcessGuard {
        supervised: Some(child),
        finished: false,
    };
    let result = tokio::time::timeout(timeout, async {
        let child = guard.supervised.as_mut().expect("probe process");
        let mut stdout = String::new();
        let mut stderr = String::new();
        while let Some(line) = child.lines.recv().await {
            let (buffer, text) = match line {
                Line::Out(text) => (&mut stdout, text),
                Line::Err(text) => (&mut stderr, text),
            };
            if buffer.len().saturating_add(text.len()) > 4 * 1024 * 1024 {
                return Err(Error::connector("probe output exceeded its size limit"));
            }
            buffer.push_str(&text);
            buffer.push('\n');
        }
        let exit = child
            .control
            .wait(timeout)
            .await
            .ok_or_else(|| Error::connector("probe did not exit"))?;
        Ok(ProbeOutput {
            success: exit.success,
            stdout,
            stderr,
        })
    })
    .await;
    match result {
        Ok(Ok(output)) => {
            guard.finished = true;
            Ok(output)
        }
        Ok(Err(error)) => {
            guard.stop().await;
            Err(error)
        }
        Err(_) => {
            guard.stop().await;
            Err(Error::connector(format!(
                "{} did not answer within {}s",
                program.display(),
                timeout.as_secs()
            )))
        }
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
