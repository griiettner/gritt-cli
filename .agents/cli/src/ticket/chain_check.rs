//! `gritt-agent ticket chain-check`: deterministic branch and ticket checks
//! the chain reviewer runs before a semantic review.

use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

use regex::Regex;

use super::identity::{resolve_ticket_identity, ResolveOptions};
use super::store::find_ticket_dir_with;
use crate::frontmatter;
use crate::fsx::{self, relative_posix};
use crate::repo::tasks_root;
use crate::Result;

const REQUIRED_REPORT_SECTIONS: [&str; 3] = ["## Summary", "## Validation", "## Completion Gate"];

fn ticket_path_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\.agents/tasks/(?:[A-Za-z0-9._-]+/)?TKT-\d{4}-\d{4}/TKT-\d{4}/").unwrap()
    })
}

fn benchmark_hint_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\bbenchmark|\bbench\b").unwrap())
}

#[derive(Debug, Clone, Default)]
pub struct ChainCheckOptions {
    pub ticket: String,
    pub base: String,
    pub require_report: bool,
    pub require_benchmark: bool,
}

#[derive(Debug, Default)]
pub struct Outcome {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub notes: Vec<String>,
}

impl Outcome {
    fn error(&mut self, message: impl Into<String>) {
        self.errors.push(message.into());
    }

    fn warn(&mut self, message: impl Into<String>) {
        self.warnings.push(message.into());
    }

    fn note(&mut self, message: impl Into<String>) {
        self.notes.push(message.into());
    }
}

pub fn run(repo_root: &Path, options: &ChainCheckOptions) -> Result<i32> {
    let mut outcome = Outcome::default();
    let tasks = tasks_root(repo_root);
    // The developer's namespace only breaks ties between same-numbered
    // tickets, so the identity lookup (which may call `gh`) runs only then.
    let ticket = find_ticket_dir_with(&tasks, &options.ticket, || {
        resolve_ticket_identity(
            repo_root,
            &ResolveOptions {
                namespace: None,
                refresh: false,
                persist: false,
            },
        )
        .ok()
        .map(|identity| identity.github_login)
    })?;
    check_ticket_artifacts(&ticket.dir, &mut outcome, options.require_report);

    let Some(git_root) = git(repo_root, &["rev-parse", "--show-toplevel"], &mut outcome) else {
        return Ok(finish(&outcome));
    };
    let git_root = Path::new(&git_root);
    outcome.note(format!("project root: {}", repo_root.display()));
    outcome.note(format!("git root: {}", git_root.display()));

    let branch = git(
        git_root,
        &["rev-parse", "--abbrev-ref", "HEAD"],
        &mut outcome,
    );
    let base_sha = git(git_root, &["rev-parse", &options.base], &mut outcome);
    let head_sha = git(git_root, &["rev-parse", "HEAD"], &mut outcome);
    let merge_base = git(
        git_root,
        &["merge-base", &options.base, "HEAD"],
        &mut outcome,
    );
    if let Some(branch) = &branch {
        outcome.note(format!("current branch: {branch}"));
        if *branch == options.base {
            outcome.warn(format!(
                "current branch is the base branch `{}`; reviewer likely expects a worker branch",
                options.base
            ));
        }
    }
    if let (Some(base_sha), Some(head_sha)) = (&base_sha, &head_sha) {
        outcome.note(format!(
            "base branch `{}` sha: {}",
            options.base,
            short_sha(base_sha)
        ));
        outcome.note(format!("head sha: {}", short_sha(head_sha)));
    }
    if let (Some(merge_base), Some(base_sha)) = (&merge_base, &base_sha) {
        if merge_base != base_sha {
            outcome.warn(format!(
                "HEAD is not based on the current tip of `{}` (merge-base {} != base {})",
                options.base,
                short_sha(merge_base),
                short_sha(base_sha)
            ));
        }
    }
    let range = format!("{}...HEAD", options.base);
    if let Some(changed) = git(git_root, &["diff", "--name-only", &range], &mut outcome) {
        let changed_files: Vec<&str> = changed
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        outcome.note(format!(
            "changed files against `{}`: {}",
            options.base,
            changed_files.len()
        ));
        for file in &changed_files {
            outcome.note(format!("  - {file}"));
        }
        let own_folder = format!(".agents/tasks/{}/", relative_posix(&tasks, &ticket.dir));
        check_changed_files(&changed_files, &own_folder, &mut outcome);
    }
    if options.require_benchmark || benchmark_expected(&ticket.dir) {
        check_benchmark_evidence(&ticket.dir, &mut outcome);
    }
    Ok(finish(&outcome))
}

fn short_sha(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

fn check_ticket_artifacts(ticket_dir: &Path, outcome: &mut Outcome, require_report: bool) {
    if !fsx::is_dir(ticket_dir) {
        outcome.error(format!(
            "ticket folder does not exist: {}",
            ticket_dir.display()
        ));
        return;
    }
    let task_path = ticket_dir.join("task.md");
    let report_path = ticket_dir.join("report.md");
    if !fsx::exists(&task_path) {
        outcome.error(format!("missing task.md: {}", task_path.display()));
    } else {
        let parsed = frontmatter::load(&task_path);
        for error in &parsed.errors {
            outcome.error(error.render());
        }
        if parsed.metadata.is_empty() {
            outcome.error(format!(
                "task.md missing YAML frontmatter: {}",
                task_path.display()
            ));
        }
    }
    if !fsx::exists(&report_path) {
        let message = format!("missing report.md: {}", report_path.display());
        if require_report {
            outcome.error(message);
        } else {
            outcome.warn(message);
        }
        return;
    }
    let parsed = frontmatter::load(&report_path);
    for error in &parsed.errors {
        outcome.error(error.render());
    }
    if parsed.metadata.is_empty() {
        outcome.error(format!(
            "report.md missing YAML frontmatter: {}",
            report_path.display()
        ));
    }
    let content = match fsx::read_text(&report_path) {
        Ok(content) => content,
        Err(error) => {
            outcome.error(format!("cannot read report.md: {}", error.message));
            return;
        }
    };
    for section in REQUIRED_REPORT_SECTIONS {
        if !content.contains(section) {
            outcome.warn(format!("report.md missing section `{section}`"));
        }
    }
}

fn benchmark_expected(ticket_dir: &Path) -> bool {
    let task_path = ticket_dir.join("task.md");
    if !fsx::exists(&task_path) {
        return false;
    }
    fsx::read_text(&task_path)
        .map(|content| benchmark_hint_regex().is_match(&content))
        .unwrap_or(false)
}

fn check_benchmark_evidence(ticket_dir: &Path, outcome: &mut Outcome) {
    let report_path = ticket_dir.join("report.md");
    if !fsx::exists(&report_path) {
        outcome.warn("benchmark expected but report.md is missing");
        return;
    }
    match fsx::read_text(&report_path) {
        Ok(content) => {
            if !benchmark_hint_regex().is_match(&content) {
                outcome.warn("benchmark expected but no benchmark evidence was found in report.md");
            }
        }
        Err(error) => outcome.error(format!(
            "cannot read report.md for benchmark check: {}",
            error.message
        )),
    }
}

/// Flags changed files under any ticket folder other than `own_folder`, the
/// checked ticket's `.agents/tasks/...` path with a trailing slash. Matching
/// the whole folder path keeps a same-numbered ticket in another namespace
/// from passing as this one.
fn check_changed_files(changed_files: &[&str], own_folder: &str, outcome: &mut Outcome) {
    if changed_files.is_empty() {
        outcome.warn("no changed files detected against base branch");
        return;
    }
    let other_tickets: Vec<&&str> = changed_files
        .iter()
        .filter(|file| ticket_path_regex().is_match(file) && !file.contains(own_folder))
        .collect();
    if !other_tickets.is_empty() {
        outcome.warn("changed files include other ticket folders:");
        for file in other_tickets {
            outcome.warn(format!("  - {file}"));
        }
    }
    if changed_files.contains(&".agents/tasks/backlog.yaml") {
        outcome.note("backlog.yaml changed; verify this was intentional");
    }
}

/// Runs `git` in `cwd` and returns trimmed stdout, recording an error on
/// any failure including a missing binary.
fn git(cwd: &Path, args: &[&str], outcome: &mut Outcome) -> Option<String> {
    let joined = args.join(" ");
    match Command::new("git").args(args).current_dir(cwd).output() {
        Ok(output) if output.status.success() => {
            Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            let detail = if !stderr.is_empty() {
                stderr
            } else if !stdout.is_empty() {
                stdout
            } else {
                format!("exit {}", output.status.code().unwrap_or(1))
            };
            outcome.error(format!("git {joined} failed: {detail}"));
            None
        }
        Err(error) => {
            outcome.error(format!("git {joined} failed: {error}"));
            None
        }
    }
}

fn finish(outcome: &Outcome) -> i32 {
    for note in &outcome.notes {
        println!("NOTE: {note}");
    }
    for warning in &outcome.warnings {
        println!("WARN: {warning}");
    }
    for error in &outcome.errors {
        eprintln!("ERROR: {error}");
    }
    if !outcome.errors.is_empty() {
        eprintln!(
            "tkt_chain_check failed ({} error(s), {} warning(s))",
            outcome.errors.len(),
            outcome.warnings.len()
        );
        return 1;
    }
    println!("tkt_chain_check ok ({} warning(s))", outcome.warnings.len());
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn other_ticket_folders_are_flagged_but_own_folder_is_not() {
        let mut outcome = Outcome::default();
        check_changed_files(
            &[
                ".agents/tasks/alice/TKT-0001-0025/TKT-0002/task.md",
                ".agents/tasks/TKT-0001-0025/TKT-0009/report.md",
                ".agents/tasks/bob/TKT-0001-0025/TKT-0002/task.md",
                ".agents/tasks/backlog.yaml",
                "src/lib.rs",
            ],
            ".agents/tasks/alice/TKT-0001-0025/TKT-0002/",
            &mut outcome,
        );
        assert_eq!(
            outcome.warnings,
            vec![
                "changed files include other ticket folders:",
                "  - .agents/tasks/TKT-0001-0025/TKT-0009/report.md",
                "  - .agents/tasks/bob/TKT-0001-0025/TKT-0002/task.md"
            ]
        );
        assert_eq!(
            outcome.notes,
            vec!["backlog.yaml changed; verify this was intentional"]
        );
    }

    #[test]
    fn benchmark_hint_matches_words_only() {
        assert!(benchmark_hint_regex().is_match("Run the Benchmark suite"));
        assert!(benchmark_hint_regex().is_match("see bench results"));
        assert!(!benchmark_hint_regex().is_match("the workbench is clean"));
        assert_eq!(short_sha("0123456789abcdef"), "0123456789ab");
    }
}
