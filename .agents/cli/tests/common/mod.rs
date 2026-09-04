#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

pub struct Fixture {
    _dir: TempDir,
    pub root: PathBuf,
}

pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

pub fn expected(name: &str) -> String {
    fs::read_to_string(fixtures_dir().join("expected").join(name))
        .unwrap_or_else(|error| panic!("missing expected fixture {name}: {error}"))
}

pub fn copy_dir(source: &Path, target: &Path) {
    fs::create_dir_all(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let destination = target.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &destination);
        } else {
            fs::copy(entry.path(), destination).unwrap();
        }
    }
}

/// Copies the fixture repository into a temporary directory.
pub fn fixture() -> Fixture {
    let dir = tempfile::Builder::new()
        .prefix("gritt-agent-")
        .tempdir()
        .unwrap();
    let root = dir.path().canonicalize().unwrap();
    copy_dir(&fixtures_dir().join("repo"), &root);
    Fixture { _dir: dir, root }
}

/// An empty repository with only `.agents/tasks/`.
pub fn empty_repo() -> Fixture {
    let dir = tempfile::Builder::new()
        .prefix("gritt-agent-")
        .tempdir()
        .unwrap();
    let root = dir.path().canonicalize().unwrap();
    fs::create_dir_all(root.join(".agents").join("tasks")).unwrap();
    Fixture { _dir: dir, root }
}

pub struct Run {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Run {
    pub fn assert_status(&self, expected: i32) -> &Self {
        assert_eq!(
            self.status, expected,
            "unexpected exit status\nstdout:\n{}\nstderr:\n{}",
            self.stdout, self.stderr
        );
        self
    }
}

pub fn command(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_gritt-agent"));
    command.arg("--repo-root").arg(root);
    command.env_remove("GRITT_TKT_NAMESPACE");
    command
}

pub fn run(root: &Path, args: &[&str]) -> Run {
    let output: Output = command(root).args(args).output().unwrap();
    Run {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

pub fn read(root: &Path, relative: &str) -> String {
    fs::read_to_string(root.join(relative))
        .unwrap_or_else(|error| panic!("cannot read {relative}: {error}"))
}

pub fn write(root: &Path, relative: &str, content: &str) {
    let target = root.join(relative);
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(target, content).unwrap();
}

pub fn ticket_task(id: &str, namespace: Option<&str>, extra: &str) -> String {
    let namespace_line = namespace
        .map(|n| format!("namespace: {n}\n"))
        .unwrap_or_default();
    format!(
        "---\nid: {id}\n{namespace_line}title: Test ticket {id}\nartifact: task\nstatus: ready\nowner: test\ncreated: 2026-08-14\nupdated: 2026-08-14\n{extra}---\n\n# {id}\n"
    )
}
