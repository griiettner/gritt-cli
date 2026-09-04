mod common;

use std::fs;
use std::path::Path;

use common::{empty_repo, Run};
use gritt_agent::codex::trust::project_header;

fn trust(root: &Path, codex_home: &Path, args: &[&str]) -> Run {
    let output = common::command(root)
        .env("CODEX_HOME", codex_home)
        .args(["codex", "trust"])
        .args(args)
        .output()
        .unwrap();
    Run {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

#[test]
fn trust_checks_adds_and_reports_an_existing_entry() {
    let repo = empty_repo();
    let codex_home = tempfile::tempdir().unwrap();
    let config = codex_home.path().join("config.toml");
    let root = repo.root.display();
    // The header escapes backslashes, so on Windows it differs from `root`.
    let header = project_header(&repo.root);

    let check = trust(&repo.root, codex_home.path(), &["--check"]);
    check.assert_status(1);
    assert_eq!(
        check.stdout,
        format!("not trusted: {root}\nconfig: {}\n", config.display())
    );
    assert!(!config.exists());

    let added = trust(&repo.root, codex_home.path(), &[]);
    added.assert_status(0);
    assert_eq!(
        added.stdout,
        format!(
            "trusted: {root}\nconfig: {}\nrestart required: start a fresh Codex session at this repository root\n",
            config.display()
        )
    );
    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        format!("{header}\ntrust_level = \"trusted\"\n")
    );

    let again = trust(&repo.root, codex_home.path(), &[]);
    again.assert_status(0);
    assert_eq!(
        again.stdout,
        format!("already trusted: {root}\nconfig: {}\n", config.display())
    );
    let check = trust(&repo.root, codex_home.path(), &["--check"]);
    check.assert_status(0);
    assert_eq!(check.stdout, format!("trusted: {root}\n"));
}

#[test]
fn trust_accepts_a_path_and_edits_existing_sections_in_place() {
    let repo = empty_repo();
    let codex_home = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let other_header = project_header(other.path());
    let config = codex_home.path().join("config.toml");
    fs::write(
        &config,
        format!(
            "model = \"x\"\n\n{other_header}\ntrust_level = \"untrusted\"\n\n[tui]\ntheme = \"dark\"\n"
        ),
    )
    .unwrap();

    let other_path = other.path().to_str().unwrap();
    trust(&repo.root, codex_home.path(), &["--check", other_path]).assert_status(1);
    trust(&repo.root, codex_home.path(), &[other_path]).assert_status(0);
    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        format!(
            "model = \"x\"\n\n{other_header}\ntrust_level = \"trusted\"\n\n[tui]\ntheme = \"dark\"\n"
        )
    );
    trust(&repo.root, codex_home.path(), &["--check", other_path]).assert_status(0);

    // The default target appends a new section after a blank line.
    trust(&repo.root, codex_home.path(), &[]).assert_status(0);
    assert!(fs::read_to_string(&config).unwrap().ends_with(&format!(
        "theme = \"dark\"\n\n{}\ntrust_level = \"trusted\"\n",
        project_header(&repo.root)
    )));
}

#[test]
fn trust_expands_a_home_relative_codex_home() {
    let repo = empty_repo();
    let home = tempfile::tempdir().unwrap();
    let output = common::command(&repo.root)
        .env("HOME", home.path())
        .env("CODEX_HOME", "~/codex-test")
        .args(["codex", "trust"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(home.path().join("codex-test/config.toml").exists());
}
