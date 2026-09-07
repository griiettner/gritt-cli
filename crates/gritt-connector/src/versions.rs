//! Connector version checks and updates. The owner detector and the
//! documented commands live in [`crate::install`]; this module keeps the
//! last successful latest-version answer with the same freshness and
//! stale-fallback rules as model lists (ADR-008), and runs an approved
//! update through the supervised process path.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use gritt_core::config::ModelListPolicy;
use gritt_core::connector::{ConnectorId, VersionComparison};
use gritt_core::session::BoxFuture;
use gritt_core::{Error, Result};
use serde::{Deserialize, Serialize};

use crate::health::compare_versions;
use crate::models::interval;
use crate::process::{self, Launch, ProcessGuard};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CachedConnectorVersion {
    /// Executable, installation source, and querying manager this result belongs to.
    #[serde(default)]
    pub installation: Option<String>,
    /// When `latest` was last read successfully.
    #[serde(default)]
    pub checked_at: Option<DateTime<Utc>>,
    /// When a query last ran, successful or not.
    #[serde(default)]
    pub last_attempt_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub latest: Option<String>,
    #[serde(default)]
    pub latest_source: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ConnectorVersionCache {
    dir: PathBuf,
}

impl ConnectorVersionCache {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn default_dir() -> Option<PathBuf> {
        dirs::cache_dir().map(|dir| dir.join("gritt").join("connector-versions"))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn path(&self, id: ConnectorId) -> PathBuf {
        self.dir.join(format!("{}.json", id.as_str()))
    }

    pub fn read(&self, id: ConnectorId) -> Result<Option<CachedConnectorVersion>> {
        let path = self.path(id);
        if !path.is_file() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|error| Error::storage(format!("cannot read {}: {error}", path.display())))?;
        Ok(serde_json::from_str(&text).ok())
    }

    pub fn write(&self, id: ConnectorId, cached: &CachedConnectorVersion) -> Result<()> {
        std::fs::create_dir_all(&self.dir).map_err(|error| {
            Error::storage(format!("cannot create {}: {error}", self.dir.display()))
        })?;
        let path = self.path(id);
        let text = serde_json::to_string_pretty(cached)
            .map_err(|error| Error::storage(error.to_string()))?;
        std::fs::write(&path, text)
            .map_err(|error| Error::storage(format!("cannot write {}: {error}", path.display())))
    }
}

pub fn cache_is_fresh(
    cached: &CachedConnectorVersion,
    policy: &ModelListPolicy,
    now: DateTime<Utc>,
) -> bool {
    cached
        .checked_at
        .is_some_and(|checked| now.signed_duration_since(checked) < interval(policy))
}

/// A query ran after the last successful read and did not replace it.
pub fn failed_since_check(cached: &CachedConnectorVersion) -> bool {
    match (cached.checked_at, cached.last_attempt_at) {
        (Some(checked), Some(attempt)) => attempt > checked,
        (None, Some(_)) => true,
        _ => false,
    }
}

pub fn attempted_recently(
    cached: &CachedConnectorVersion,
    policy: &ModelListPolicy,
    now: DateTime<Utc>,
) -> bool {
    cached
        .last_attempt_at
        .is_some_and(|attempt| now.signed_duration_since(attempt) < interval(policy))
}

pub fn comparison(installed: Option<&str>, latest: Option<&str>) -> VersionComparison {
    let (Some(installed), Some(latest)) = (installed, latest) else {
        return VersionComparison::Unknown;
    };
    match compare_versions(installed, latest) {
        Some(std::cmp::Ordering::Less) => VersionComparison::Outdated,
        Some(std::cmp::Ordering::Equal) => VersionComparison::Current,
        Some(std::cmp::Ordering::Greater) => VersionComparison::Newer,
        None => VersionComparison::Unknown,
    }
}

/// No update may run longer than this, however busy it looks.
pub const UPDATE_HARD_CAP: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateRun {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub cancelled: bool,
}

/// Runs `program args` through the supervised process path. Output is
/// read line by line with `idle` as the silence bound and
/// [`UPDATE_HARD_CAP`] as the total bound; both kill the process tree.
pub async fn run_update(
    program: &Path,
    args: &[String],
    cwd: &Path,
    idle: Duration,
    cancel: &mut BoxFuture<'_, ()>,
) -> Result<UpdateRun> {
    let launch = Launch {
        program: program.to_path_buf(),
        args: args.to_vec(),
        cwd: cwd.to_path_buf(),
        env_remove: Vec::new(),
        transport: gritt_core::connector::Transport::MachineReadable,
    };
    let supervised = process::spawn(&launch).await?;
    let mut guard = ProcessGuard {
        supervised: Some(supervised),
        finished: false,
    };
    let started = tokio::time::Instant::now();
    let mut timed_out = false;
    let mut cancelled = false;
    loop {
        let remaining = UPDATE_HARD_CAP.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            timed_out = true;
            break;
        }
        let wait = idle.min(remaining);
        let supervised = guard.supervised.as_mut().expect("child until finished");
        let next = tokio::select! {
            biased;
            () = &mut *cancel => { cancelled = true; break; }
            next = tokio::time::timeout(wait, supervised.lines.recv()) => next,
        };
        match next {
            // Output is activity only. It may contain credentials unknown
            // to Gritt, so no stdout/stderr content leaves this runner.
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(_) => {
                timed_out = true;
                break;
            }
        }
    }
    let supervised = guard.supervised.as_mut().expect("child until finished");
    let exit = if timed_out || cancelled {
        supervised.control.kill().await;
        None
    } else {
        tokio::select! {
            biased;
            () = &mut *cancel => { cancelled = true; None }
            exit = supervised.control.wait(Duration::from_secs(30)) => exit,
        }
    };
    if exit.is_none() && !timed_out {
        if let Some(pid) = supervised.control.pid() {
            process::kill_tree(pid).await;
        }
        supervised.control.kill().await;
    }
    guard.finished = true;
    Ok(UpdateRun {
        success: exit.is_some_and(|exit| exit.success),
        exit_code: exit.and_then(|exit| exit.code),
        timed_out,
        cancelled,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparisons_cover_every_outcome() {
        assert_eq!(
            comparison(Some("1.0.0"), Some("1.0.1")),
            VersionComparison::Outdated
        );
        assert_eq!(
            comparison(Some("1.0.1"), Some("1.0.1")),
            VersionComparison::Current
        );
        assert_eq!(
            comparison(Some("1.1"), Some("1.0.9")),
            VersionComparison::Newer
        );
        assert_eq!(comparison(Some("x"), Some("1")), VersionComparison::Unknown);
        assert_eq!(comparison(None, Some("1")), VersionComparison::Unknown);
    }

    #[test]
    fn a_failed_attempt_after_a_check_marks_the_cache_stale() {
        let now = Utc::now();
        let policy = ModelListPolicy::default();
        let fresh = CachedConnectorVersion {
            installation: None,
            checked_at: Some(now),
            last_attempt_at: Some(now),
            latest: Some("1".into()),
            latest_source: None,
        };
        assert!(cache_is_fresh(&fresh, &policy, now));
        assert!(!failed_since_check(&fresh));
        let failed = CachedConnectorVersion {
            last_attempt_at: Some(now + chrono::Duration::seconds(5)),
            ..fresh
        };
        assert!(failed_since_check(&failed));
        assert!(attempted_recently(
            &failed,
            &policy,
            now + chrono::Duration::seconds(6)
        ));
    }
}
