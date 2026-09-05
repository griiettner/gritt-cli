//! Workspace change observation for the session sidebar (feature plan,
//! step 4).
//!
//! The section reports **workspace state**, not authorship. A change that
//! was already there when the session opened is labelled `pre-existing`,
//! because Gritt cannot know who made it. Where the workspace is a Git
//! repository the list comes from read-only `git status`; where it is not,
//! only files Gritt itself wrote are observable, and the list says so.
//!
//! Every call here runs off the terminal event path: the runtime spawns
//! the scan and takes the result as a message, so a slow repository never
//! blocks a frame.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::RwLock;

/// How a file differs from the workspace baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ChangeStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
}

impl ChangeStatus {
    pub fn label(self) -> &'static str {
        match self {
            ChangeStatus::Added => "added",
            ChangeStatus::Modified => "modified",
            ChangeStatus::Deleted => "deleted",
            ChangeStatus::Renamed => "renamed",
            ChangeStatus::Untracked => "new",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    pub status: ChangeStatus,
    /// Present in the workspace before Gritt opened it. The sidebar
    /// reports workspace state; it does not claim authorship.
    pub pre_existing: bool,
}

/// Where the changed-file list came from, which decides how complete it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeSource {
    /// Read-only Git status against the baseline taken at open.
    Git,
    /// No Git repository: only files Gritt itself wrote are observable.
    ObservedWrites,
}

impl ChangeSource {
    pub fn caveat(self) -> Option<&'static str> {
        match self {
            ChangeSource::Git => None,
            ChangeSource::ObservedWrites => Some("partial: observed writes only"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangedFiles {
    /// Not collected yet, or collection is not possible here.
    Unavailable { reason: String },
    Observed {
        source: ChangeSource,
        files: Vec<ChangedFile>,
    },
}

impl Default for ChangedFiles {
    fn default() -> Self {
        ChangedFiles::Unavailable {
            reason: "not collected yet".into(),
        }
    }
}

impl ChangedFiles {
    pub fn files(&self) -> &[ChangedFile] {
        match self {
            ChangedFiles::Unavailable { .. } => &[],
            ChangedFiles::Observed { files, .. } => files,
        }
    }
}

/// A read-only diff for one file, or the reason there is none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileDiff {
    Text { path: String, body: String },
    Unavailable { path: String, reason: String },
}

/// Runs a Git subcommand. Every invocation here is read-only: the service
/// never stages, checks out, or writes to the repository.
///
/// Injected so tests do not need a repository, and so a workspace with no
/// `git` on PATH degrades to observed writes instead of failing.
pub trait GitRunner: Send + Sync {
    fn run(&self, root: &Path, args: &[&str]) -> std::io::Result<std::process::Output>;
}

/// At most this many scans or diffs run at once.
///
/// Git and the filesystem are reached from blocking workers, and a
/// workspace can produce a refresh after every tool call. Without a bound,
/// a slow repository would let refreshes pile up and occupy the blocking
/// pool; with it, the newest request waits for the previous one instead.
const MAX_CONCURRENT_SCANS: usize = 2;

/// The real one: the `git` executable, with the workspace as `-C`.
pub struct SystemGit;

impl GitRunner for SystemGit {
    fn run(&self, root: &Path, args: &[&str]) -> std::io::Result<std::process::Output> {
        std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
    }
}

/// Diff output above this many bytes is truncated with a visible note, so
/// a generated file cannot fill the viewport or the frame budget.
const MAX_DIFF_BYTES: usize = 64 * 1024;

/// One workspace's change observations for the life of a run.
///
/// The baseline is captured once, when the workspace is opened, and every
/// later scan is compared against it. Observed writes accumulate for the
/// non-Git path and are also used to keep a Git list honest about which
/// entries Gritt touched.
pub struct WorkspaceChanges {
    root: PathBuf,
    git: Arc<dyn GitRunner>,
    /// Paths already changed when the session opened.
    baseline: RwLock<Option<BTreeSet<String>>>,
    /// Paths Gritt wrote in this run, with the status to report when Git
    /// is not available.
    observed: RwLock<BTreeMap<String, ChangeStatus>>,
    /// `false` when the workspace is not a Git repository or `git` cannot
    /// be run, which is what makes the list partial.
    is_repository: RwLock<bool>,
    /// Bounds how much blocking work this service can have in flight.
    ///
    /// Behind an `Arc` so a permit can be *owned by the worker* rather
    /// than by the future awaiting it. A caller that is cancelled drops
    /// the future, and a permit held there would be released while the
    /// blocking worker it was taken for is still running.
    scans: Arc<tokio::sync::Semaphore>,
}

impl WorkspaceChanges {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::with_git(root, Arc::new(SystemGit))
    }

    pub fn with_git(root: impl Into<PathBuf>, git: Arc<dyn GitRunner>) -> Self {
        Self {
            root: root.into(),
            git,
            baseline: RwLock::new(None),
            observed: RwLock::new(BTreeMap::new()),
            is_repository: RwLock::new(false),
            scans: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_SCANS)),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Captures the baseline. Anything already changed at this moment is
    /// reported as pre-existing for the rest of the run.
    ///
    /// Called once, when the workspace is opened. Calling it again would
    /// silently reclassify the session's own changes as pre-existing, so
    /// a second call is ignored.
    pub async fn capture_baseline(&self) {
        if self.baseline.read().await.is_some() {
            return;
        }
        match self.status().await {
            Some(entries) => {
                *self.is_repository.write().await = true;
                *self.baseline.write().await =
                    Some(entries.into_iter().map(|entry| entry.path).collect());
            }
            None => {
                *self.is_repository.write().await = false;
                *self.baseline.write().await = Some(BTreeSet::new());
            }
        }
    }

    /// Records a successful native write. This is the only source in a
    /// workspace with no repository, and the plan requires that list to be
    /// labelled partial.
    pub async fn record_write(&self, path: impl Into<String>) {
        let path = self.relative(&path.into());
        // `exists()` is a stat, and this is awaited by the event handler
        // after every turn. It goes to a blocking worker like the rest of
        // the filesystem work here.
        let target = self.root.join(&path);
        let existed = self
            .blocking(move || target.exists())
            .await
            .unwrap_or(false);
        let mut observed = self.observed.write().await;
        observed.entry(path).or_insert(if existed {
            ChangeStatus::Modified
        } else {
            ChangeStatus::Added
        });
    }

    /// The current change list. Git where it is available, observed writes
    /// where it is not.
    pub async fn scan(&self) -> ChangedFiles {
        let baseline = self.baseline.read().await.clone();
        if let Some(entries) = self.status().await {
            *self.is_repository.write().await = true;
            let baseline = baseline.unwrap_or_default();
            let files = entries
                .into_iter()
                .map(|entry| ChangedFile {
                    pre_existing: baseline.contains(&entry.path),
                    path: entry.path,
                    status: entry.status,
                })
                .collect();
            return ChangedFiles::Observed {
                source: ChangeSource::Git,
                files,
            };
        }
        *self.is_repository.write().await = false;
        let observed = self.observed.read().await;
        let baseline = baseline.unwrap_or_default();
        ChangedFiles::Observed {
            source: ChangeSource::ObservedWrites,
            files: observed
                .iter()
                .map(|(path, status)| ChangedFile {
                    path: path.clone(),
                    status: *status,
                    pre_existing: baseline.contains(path),
                })
                .collect(),
        }
    }

    /// A read-only unified diff for one file.
    ///
    /// Nothing here can modify the repository: worktree diff first, then
    /// the staged diff, then the content of a file Git does not track.
    pub async fn diff(&self, path: &str) -> FileDiff {
        let path = self.relative(path);
        if *self.is_repository.read().await {
            for args in [
                vec![
                    "diff".to_owned(),
                    "--no-color".to_owned(),
                    "--".to_owned(),
                    path.clone(),
                ],
                vec![
                    "diff".to_owned(),
                    "--no-color".to_owned(),
                    "--cached".to_owned(),
                    "--".to_owned(),
                    path.clone(),
                ],
            ] {
                if let Some(bytes) = self.git_bytes(args).await {
                    let text = String::from_utf8_lossy(&bytes).into_owned();
                    if !text.trim().is_empty() {
                        return FileDiff::Text {
                            path: path.clone(),
                            body: truncate_diff(&text),
                        };
                    }
                }
            }
        }
        // A file Git does not track is read for its content, which is a
        // blocking read and belongs on a blocking worker like the rest.
        let target = self.root.join(&path);
        let read = self
            .blocking(move || std::fs::read_to_string(&target))
            .await;
        match read {
            Some(Ok(content)) => {
                let body: String = content
                    .lines()
                    .map(|line| format!("+{line}\n"))
                    .collect::<String>();
                FileDiff::Text {
                    path,
                    body: truncate_diff(&body),
                }
            }
            Some(Err(error)) => FileDiff::Unavailable {
                path,
                reason: format!("cannot read this file: {error}"),
            },
            None => FileDiff::Unavailable {
                path,
                reason: "reading this file did not finish".into(),
            },
        }
    }

    /// `git status --porcelain=v1 -z`, or `None` when this is not a
    /// repository or `git` cannot be run. A non-zero exit is not an error
    /// to report: it means the same thing as a missing executable here.
    ///
    /// `-z` is not an optimisation. Without it Git quotes any path that is
    /// not plain ASCII under the default `core.quotePath`, and a filename
    /// containing ` -> ` is indistinguishable from a rename record. The
    /// NUL-separated form has neither problem: paths are literal bytes and
    /// a rename is two records.
    async fn status(&self) -> Option<Vec<StatusEntry>> {
        let bytes = self
            .git_bytes(vec![
                "status".to_owned(),
                "--porcelain=v1".to_owned(),
                "-z".to_owned(),
                "--untracked-files=all".to_owned(),
            ])
            .await?;
        Some(parse_status_z(&bytes))
    }

    /// Runs Git on a blocking worker. `std::process::Command::output`
    /// blocks until the child exits, so a slow repository would otherwise
    /// occupy a Tokio worker thread.
    async fn git_bytes(&self, args: Vec<String>) -> Option<Vec<u8>> {
        let git = Arc::clone(&self.git);
        let root = self.root.clone();
        let output = self
            .blocking(move || {
                let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
                git.run(&root, &borrowed)
            })
            .await?;
        let output = output.ok()?;
        if !output.status.success() {
            return None;
        }
        Some(output.stdout)
    }

    /// Runs one blocking closure under the concurrency bound. `None` means
    /// the worker did not finish, which a caller reports rather than
    /// treating as an answer.
    ///
    /// The permit is moved into the worker, so it is released when the
    /// work actually ends. Holding it in this future instead would let a
    /// cancelled caller hand the permit to the next request while its own
    /// worker was still running, and repeated cancel-and-retry would then
    /// exceed the bound the permit exists to enforce.
    async fn blocking<T, F>(&self, work: F) -> Option<T>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let permit = Arc::clone(&self.scans).acquire_owned().await.ok()?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            work()
        })
        .await
        .ok()
    }

    /// Paths are reported relative to the workspace root, whichever form
    /// the caller had.
    fn relative(&self, path: &str) -> String {
        Path::new(path)
            .strip_prefix(&self.root)
            .map(|rest| rest.to_string_lossy().into_owned())
            .unwrap_or_else(|_| path.to_owned())
    }
}

fn truncate_diff(text: &str) -> String {
    if text.len() <= MAX_DIFF_BYTES {
        return text.to_owned();
    }
    let mut cut = MAX_DIFF_BYTES;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}\n… truncated at {MAX_DIFF_BYTES} bytes\n", &text[..cut])
}

struct StatusEntry {
    path: String,
    status: ChangeStatus,
}

fn status_for(code: &str) -> ChangeStatus {
    match code {
        "??" => ChangeStatus::Untracked,
        c if c.contains('R') => ChangeStatus::Renamed,
        c if c.contains('C') => ChangeStatus::Renamed,
        c if c.contains('D') => ChangeStatus::Deleted,
        c if c.contains('A') => ChangeStatus::Added,
        _ => ChangeStatus::Modified,
    }
}

/// Parses `git status --porcelain=v1 -z`.
///
/// Each record is `XY <path>` terminated by NUL. A rename or copy is two
/// records: the status record carrying the **new** path, then a bare
/// record carrying the path it came from. Nothing is quoted or escaped in
/// this form, so a path is taken literally and a filename containing
/// ` -> ` is no longer mistaken for a rename.
fn parse_status_z(bytes: &[u8]) -> Vec<StatusEntry> {
    let mut out = Vec::new();
    let mut records = bytes
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .map(|record| String::from_utf8_lossy(record).into_owned());
    while let Some(record) = records.next() {
        if record.len() < 4 {
            continue;
        }
        let code = &record[..2];
        let path = record[3..].to_owned();
        let status = status_for(code);
        if status == ChangeStatus::Renamed {
            // The origin path follows as its own record and is consumed
            // here so it is never reported as a change of its own.
            let _origin = records.next();
        }
        out.push(StatusEntry { path, status });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;

    /// Decrements the live counter when the invocation returns.
    struct LiveGuard<'a>(&'a std::sync::atomic::AtomicUsize);

    impl Drop for LiveGuard<'_> {
        fn drop(&mut self) {
            self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    fn scopeguard(counter: &std::sync::atomic::AtomicUsize) -> LiveGuard<'_> {
        LiveGuard(counter)
    }

    /// A `git` that answers from a script, so the parsing and the
    /// baseline rule are testable without a repository.
    struct FakeGit {
        status: Mutex<Vec<String>>,
        diff: Option<String>,
        available: bool,
        /// How long each invocation blocks, for the concurrency bound.
        delay: Duration,
        /// Workers inside `run` right now, and the highest that has been.
        live: Arc<std::sync::atomic::AtomicUsize>,
        peak: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl FakeGit {
        fn new(statuses: &[&str]) -> Self {
            Self {
                status: Mutex::new(statuses.iter().map(|s| (*s).to_owned()).collect()),
                diff: None,
                available: true,
                delay: Duration::default(),
                live: Default::default(),
                peak: Default::default(),
            }
        }
    }

    impl GitRunner for FakeGit {
        fn run(&self, _root: &Path, args: &[&str]) -> std::io::Result<std::process::Output> {
            use std::os::unix::process::ExitStatusExt;
            use std::sync::atomic::Ordering;
            let live = self.live.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(live, Ordering::SeqCst);
            std::thread::sleep(self.delay);
            let _guard = scopeguard(&self.live);
            let ok = std::process::ExitStatus::from_raw(0);
            let fail = std::process::ExitStatus::from_raw(1 << 8);
            if !self.available {
                return Err(std::io::Error::other("no git"));
            }
            let stdout = if args[0] == "status" {
                let mut queue = self.status.lock().unwrap();
                if queue.len() > 1 {
                    queue.remove(0)
                } else {
                    queue.first().cloned().unwrap_or_default()
                }
            } else {
                match &self.diff {
                    Some(text) => text.clone(),
                    None => {
                        return Ok(std::process::Output {
                            status: fail,
                            stdout: Vec::new(),
                            stderr: Vec::new(),
                        })
                    }
                }
            };
            Ok(std::process::Output {
                status: ok,
                stdout: stdout.into_bytes(),
                stderr: Vec::new(),
            })
        }
    }

    /// `-z` records: `XY <path>` terminated by NUL, no trailing newline.
    fn z(records: &[&str]) -> String {
        records.iter().map(|record| format!("{record}\0")).collect()
    }

    #[tokio::test]
    async fn changes_present_at_open_are_labelled_pre_existing() {
        let git = Arc::new(FakeGit::new(&[
            &z(&[" M src/lib.rs"]),
            &z(&[" M src/lib.rs", "?? notes.md", "A  added.rs"]),
        ]));
        let changes = WorkspaceChanges::with_git("/tmp/ws", git);
        changes.capture_baseline().await;
        let ChangedFiles::Observed { source, files } = changes.scan().await else {
            panic!("a repository must produce a list");
        };
        assert_eq!(source, ChangeSource::Git);
        assert_eq!(files.len(), 3);
        let by_path = |name: &str| files.iter().find(|f| f.path == name).unwrap().clone();
        assert!(by_path("src/lib.rs").pre_existing);
        assert!(!by_path("notes.md").pre_existing);
        assert_eq!(by_path("notes.md").status, ChangeStatus::Untracked);
        assert_eq!(by_path("added.rs").status, ChangeStatus::Added);
    }

    #[tokio::test]
    async fn a_workspace_without_git_reports_observed_writes_as_partial() {
        let git = Arc::new(FakeGit {
            status: Mutex::new(Vec::new()),
            diff: None,
            available: false,
            delay: Duration::default(),
            live: Default::default(),
            peak: Default::default(),
        });
        let changes = WorkspaceChanges::with_git("/tmp/ws", git);
        changes.capture_baseline().await;
        changes.record_write("/tmp/ws/notes.txt").await;
        let ChangedFiles::Observed { source, files } = changes.scan().await else {
            panic!("observed writes are still a list");
        };
        assert_eq!(source, ChangeSource::ObservedWrites);
        assert_eq!(source.caveat(), Some("partial: observed writes only"));
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "notes.txt");
    }

    /// A rename is two `-z` records, new path first, and the origin record
    /// is consumed rather than reported as a change of its own.
    #[test]
    fn a_rename_reports_the_path_that_exists_on_disk() {
        let entries =
            parse_status_z(z(&["R  new/name.rs", "old/name.rs", "D  gone.rs"]).as_bytes());
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, "new/name.rs");
        assert_eq!(entries[0].status, ChangeStatus::Renamed);
        assert_eq!(entries[1].path, "gone.rs");
        assert_eq!(entries[1].status, ChangeStatus::Deleted);
    }

    /// The two cases the line-based parser got wrong: a path Git would
    /// have quoted under `core.quotePath`, and an ordinary filename that
    /// contains the rename separator.
    #[test]
    fn unicode_paths_and_paths_containing_the_rename_arrow_survive() {
        let entries = parse_status_z(
            z(&[
                " M src/世界.rs",
                "?? notes -> draft.md",
                "?? \"already quoted\".md",
            ])
            .as_bytes(),
        );
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].path, "src/世界.rs");
        assert_eq!(
            entries[1].path, "notes -> draft.md",
            "an ordinary filename was mistaken for a rename record"
        );
        assert_eq!(
            entries[2].path, "\"already quoted\".md",
            "quotes that are part of the name were stripped"
        );
    }

    /// Round 2, finding 3: a cancelled caller must not hand its permit to
    /// the next request while its own blocking worker is still running.
    ///
    /// The permit belongs to the worker, so cancelling and retrying cannot
    /// push more than `MAX_CONCURRENT_SCANS` invocations into `git` at
    /// once. With the permit held by the awaiting future instead, each
    /// cancellation released one early and this peaks well above the
    /// bound.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn cancelling_a_scan_does_not_release_its_worker_s_permit() {
        let peak: Arc<std::sync::atomic::AtomicUsize> = Default::default();
        let git = Arc::new(FakeGit {
            status: Mutex::new(vec![String::new()]),
            diff: None,
            available: true,
            delay: Duration::from_millis(120),
            live: Default::default(),
            peak: Arc::clone(&peak),
        });
        let changes = Arc::new(WorkspaceChanges::with_git("/tmp/ws", git));
        // Six requests, each abandoned before its worker can finish.
        for _ in 0..6 {
            let changes = Arc::clone(&changes);
            let _ = tokio::time::timeout(
                Duration::from_millis(10),
                async move { changes.scan().await },
            )
            .await;
        }
        // Let every started worker drain.
        tokio::time::sleep(Duration::from_millis(400)).await;
        let peak = peak.load(std::sync::atomic::Ordering::SeqCst);
        assert!(
            peak <= MAX_CONCURRENT_SCANS,
            "cancellation let {peak} workers run at once, above the bound of {MAX_CONCURRENT_SCANS}"
        );
        // The service still works afterwards: no permit was lost either.
        let done = tokio::time::timeout(Duration::from_secs(5), changes.scan()).await;
        assert!(done.is_ok(), "the concurrency bound was never released");
    }

    #[tokio::test]
    async fn the_baseline_is_captured_once_so_gritts_own_writes_stay_visible() {
        let git = Arc::new(FakeGit::new(&["", &z(&[" M src/lib.rs"])]));
        let changes = WorkspaceChanges::with_git("/tmp/ws", git);
        changes.capture_baseline().await;
        // A second call must not adopt the session's own change as the
        // baseline; the file stays reported as this run's work.
        changes.capture_baseline().await;
        let files = changes.scan().await.files().to_vec();
        assert_eq!(files.len(), 1);
        assert!(!files[0].pre_existing);
    }
}
