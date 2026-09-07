//! Owner detection fixtures, fake package managers, and the update
//! runner for connector version checks. Every fixture is a scratch
//! directory; nothing here depends on what the host has installed.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gritt_connector::install::{detect_install_source, update_action, InstallEnv, VendorInstall};
use gritt_connector::process::is_alive;
use gritt_connector::protocols::claude::ClaudeCode;
use gritt_connector::protocols::codex::Codex;
use gritt_connector::protocols::opencode::OpenCode;
use gritt_connector::versions::ConnectorVersionCache;
use gritt_connector::{ExternalConnector, Protocol, Timeouts};
use gritt_core::config::{ConnectorSettings, ModelListPolicy};
use gritt_core::connector::{
    Connector, ConnectorUpdateOutcome, ConnectorVersionCheck, InstallSource, VersionCheckFailure,
    VersionCheckMode, VersionComparison, VersionFreshness,
};
use gritt_core::secret::Secret;

fn agent_script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake-agent/agent.sh")
}

fn write_script(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!("#!/bin/sh\n{body}")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A wrapper at `path` that exports `vars` and runs the fake agent.
fn fake_agent(path: &Path, vars: &[(&str, String)]) {
    let mut body = String::new();
    for (name, value) in vars {
        body.push_str(&format!(
            "{name}='{}'\nexport {name}\n",
            value.replace('\'', "'\\''")
        ));
    }
    body.push_str(&format!("exec '{}' \"$@\"\n", agent_script().display()));
    write_script(path, &body);
}

/// A fake package manager that records its arguments and prints `stdout`.
fn fake_manager(path: &Path, args_file: &Path, stdout: &str, exit: i32, stderr: &str) {
    let root_probe = if path.file_name().unwrap() == "npm" {
        format!(
            "if [ \"$1\" = root ]; then echo '{}'; exit 0; fi\n",
            path.parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("lib/node_modules")
                .display()
        )
    } else {
        String::new()
    };
    let body = format!(
        "{root_probe}: > '{args}'\nfor arg in \"$@\"; do printf '%s\\n' \"$arg\" >> '{args}'; done\n\
         if [ -n '{stderr}' ]; then echo '{stderr}' >&2; fi\n\
         cat <<'FAKE_EOF'\n{stdout}\nFAKE_EOF\nexit {exit}\n",
        args = args_file.display(),
    );
    write_script(path, &body);
}

fn recorded_args(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn settings(name: &str, exe: &Path) -> ConnectorSettings {
    ConnectorSettings {
        executables: BTreeMap::from([(name.to_owned(), exe.display().to_string())]),
        health_check_timeout_secs: Some(5),
        task_timeout_secs: Some(2),
        ..ConnectorSettings::default()
    }
}

fn policy() -> ModelListPolicy {
    ModelListPolicy {
        refresh_interval_secs: 24 * 60 * 60,
        stale_fallback: true,
    }
}

fn connector<P: Protocol>(
    protocol: P,
    name: &str,
    exe: &Path,
    home: &Path,
    managers: BTreeMap<String, PathBuf>,
    cache: Option<&Path>,
) -> ExternalConnector<P> {
    let mut connector = ExternalConnector::new(protocol, &settings(name, exe))
        .with_timeouts(Timeouts {
            health: Duration::from_secs(5),
            startup: Duration::from_secs(5),
            idle: Duration::from_secs(2),
        })
        .with_install_env(InstallEnv {
            home: Some(home.to_path_buf()),
            cargo_home: Some(home.join(".cargo")),
        })
        .with_manager_programs(managers);
    if let Some(dir) = cache {
        connector = connector.with_version_cache(ConnectorVersionCache::new(dir), policy());
    }
    connector
}

const OPENCODE_VENDOR: VendorInstall = VendorInstall {
    installer: "OpenCode install script",
    markers: &[".opencode/bin/"],
    update_args: &["upgrade"],
};

fn env(home: &Path) -> InstallEnv {
    InstallEnv {
        home: Some(home.to_path_buf()),
        cargo_home: Some(home.join(".cargo")),
    }
}

#[tokio::test]
async fn probe_timeout_stops_descendants() {
    let root = tempfile::tempdir().unwrap();
    let exe = root.path().join("probe");
    let pid_file = root.path().join("descendant.pid");
    write_script(
        &exe,
        &format!("sleep 30 &\necho $! > '{}'\nwait\n", pid_file.display()),
    );
    assert!(
        gritt_connector::health::probe(&exe, &[], Duration::from_secs(2))
            .await
            .is_err()
    );
    let pid: u32 = std::fs::read_to_string(pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let alive = is_alive(pid).await;
    if alive {
        gritt_connector::process::kill_tree(pid).await;
    }
    assert!(!alive, "a timed-out probe left its descendant running");
}

#[tokio::test]
async fn failed_updates_do_not_retain_package_manager_content() {
    let fx = vendor_fixture(&[]);
    write_script(&fx.exe, "if [ \"$1\" = --version ]; then echo 1.0.0; exit 0; fi\necho 'registry failure https://fixture-user:fixture-password@registry.invalid/' >&2\nexit 1\n");
    let connector = vendor_connector(&fx);
    let action = connector
        .check_version(VersionCheckMode::Refresh)
        .await
        .status()
        .unwrap()
        .update
        .clone()
        .unwrap();
    let outcome = connector.update(action).await;
    let ConnectorUpdateOutcome::Failed { reason, output, .. } = outcome else {
        panic!("expected failure")
    };
    assert!(reason.contains("status 1"));
    assert!(
        output.is_empty(),
        "package-manager content must stay out of diagnostics"
    );
}

#[tokio::test]
async fn a_local_npm_install_never_offers_a_global_update() {
    let fx = npm_fixture("1.0.0", "2.0.0", 0, "");
    let package = fx._root.path().join("project/node_modules/@openai/codex");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(package.join("package.json"), "{}").unwrap();
    let exe = package.join("bin/codex");
    fake_agent(&exe, &[]);
    let check = connector(
        Codex,
        "codex",
        &exe,
        &fx.home,
        BTreeMap::from([("npm".into(), fx.npm.clone())]),
        None,
    )
    .check_version(VersionCheckMode::Refresh)
    .await;
    assert!(
        check.status().unwrap().update.is_none(),
        "a local package must not be updated globally"
    );
    assert!(check.status().unwrap().next_step.is_some());
}

#[test]
fn a_globally_linked_local_package_is_not_replaced_by_an_update() {
    let root = tempfile::tempdir().unwrap();
    let global_root = root.path().join("lib/node_modules");
    let local = root.path().join("project/node_modules/fixture-cli");
    let exe = local.join("bin/cli");
    write_script(&exe, "exit 0\n");
    std::fs::create_dir_all(&global_root).unwrap();
    symlink(&local, global_root.join("fixture-cli")).unwrap();
    let action = gritt_connector::install::npm_update_action(
        &InstallSource::Npm {
            package: "fixture-cli".into(),
        },
        &exe,
        &root.path().join("bin/npm"),
        &global_root,
    );
    assert!(
        action.is_none(),
        "a global link does not make the local package a global installation"
    );
}

#[tokio::test]
async fn changing_installation_source_does_not_reuse_a_fresh_version() {
    let fx = npm_fixture("1.0.0", "2.0.0", 0, "");
    npm_connector(&fx)
        .check_version(VersionCheckMode::Refresh)
        .await;
    let exe = fx._root.path().join("Cellar/codex/1.0.0/bin/codex");
    fake_agent(&exe, &[]);
    let brew = fx._root.path().join("managers/brew");
    let args = fx._root.path().join("brew-args");
    fake_manager(
        &brew,
        &args,
        r#"{"formulae":[{"name":"codex","versions":{"stable":"3.0.0"}}]}"#,
        0,
        "",
    );
    let check = connector(
        Codex,
        "codex",
        &exe,
        &fx.home,
        BTreeMap::from([("brew".into(), brew)]),
        Some(&fx.cache),
    )
    .check_version(VersionCheckMode::Cached)
    .await;
    assert_eq!(check.status().unwrap().latest.as_deref(), Some("3.0.0"));
    assert!(args.exists(), "a new install source must be checked");
}

// -- owner detection -------------------------------------------------------

#[test]
fn homebrew_is_detected_from_the_cellar_or_caskroom_behind_the_symlink() {
    let root = tempfile::tempdir().unwrap();
    let formula = root.path().join("Cellar/opencode/1.18.29/bin/opencode");
    write_script(&formula, "exit 0\n");
    let link = root.path().join("bin/opencode");
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    symlink(&formula, &link).unwrap();
    assert_eq!(
        detect_install_source(&link, &env(root.path()), None),
        InstallSource::Homebrew {
            name: "opencode".into(),
            cask: false
        }
    );
    let cask = root.path().join("Caskroom/codex/0.153.4/bin/codex");
    write_script(&cask, "exit 0\n");
    let source = detect_install_source(&cask, &env(root.path()), None);
    assert_eq!(
        source,
        InstallSource::Homebrew {
            name: "codex".into(),
            cask: true
        }
    );
    let action = update_action(&source, &cask, None).unwrap();
    assert_eq!(action.program, "brew");
    assert_eq!(action.args, vec!["upgrade", "--cask", "codex"]);
}

#[test]
fn npm_needs_the_package_manifest_and_reads_a_scoped_name() {
    let root = tempfile::tempdir().unwrap();
    let package = root
        .path()
        .join("lib/node_modules/@anthropic-ai/claude-code");
    let bin = package.join("bin/claude");
    write_script(&bin, "exit 0\n");
    let link = root.path().join("bin/claude");
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    symlink(&bin, &link).unwrap();
    assert_eq!(
        detect_install_source(&link, &env(root.path()), None),
        InstallSource::Unknown,
        "a node_modules path without package.json is not evidence"
    );
    std::fs::write(
        package.join("package.json"),
        r#"{"name":"@anthropic-ai/claude-code"}"#,
    )
    .unwrap();
    let source = detect_install_source(&link, &env(root.path()), None);
    assert_eq!(
        source,
        InstallSource::Npm {
            package: "@anthropic-ai/claude-code".into()
        }
    );
    assert!(
        update_action(&source, &link, None).is_none(),
        "a manifest alone does not prove global ownership"
    );
    let manager = root.path().join("bin/npm");
    let action = gritt_connector::install::npm_update_action(
        &source,
        &link,
        &manager,
        &root.path().join("lib/node_modules"),
    )
    .unwrap();
    assert_eq!(
        action.args,
        vec![
            "install".to_owned(),
            "-g".into(),
            "--prefix".into(),
            std::fs::canonicalize(root.path())
                .unwrap()
                .display()
                .to_string(),
            "@anthropic-ai/claude-code@latest".into()
        ]
    );
}

#[test]
fn pipx_needs_its_venv_metadata() {
    let home = tempfile::tempdir().unwrap();
    let venv = home.path().join(".local/pipx/venvs/tool");
    let bin = venv.join("bin/tool");
    write_script(&bin, "exit 0\n");
    assert_eq!(
        detect_install_source(&bin, &env(home.path()), None),
        InstallSource::Unknown
    );
    std::fs::write(venv.join("pipx_metadata.json"), "{}").unwrap();
    let source = detect_install_source(&bin, &env(home.path()), None);
    assert_eq!(
        source,
        InstallSource::Pipx {
            package: "tool".into()
        }
    );
    assert_eq!(
        update_action(&source, &bin, None).unwrap().args,
        vec!["upgrade", "tool"]
    );
}

#[test]
fn cargo_needs_a_crates_toml_entry_naming_the_binary() {
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join(".cargo/bin/tool");
    write_script(&bin, "exit 0\n");
    assert_eq!(
        detect_install_source(&bin, &env(home.path()), None),
        InstallSource::Unknown
    );
    std::fs::write(
        home.path().join(".cargo/.crates.toml"),
        "[v1]\n\"other 1.0.0 (registry+x)\" = [\"other\"]\n\
         \"tool-crate 0.3.0 (registry+x)\" = [\"tool\", \"tool-extra\"]\n",
    )
    .unwrap();
    let source = detect_install_source(&bin, &env(home.path()), None);
    assert_eq!(
        source,
        InstallSource::Cargo {
            crate_name: "tool-crate".into()
        }
    );
    assert_eq!(
        update_action(&source, &bin, None).unwrap().args,
        vec!["install", "tool-crate"]
    );
}

#[test]
fn vendor_installs_are_matched_under_the_home_directory_only() {
    let home = tempfile::tempdir().unwrap();
    let exe = home.path().join(".opencode/bin/opencode");
    write_script(&exe, "exit 0\n");
    let source = detect_install_source(&exe, &env(home.path()), Some(&OPENCODE_VENDOR));
    assert_eq!(
        source,
        InstallSource::Vendor {
            installer: "OpenCode install script".into()
        }
    );
    let action = update_action(&source, &exe, Some(&OPENCODE_VENDOR)).unwrap();
    assert_eq!(action.program, exe.display().to_string());
    assert_eq!(action.args, vec!["upgrade"]);
    let elsewhere = tempfile::tempdir().unwrap();
    let other = elsewhere.path().join(".opencode/bin/opencode");
    write_script(&other, "exit 0\n");
    assert_eq!(
        detect_install_source(&other, &env(home.path()), Some(&OPENCODE_VENDOR)),
        InstallSource::Unknown
    );
}

#[test]
fn two_owners_with_evidence_are_ambiguous_and_offer_nothing() {
    let home = tempfile::tempdir().unwrap();
    let package = home
        .path()
        .join(".claude/local/node_modules/@anthropic-ai/claude-code");
    let bin = package.join("bin/claude");
    write_script(&bin, "exit 0\n");
    std::fs::write(package.join("package.json"), "{}").unwrap();
    let vendor = ClaudeCode.vendor_install().unwrap();
    let source = detect_install_source(&bin, &env(home.path()), Some(&vendor));
    let InstallSource::Ambiguous { candidates } = &source else {
        panic!("expected ambiguous, got {source:?}");
    };
    assert_eq!(candidates.len(), 2, "{candidates:?}");
    assert!(update_action(&source, &bin, Some(&vendor)).is_none());
    assert!(gritt_connector::install::next_step(&source, &bin)
        .unwrap()
        .contains("could belong to"));
}

// -- version checks through the connector ----------------------------------

struct NpmFixture {
    _root: tempfile::TempDir,
    exe: PathBuf,
    home: PathBuf,
    npm: PathBuf,
    args: PathBuf,
    cache: PathBuf,
}

/// Codex as an npm global install, with a fake `npm` that answers `view`.
fn npm_fixture(installed: &str, latest_stdout: &str, exit: i32, stderr: &str) -> NpmFixture {
    let root = tempfile::tempdir().unwrap();
    let package = root.path().join("lib/node_modules/@openai/codex");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(package.join("package.json"), "{}").unwrap();
    let version_file = root.path().join("version.txt");
    std::fs::write(&version_file, format!("{installed}\n")).unwrap();
    let exe = package.join("bin/codex");
    fake_agent(
        &exe,
        &[(
            "FAKE_AGENT_VERSION_FILE",
            version_file.display().to_string(),
        )],
    );
    let args = root.path().join("npm-args.txt");
    let npm = root.path().join("managers/npm");
    fake_manager(&npm, &args, latest_stdout, exit, stderr);
    NpmFixture {
        home: root.path().join("home"),
        cache: root.path().join("cache"),
        _root: root,
        exe,
        npm,
        args,
    }
}

fn npm_connector(fx: &NpmFixture) -> ExternalConnector<Codex> {
    connector(
        Codex,
        "codex",
        &fx.exe,
        &fx.home,
        BTreeMap::from([("npm".to_owned(), fx.npm.clone())]),
        Some(&fx.cache),
    )
}

#[tokio::test]
async fn an_outdated_npm_install_reports_both_versions_the_owner_and_the_exact_action() {
    let fx = npm_fixture("1.0.0", "1.2.0", 0, "");
    let connector = npm_connector(&fx);
    let check = connector.check_version(VersionCheckMode::Refresh).await;
    let ConnectorVersionCheck::Checked { status } = &check else {
        panic!("expected a checked status, got {check:?}");
    };
    assert_eq!(status.installed.as_deref(), Some("1.0.0"));
    assert_eq!(status.latest.as_deref(), Some("1.2.0"));
    assert_eq!(status.comparison, VersionComparison::Outdated);
    assert_eq!(status.freshness, VersionFreshness::Current);
    assert_eq!(
        status.source,
        InstallSource::Npm {
            package: "@openai/codex".into()
        }
    );
    assert_eq!(
        status.latest_source.as_deref(),
        Some("npm view @openai/codex version")
    );
    let action = status
        .update
        .as_ref()
        .expect("an outdated install offers its command");
    assert_eq!(action.program, fx.npm.display().to_string());
    assert_eq!(
        action.args,
        vec![
            "install".to_owned(),
            "-g".into(),
            "--prefix".into(),
            std::fs::canonicalize(fx._root.path())
                .unwrap()
                .display()
                .to_string(),
            "@openai/codex@latest".into()
        ]
    );
    assert!(check.update_available());
    assert_eq!(
        recorded_args(&fx.args),
        vec!["view", "@openai/codex", "version"],
        "the query is the documented vector, nothing else"
    );
    let text = check.describe();
    assert!(text.contains("outdated"), "{text}");
    assert!(text.contains("npm package @openai/codex"), "{text}");
}

#[tokio::test]
async fn a_current_install_offers_no_update() {
    let fx = npm_fixture("1.2.0", "1.2.0", 0, "");
    let check = npm_connector(&fx)
        .check_version(VersionCheckMode::Refresh)
        .await;
    let status = check.status().unwrap();
    assert_eq!(status.comparison, VersionComparison::Current);
    assert!(status.update.is_none(), "{status:?}");
    assert!(!check.update_available());
    assert!(check.describe().contains("is current"));
}

#[tokio::test]
async fn a_fresh_cache_answers_without_querying_and_a_refresh_queries_again() {
    let fx = npm_fixture("1.0.0", "1.2.0", 0, "");
    let connector = npm_connector(&fx);
    assert!(matches!(
        connector.check_version(VersionCheckMode::Cached).await,
        ConnectorVersionCheck::Checked { .. }
    ));
    std::fs::remove_file(&fx.args).unwrap();
    let again = connector.check_version(VersionCheckMode::Cached).await;
    assert!(matches!(again, ConnectorVersionCheck::Checked { .. }));
    assert_eq!(again.status().unwrap().latest.as_deref(), Some("1.2.0"));
    assert!(!fx.args.exists(), "a fresh cache must not run the query");
    connector.check_version(VersionCheckMode::Refresh).await;
    assert!(fx.args.exists(), "a refresh runs the query");
}

#[tokio::test]
async fn a_failed_query_falls_back_to_the_stale_cache_and_stays_stale() {
    let fx = npm_fixture("1.0.0", "1.5.0", 0, "");
    let connector = npm_connector(&fx);
    connector.check_version(VersionCheckMode::Refresh).await;
    fake_manager(
        &fx.npm,
        &fx.args,
        "",
        1,
        "npm ERR! code ENOTFOUND registry.npmjs.org",
    );
    let refreshed = connector.check_version(VersionCheckMode::Refresh).await;
    let ConnectorVersionCheck::CachedStale { status, reason } = &refreshed else {
        panic!("expected stale fallback, got {refreshed:?}");
    };
    assert_eq!(status.freshness, VersionFreshness::Stale);
    assert_eq!(status.latest.as_deref(), Some("1.5.0"));
    assert_eq!(status.comparison, VersionComparison::Outdated);
    assert!(reason.contains("exited unsuccessfully"), "{reason}");
    assert!(
        !refreshed.update_available(),
        "a stale answer is never presented as a current update offer"
    );
    let lookup = connector.check_version(VersionCheckMode::Cached).await;
    assert!(
        matches!(lookup, ConnectorVersionCheck::CachedStale { .. }),
        "an ordinary lookup after a failed refresh must stay stale, got {lookup:?}"
    );
}

#[tokio::test]
async fn a_failed_query_without_a_cache_is_typed_by_cause() {
    let offline = npm_fixture("1.0.0", "", 1, "npm ERR! code ENOTFOUND registry.npmjs.org");
    let check = npm_connector(&offline)
        .check_version(VersionCheckMode::Refresh)
        .await;
    let ConnectorVersionCheck::LatestUnavailable {
        status, failure, ..
    } = &check
    else {
        panic!("expected latest unavailable, got {check:?}");
    };
    assert_eq!(*failure, VersionCheckFailure::Network);
    assert_eq!(status.installed.as_deref(), Some("1.0.0"));
    assert_eq!(status.latest, None);
    assert_eq!(status.comparison, VersionComparison::Unknown);
    assert!(
        status.update.is_some(),
        "the owner is known, so the command is still shown"
    );
    assert!(!check.update_available());

    let denied = npm_fixture("1.0.0", "", 1, "npm ERR! 401 Unauthorized");
    let check = npm_connector(&denied)
        .check_version(VersionCheckMode::Refresh)
        .await;
    let ConnectorVersionCheck::LatestUnavailable { failure, .. } = &check else {
        panic!("{check:?}");
    };
    assert_eq!(*failure, VersionCheckFailure::Authentication);

    let malformed = npm_fixture("1.0.0", "not a version at all", 0, "");
    let check = npm_connector(&malformed)
        .check_version(VersionCheckMode::Refresh)
        .await;
    let ConnectorVersionCheck::LatestUnavailable { failure, .. } = &check else {
        panic!("{check:?}");
    };
    assert_eq!(*failure, VersionCheckFailure::MalformedResponse);

    let empty = npm_fixture("1.0.0", "", 0, "");
    let check = npm_connector(&empty)
        .check_version(VersionCheckMode::Refresh)
        .await;
    let ConnectorVersionCheck::LatestUnavailable { failure, .. } = &check else {
        panic!("{check:?}");
    };
    assert_eq!(*failure, VersionCheckFailure::MalformedResponse);
}

#[tokio::test]
async fn offline_mode_never_runs_a_query() {
    let fx = npm_fixture("1.0.0", "1.2.0", 0, "");
    let check = npm_connector(&fx)
        .check_version(VersionCheckMode::Offline)
        .await;
    let ConnectorVersionCheck::LatestUnavailable {
        status, failure, ..
    } = &check
    else {
        panic!("expected not-checked, got {check:?}");
    };
    assert_eq!(*failure, VersionCheckFailure::NotChecked);
    assert_eq!(status.installed.as_deref(), Some("1.0.0"));
    assert!(!fx.args.exists(), "offline mode ran the package manager");
}

#[tokio::test]
async fn a_missing_package_manager_is_a_typed_failure_not_a_panic() {
    let fx = npm_fixture("1.0.0", "1.2.0", 0, "");
    let connector = connector(
        Codex,
        "codex",
        &fx.exe,
        &fx.home,
        BTreeMap::from([("npm".to_owned(), fx.home.join("no-such-npm"))]),
        Some(&fx.cache),
    );
    let check = connector.check_version(VersionCheckMode::Refresh).await;
    let ConnectorVersionCheck::LatestUnavailable {
        failure, reason, ..
    } = &check
    else {
        panic!("{check:?}");
    };
    assert_eq!(*failure, VersionCheckFailure::CommandFailure);
    assert!(reason.contains("cannot run"), "{reason}");
}

#[tokio::test]
async fn homebrew_and_cargo_owners_use_their_documented_queries() {
    let root = tempfile::tempdir().unwrap();
    let cask = root.path().join("Caskroom/codex/0.1.0/bin/codex");
    fake_agent(&cask, &[]);
    let brew_args = root.path().join("brew-args.txt");
    let brew = root.path().join("managers/brew");
    fake_manager(
        &brew,
        &brew_args,
        r#"{"formulae":[],"casks":[{"token":"codex","version":"2.0.0"}]}"#,
        0,
        "",
    );
    let check = connector(
        Codex,
        "codex",
        &cask,
        &root.path().join("home"),
        BTreeMap::from([("brew".to_owned(), brew)]),
        None,
    )
    .check_version(VersionCheckMode::Refresh)
    .await;
    let status = check.status().unwrap();
    assert_eq!(status.latest.as_deref(), Some("2.0.0"));
    assert_eq!(status.comparison, VersionComparison::Outdated);
    assert_eq!(
        status.update.as_ref().unwrap().args,
        vec!["upgrade", "--cask", "codex"]
    );
    assert_eq!(
        recorded_args(&brew_args),
        vec!["info", "--json=v2", "codex"]
    );

    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join(".cargo/bin/opencode");
    fake_agent(&bin, &[]);
    std::fs::write(
        home.path().join(".cargo/.crates.toml"),
        "[v1]\n\"opencode-cli 0.9.0 (registry+x)\" = [\"opencode\"]\n",
    )
    .unwrap();
    let cargo_args = home.path().join("cargo-args.txt");
    let cargo = home.path().join("managers/cargo");
    fake_manager(
        &cargo,
        &cargo_args,
        "opencode-cli = \"1.0.0\"    # a cli\n",
        0,
        "",
    );
    let check = connector(
        OpenCode,
        "opencode",
        &bin,
        home.path(),
        BTreeMap::from([("cargo".to_owned(), cargo)]),
        None,
    )
    .check_version(VersionCheckMode::Refresh)
    .await;
    let status = check.status().unwrap();
    assert_eq!(
        status.source,
        InstallSource::Cargo {
            crate_name: "opencode-cli".into()
        }
    );
    assert_eq!(status.latest.as_deref(), Some("1.0.0"));
    assert_eq!(status.comparison, VersionComparison::Current);
    assert_eq!(
        recorded_args(&cargo_args),
        vec!["search", "opencode-cli", "--limit", "1"]
    );
}

#[tokio::test]
async fn an_unknown_owner_offers_no_update_and_says_what_to_do() {
    let root = tempfile::tempdir().unwrap();
    let exe = root.path().join("somewhere/bin/codex");
    fake_agent(&exe, &[]);
    let check = connector(
        Codex,
        "codex",
        &exe,
        &root.path().join("home"),
        BTreeMap::new(),
        None,
    )
    .check_version(VersionCheckMode::Refresh)
    .await;
    let ConnectorVersionCheck::LatestUnavailable {
        status, failure, ..
    } = &check
    else {
        panic!("{check:?}");
    };
    assert_eq!(*failure, VersionCheckFailure::UnsupportedSource);
    assert_eq!(status.source, InstallSource::Unknown);
    assert_eq!(status.installed.as_deref(), Some("1.0.0"));
    assert!(status.update.is_none());
    assert!(status
        .next_step
        .as_deref()
        .unwrap()
        .contains("could not tell which installer"));
}

#[tokio::test]
async fn a_missing_executable_is_not_installed() {
    let root = tempfile::tempdir().unwrap();
    let check = connector(
        Codex,
        "codex",
        &root.path().join("missing"),
        root.path(),
        BTreeMap::new(),
        None,
    )
    .check_version(VersionCheckMode::Refresh)
    .await;
    assert!(
        matches!(check, ConnectorVersionCheck::NotInstalled { .. }),
        "{check:?}"
    );
}

// -- updates -----------------------------------------------------------------

struct VendorFixture {
    home: tempfile::TempDir,
    exe: PathBuf,
    pid_file: PathBuf,
}

fn vendor_fixture(extra: &[(&str, String)]) -> VendorFixture {
    let home = tempfile::tempdir().unwrap();
    let version_file = home.path().join("version.txt");
    std::fs::write(&version_file, "1.0.0\n").unwrap();
    let pid_file = home.path().join("update.pid");
    let exe = home.path().join(".opencode/bin/opencode");
    let mut vars = vec![
        (
            "FAKE_AGENT_VERSION_FILE",
            version_file.display().to_string(),
        ),
        ("FAKE_AGENT_PID_FILE", pid_file.display().to_string()),
        ("FAKE_AGENT_UPDATE_TO", "2.0.0".to_owned()),
    ];
    vars.extend(extra.iter().map(|(k, v)| (*k, v.clone())));
    fake_agent(&exe, &vars);
    VendorFixture {
        exe,
        pid_file,
        home,
    }
}

fn vendor_connector(fx: &VendorFixture) -> ExternalConnector<OpenCode> {
    connector(
        OpenCode,
        "opencode",
        &fx.exe,
        fx.home.path(),
        BTreeMap::new(),
        None,
    )
}

#[tokio::test]
async fn an_approved_update_runs_the_vector_and_rechecks_the_version() {
    let fx = vendor_fixture(&[]);
    let connector = vendor_connector(&fx);
    let check = connector.check_version(VersionCheckMode::Refresh).await;
    let status = check.status().unwrap();
    assert_eq!(
        status.source,
        InstallSource::Vendor {
            installer: "OpenCode install script".into()
        }
    );
    assert_eq!(status.installed.as_deref(), Some("1.0.0"));
    let action = status
        .update
        .clone()
        .expect("a vendor install offers its self-update");
    assert_eq!(action.program, fx.exe.display().to_string());
    assert_eq!(action.args, vec!["upgrade"]);
    let outcome = connector.update(action).await;
    let ConnectorUpdateOutcome::Updated {
        before,
        after,
        recheck,
        ..
    } = &outcome
    else {
        panic!("expected an update, got {outcome:?}");
    };
    assert_eq!(before.as_deref(), Some("1.0.0"));
    assert_eq!(after.as_deref(), Some("2.0.0"));
    assert_eq!(
        recheck.status().unwrap().installed.as_deref(),
        Some("2.0.0"),
        "a successful update triggers a fresh check"
    );
    assert!(outcome.describe().contains("updated to 2.0.0"));
}

#[tokio::test]
async fn a_failed_update_reports_the_outcome_and_redacts_secrets() {
    let fx = vendor_fixture(&[("FAKE_AGENT_UPDATE_EXIT", "1".to_owned())]);
    let connector = vendor_connector(&fx).with_secrets(vec![Secret::new("sk-fake-secret")]);
    let action = connector
        .check_version(VersionCheckMode::Refresh)
        .await
        .status()
        .unwrap()
        .update
        .clone()
        .unwrap();
    let outcome = connector.update(action).await;
    let ConnectorUpdateOutcome::Failed { reason, output, .. } = &outcome else {
        panic!("expected a failure, got {outcome:?}");
    };
    assert!(reason.contains("status 1"), "{reason}");
    let joined = output.join("\n");
    assert!(joined.is_empty(), "{joined}");
    assert!(!joined.contains("sk-fake-secret"), "{joined}");
    // The connector is still usable: its version still probes.
    let info = connector.info().await.unwrap();
    assert_eq!(info.version.as_deref(), Some("1.0.0"));
}

#[tokio::test]
async fn a_silent_update_times_out_kills_the_process_and_leaves_the_connector_usable() {
    let fx = vendor_fixture(&[("FAKE_AGENT_UPDATE_SLEEP", "30".to_owned())]);
    let connector = vendor_connector(&fx);
    let action = connector
        .check_version(VersionCheckMode::Refresh)
        .await
        .status()
        .unwrap()
        .update
        .clone()
        .unwrap();
    let started = std::time::Instant::now();
    let outcome = connector.update(action).await;
    assert!(
        matches!(outcome, ConnectorUpdateOutcome::TimedOut { .. }),
        "{outcome:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(20));
    let pid: u32 = std::fs::read_to_string(&fx.pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!is_alive(pid).await, "the timed-out update kept running");
    assert_eq!(
        std::fs::read_to_string(fx.home.path().join("version.txt"))
            .unwrap()
            .trim(),
        "1.0.0"
    );
    assert!(connector.info().await.is_ok());
}

#[tokio::test]
async fn cancelling_an_update_stops_its_process() {
    let fx = vendor_fixture(&[("FAKE_AGENT_UPDATE_SLEEP", "30".to_owned())]);
    let connector = Arc::new(vendor_connector(&fx));
    let action = connector
        .check_version(VersionCheckMode::Refresh)
        .await
        .status()
        .unwrap()
        .update
        .clone()
        .unwrap();
    let running = Arc::clone(&connector);
    let task = tokio::spawn(async move { running.update(action).await });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !fx.pid_file.exists() && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let pid: u32 = std::fs::read_to_string(&fx.pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(is_alive(pid).await);
    task.abort();
    let _ = task.await;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while is_alive(pid).await && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        !is_alive(pid).await,
        "cancelling the update left its process running"
    );
}

#[tokio::test]
async fn an_update_never_runs_without_an_action_for_a_missing_executable() {
    let root = tempfile::tempdir().unwrap();
    let connector = connector(
        Codex,
        "codex",
        &root.path().join("missing"),
        root.path(),
        BTreeMap::new(),
        None,
    );
    let outcome = connector
        .update(gritt_core::connector::UpdateAction {
            program: "npm".into(),
            args: vec!["install".into(), "-g".into(), "@openai/codex@latest".into()],
            source: InstallSource::Npm {
                package: "@openai/codex".into(),
            },
        })
        .await;
    assert!(
        matches!(outcome, ConnectorUpdateOutcome::NoAction { .. }),
        "{outcome:?}"
    );
}
