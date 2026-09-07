//! Who installed a connector executable, and the fixed commands that
//! owner documents for finding and installing its newest version.
//!
//! Every detector needs evidence beyond a path prefix: a Homebrew
//! `Cellar` or `Caskroom` directory, an npm package directory with its
//! `package.json`, a pipx venv with `pipx_metadata.json`, a Cargo
//! `.crates.toml` entry naming the binary, or a vendor installer's own
//! directory under the home directory. Two owners with evidence make the
//! result ambiguous. No owner makes it unknown. Neither gets a command.
//! Nothing here reads a shell profile, a credential, or package-manager
//! configuration.

use std::path::{Path, PathBuf};

use gritt_core::connector::{InstallSource, UpdateAction, VersionCheckFailure};

/// Where the detectors look. Injected so tests use a scratch home.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallEnv {
    pub home: Option<PathBuf>,
    pub cargo_home: Option<PathBuf>,
}

impl InstallEnv {
    pub fn from_process() -> Self {
        let home = dirs::home_dir();
        let cargo_home = std::env::var_os("CARGO_HOME")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|home| home.join(".cargo")));
        Self { home, cargo_home }
    }
}

/// A vendor's own installer, as a connector protocol documents it:
/// directories under the home directory that only that installer
/// creates, and the self-update subcommand it documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VendorInstall {
    pub installer: &'static str,
    /// Home-relative directory fragments, with a trailing separator.
    pub markers: &'static [&'static str],
    /// Arguments to the connector executable itself.
    pub update_args: &'static [&'static str],
}

/// Detects the owner of `executable`. Symlinks are resolved first: the
/// visible `bin/` entry of a Homebrew or npm install is a link into the
/// directory that carries the evidence.
pub fn detect_install_source(
    executable: &Path,
    env: &InstallEnv,
    vendor: Option<&VendorInstall>,
) -> InstallSource {
    let resolved = std::fs::canonicalize(executable).unwrap_or_else(|_| executable.to_path_buf());
    let mut found: Vec<InstallSource> = Vec::new();
    if let Some(source) = homebrew(&resolved) {
        found.push(source);
    }
    if let Some(source) = npm(&resolved) {
        found.push(source);
    }
    if let Some(source) = pipx(&resolved) {
        found.push(source);
    }
    if let Some(source) = cargo(&resolved, env) {
        found.push(source);
    }
    if let Some(source) = vendor_owned(executable, &resolved, env, vendor) {
        found.push(source);
    }
    match found.len() {
        0 => InstallSource::Unknown,
        1 => found.remove(0),
        _ => InstallSource::Ambiguous {
            candidates: found.iter().map(InstallSource::label).collect(),
        },
    }
}

fn components(path: &Path) -> Vec<String> {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect()
}

fn homebrew(resolved: &Path) -> Option<InstallSource> {
    let parts = components(resolved);
    let index = parts
        .iter()
        .position(|part| part == "Cellar" || part == "Caskroom")?;
    let name = parts.get(index + 1)?;
    if name.is_empty() {
        return None;
    }
    Some(InstallSource::Homebrew {
        name: name.clone(),
        cask: parts[index] == "Caskroom",
    })
}

fn npm(resolved: &Path) -> Option<InstallSource> {
    let parts = components(resolved);
    let index = parts.iter().position(|part| part == "node_modules")?;
    let first = parts.get(index + 1)?;
    let (package, depth) = if let Some(scope) = first.strip_prefix('@') {
        let name = parts.get(index + 2)?;
        (format!("@{scope}/{name}"), 3)
    } else {
        (first.clone(), 2)
    };
    // The package directory is the ancestor that many `bin/` levels sit
    // under; its manifest is the evidence that this is an npm package.
    let package_dir: PathBuf = resolved.components().take(index + depth).collect();
    if !package_dir.join("package.json").is_file() {
        return None;
    }
    Some(InstallSource::Npm { package })
}

fn pipx(resolved: &Path) -> Option<InstallSource> {
    let parts = components(resolved);
    let index = parts
        .iter()
        .enumerate()
        .position(|(i, part)| part == "venvs" && i > 0 && parts[i - 1] == "pipx")?;
    let package = parts.get(index + 1)?.clone();
    let venv: PathBuf = resolved.components().take(index + 2).collect();
    if !venv.join("pipx_metadata.json").is_file() {
        return None;
    }
    Some(InstallSource::Pipx { package })
}

fn cargo(resolved: &Path, env: &InstallEnv) -> Option<InstallSource> {
    let cargo_home = env.cargo_home.as_ref()?;
    let bin = cargo_home.join("bin");
    let bin = std::fs::canonicalize(&bin).unwrap_or(bin);
    if resolved.parent()? != bin {
        return None;
    }
    let file = resolved.file_name()?.to_string_lossy().into_owned();
    let file = file.strip_suffix(".exe").unwrap_or(&file).to_owned();
    let manifest = std::fs::read_to_string(cargo_home.join(".crates.toml")).ok()?;
    crate_owning(&manifest, &file).map(|crate_name| InstallSource::Cargo { crate_name })
}

/// The crate in a `.crates.toml` whose binary list names `binary`.
/// Each entry looks like `"name 1.2.3 (registry+...)" = ["bin-a", "bin-b"]`.
pub fn crate_owning(manifest: &str, binary: &str) -> Option<String> {
    for line in manifest.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix('"') else {
            continue;
        };
        let (key, value) = rest.split_once('"')?;
        let bins = value.split_once('=').map(|(_, v)| v)?;
        let names = bins
            .trim()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .split(',')
            .map(|name| name.trim().trim_matches('"'));
        if names.into_iter().any(|name| name == binary) {
            return key.split_whitespace().next().map(str::to_owned);
        }
    }
    None
}

fn vendor_owned(
    original: &Path,
    resolved: &Path,
    env: &InstallEnv,
    vendor: Option<&VendorInstall>,
) -> Option<InstallSource> {
    let vendor = vendor?;
    let home = env.home.as_ref()?;
    let home = std::fs::canonicalize(home).unwrap_or_else(|_| home.clone());
    for marker in vendor.markers {
        let dir = home.join(marker.trim_matches(['/', '\\']));
        if original.starts_with(&dir) || resolved.starts_with(&dir) {
            return Some(InstallSource::Vendor {
                installer: vendor.installer.to_owned(),
            });
        }
    }
    None
}

/// The documented update command for a known owner. `executable` is the
/// connector itself, which vendor installers update in place.
pub fn update_action(
    source: &InstallSource,
    executable: &Path,
    vendor: Option<&VendorInstall>,
) -> Option<UpdateAction> {
    let (program, args): (String, Vec<String>) = match source {
        InstallSource::Homebrew { name, cask: true } => (
            "brew".into(),
            vec!["upgrade".into(), "--cask".into(), name.clone()],
        ),
        InstallSource::Homebrew { name, cask: false } => {
            ("brew".into(), vec!["upgrade".into(), name.clone()])
        }
        // An npm package can be local, global, or owned by another Node
        // manager. Only the lifecycle check can verify npm's global root.
        InstallSource::Npm { .. } => return None,
        InstallSource::Pipx { package } => ("pipx".into(), vec!["upgrade".into(), package.clone()]),
        InstallSource::Cargo { crate_name } => {
            ("cargo".into(), vec!["install".into(), crate_name.clone()])
        }
        InstallSource::Vendor { .. } => {
            let vendor = vendor?;
            (
                executable.display().to_string(),
                vendor
                    .update_args
                    .iter()
                    .map(|arg| (*arg).to_owned())
                    .collect(),
            )
        }
        InstallSource::Unknown | InstallSource::Ambiguous { .. } => return None,
    };
    Some(UpdateAction {
        program,
        args,
        source: source.clone(),
    })
}

/// Offers an npm update only when this manager's global root contains the
/// selected package. Pin the manager and prefix so another npm on PATH or
/// a different current directory cannot redirect the approved update.
pub fn npm_update_action(
    source: &InstallSource,
    executable: &Path,
    manager: &Path,
    global_root: &Path,
) -> Option<UpdateAction> {
    let InstallSource::Npm { package } = source else {
        return None;
    };
    let root = std::fs::canonicalize(global_root).ok()?;
    let package_dir = std::fs::canonicalize(root.join(package)).ok()?;
    // `npm link` can expose a project-local package from the global root.
    // Replacing that link would not update an explicitly configured local executable.
    if package_dir != root.join(package) {
        return None;
    }
    let executable = std::fs::canonicalize(executable).ok()?;
    if !executable.starts_with(&package_dir) || root.file_name()? != "node_modules" {
        return None;
    }
    let parent = root.parent()?;
    #[cfg(unix)]
    let prefix = {
        if parent.file_name()? != "lib" {
            return None;
        }
        parent.parent()?
    };
    #[cfg(windows)]
    let prefix = parent;
    Some(UpdateAction {
        program: manager.display().to_string(),
        args: vec![
            "install".into(),
            "-g".into(),
            "--prefix".into(),
            prefix.display().to_string(),
            format!("{package}@latest"),
        ],
        source: source.clone(),
    })
}

/// What the user can do when no update is offered.
pub fn next_step(source: &InstallSource, executable: &Path) -> Option<String> {
    let path = executable.display();
    match source {
        InstallSource::Unknown => Some(format!(
            "Gritt could not tell which installer owns {path}; update it with the tool \
             you installed it with, then run the check again"
        )),
        InstallSource::Ambiguous { candidates } => Some(format!(
            "{path} could belong to {}; update it with the one you used, then run the \
             check again",
            candidates.join(" or ")
        )),
        InstallSource::Npm { .. } => Some(
            "the selected npm has not verified ownership of this global installation; refresh the check or update it with its original installer".into()
        ),
        InstallSource::Pipx { .. } | InstallSource::Vendor { .. } => Some(
            "this installer does not publish a newest version Gritt can read; the update \
             command reports whether anything changed"
                .to_owned(),
        ),
        _ => None,
    }
}

/// The documented query for the newest version an owner publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatestQuery {
    pub program: String,
    pub args: Vec<String>,
    /// How the query is described to the user.
    pub source: String,
}

pub fn latest_query(source: &InstallSource) -> Option<LatestQuery> {
    match source {
        InstallSource::Homebrew { name, .. } => Some(LatestQuery {
            program: "brew".into(),
            args: vec!["info".into(), "--json=v2".into(), name.clone()],
            source: format!("brew info --json=v2 {name}"),
        }),
        InstallSource::Npm { package } => Some(LatestQuery {
            program: "npm".into(),
            args: vec!["view".into(), package.clone(), "version".into()],
            source: format!("npm view {package} version"),
        }),
        InstallSource::Cargo { crate_name } => Some(LatestQuery {
            program: "cargo".into(),
            args: vec![
                "search".into(),
                crate_name.clone(),
                "--limit".into(),
                "1".into(),
            ],
            source: format!("cargo search {crate_name} --limit 1"),
        }),
        InstallSource::Pipx { .. }
        | InstallSource::Vendor { .. }
        | InstallSource::Unknown
        | InstallSource::Ambiguous { .. } => None,
    }
}

/// Reads the newest version out of a query's stdout.
pub fn parse_latest(source: &InstallSource, stdout: &str) -> Result<String, VersionCheckFailure> {
    let malformed = VersionCheckFailure::MalformedResponse;
    match source {
        InstallSource::Homebrew { name, cask } => {
            let value: serde_json::Value =
                serde_json::from_str(stdout.trim()).map_err(|_| malformed)?;
            let version = if *cask {
                value
                    .get("casks")
                    .and_then(|casks| casks.as_array())
                    .and_then(|casks| {
                        casks
                            .iter()
                            .find(|cask| cask.get("token").and_then(|t| t.as_str()) == Some(name))
                            .or_else(|| casks.first())
                    })
                    .and_then(|cask| cask.get("version"))
                    .and_then(|v| v.as_str())
                    // A cask version may carry a build after a comma.
                    .map(|v| v.split(',').next().unwrap_or(v).to_owned())
            } else {
                value
                    .get("formulae")
                    .and_then(|formulae| formulae.as_array())
                    .and_then(|formulae| {
                        formulae
                            .iter()
                            .find(|f| f.get("name").and_then(|n| n.as_str()) == Some(name))
                            .or_else(|| formulae.first())
                    })
                    .and_then(|formula| formula.get("versions"))
                    .and_then(|versions| versions.get("stable"))
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
            };
            version.filter(|v| !v.is_empty()).ok_or(malformed)
        }
        InstallSource::Npm { .. } => stdout
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .and_then(crate::health::version_token)
            .ok_or(malformed),
        InstallSource::Cargo { crate_name } => stdout
            .lines()
            .map(str::trim)
            .find_map(|line| {
                let rest = line.strip_prefix(crate_name.as_str())?;
                let rest = rest.trim_start().strip_prefix('=')?;
                let rest = rest.trim_start().strip_prefix('"')?;
                let (version, _) = rest.split_once('"')?;
                (!version.is_empty()).then(|| version.to_owned())
            })
            .ok_or(malformed),
        _ => Err(VersionCheckFailure::UnsupportedSource),
    }
}

/// Classifies a query that exited unsuccessfully from what it printed.
/// Only the class is kept; the text itself never leaves the process.
pub fn classify_failure(stderr: &str, stdout: &str) -> VersionCheckFailure {
    let text = format!("{stderr}\n{stdout}").to_ascii_lowercase();
    const NETWORK: [&str; 10] = [
        "enotfound",
        "eai_again",
        "econnrefused",
        "econnreset",
        "network",
        "could not resolve",
        "couldn't connect",
        "failed to fetch",
        "offline",
        "timed out",
    ];
    const AUTH: [&str; 6] = [
        "401",
        "403",
        "unauthorized",
        "forbidden",
        "authentication",
        "please log in",
    ];
    if NETWORK.iter().any(|marker| text.contains(marker)) {
        VersionCheckFailure::Network
    } else if AUTH.iter().any(|marker| text.contains(marker)) {
        VersionCheckFailure::Authentication
    } else {
        VersionCheckFailure::CommandFailure
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crates_toml_names_the_owning_crate() {
        let manifest = "[v1]\n\
            \"cargo-dist 0.32.0 (registry+https://github.com/rust-lang/crates.io-index)\" = [\"dist\"]\n\
            \"ripgrep 15.2.0 (registry+https://github.com/rust-lang/crates.io-index)\" = [\"rg\"]\n";
        assert_eq!(crate_owning(manifest, "rg").as_deref(), Some("ripgrep"));
        assert_eq!(
            crate_owning(manifest, "dist").as_deref(),
            Some("cargo-dist")
        );
        assert_eq!(crate_owning(manifest, "cargo-dist"), None);
    }

    #[test]
    fn latest_versions_parse_from_documented_output_only() {
        let cask = InstallSource::Homebrew {
            name: "codex".into(),
            cask: true,
        };
        assert_eq!(
            parse_latest(
                &cask,
                r#"{"formulae":[],"casks":[{"token":"codex","version":"0.153.4,77"}]}"#
            )
            .as_deref(),
            Ok("0.153.4")
        );
        let formula = InstallSource::Homebrew {
            name: "opencode".into(),
            cask: false,
        };
        assert_eq!(
            parse_latest(
                &formula,
                r#"{"formulae":[{"name":"opencode","versions":{"stable":"1.18.29"}}],"casks":[]}"#
            )
            .as_deref(),
            Ok("1.18.29")
        );
        assert_eq!(
            parse_latest(&formula, "not json"),
            Err(VersionCheckFailure::MalformedResponse)
        );
        assert_eq!(
            parse_latest(&formula, r#"{"formulae":[],"casks":[]}"#),
            Err(VersionCheckFailure::MalformedResponse)
        );
        let npm = InstallSource::Npm {
            package: "@openai/codex".into(),
        };
        assert_eq!(parse_latest(&npm, "\n0.153.4\n").as_deref(), Ok("0.153.4"));
        assert_eq!(
            parse_latest(&npm, ""),
            Err(VersionCheckFailure::MalformedResponse)
        );
        assert_eq!(
            parse_latest(&npm, "   \n"),
            Err(VersionCheckFailure::MalformedResponse)
        );
        let cargo = InstallSource::Cargo {
            crate_name: "ripgrep".into(),
        };
        assert_eq!(
            parse_latest(
                &cargo,
                "ripgrep = \"15.2.0\"    # ripgrep is a line-oriented search tool\n... and 638 crates more\n"
            )
            .as_deref(),
            Ok("15.2.0")
        );
        assert_eq!(
            parse_latest(&cargo, "nothing = \"1\""),
            Err(VersionCheckFailure::MalformedResponse)
        );
        assert_eq!(
            parse_latest(
                &InstallSource::Pipx {
                    package: "x".into()
                },
                "1.0"
            ),
            Err(VersionCheckFailure::UnsupportedSource)
        );
    }

    #[test]
    fn failures_classify_without_keeping_the_text() {
        assert_eq!(
            classify_failure("npm ERR! code ENOTFOUND", ""),
            VersionCheckFailure::Network
        );
        assert_eq!(
            classify_failure("npm ERR! 401 Unauthorized", ""),
            VersionCheckFailure::Authentication
        );
        assert_eq!(
            classify_failure("Error: No available formula", ""),
            VersionCheckFailure::CommandFailure
        );
    }

    #[test]
    fn update_actions_are_fixed_vectors_per_owner_and_none_for_unknown() {
        let exe = Path::new("/home/u/.opencode/bin/opencode");
        let vendor = VendorInstall {
            installer: "OpenCode install script",
            markers: &[".opencode/bin/"],
            update_args: &["upgrade"],
        };
        type Expected = Option<(&'static str, Vec<&'static str>)>;
        let cases: Vec<(InstallSource, Expected)> = vec![
            (
                InstallSource::Homebrew {
                    name: "codex".into(),
                    cask: true,
                },
                Some(("brew", vec!["upgrade", "--cask", "codex"])),
            ),
            (
                InstallSource::Homebrew {
                    name: "opencode".into(),
                    cask: false,
                },
                Some(("brew", vec!["upgrade", "opencode"])),
            ),
            (
                InstallSource::Npm {
                    package: "@anthropic-ai/claude-code".into(),
                },
                None,
            ),
            (
                InstallSource::Pipx {
                    package: "tool".into(),
                },
                Some(("pipx", vec!["upgrade", "tool"])),
            ),
            (
                InstallSource::Cargo {
                    crate_name: "tool".into(),
                },
                Some(("cargo", vec!["install", "tool"])),
            ),
            (
                InstallSource::Vendor {
                    installer: vendor.installer.into(),
                },
                Some(("/home/u/.opencode/bin/opencode", vec!["upgrade"])),
            ),
            (InstallSource::Unknown, None),
            (
                InstallSource::Ambiguous {
                    candidates: vec!["a".into(), "b".into()],
                },
                None,
            ),
        ];
        for (source, expected) in cases {
            let action = update_action(&source, exe, Some(&vendor));
            match expected {
                Some((program, args)) => {
                    let action = action.expect("an action for a known owner");
                    assert_eq!(action.program, program);
                    assert_eq!(action.args, args);
                    assert_eq!(action.source, source);
                }
                None => {
                    assert!(action.is_none(), "{source:?} must not guess a command");
                    assert!(next_step(&source, exe).is_some());
                }
            }
        }
        assert!(latest_query(&InstallSource::Unknown).is_none());
        assert_eq!(
            latest_query(&InstallSource::Npm {
                package: "x".into()
            })
            .unwrap()
            .args,
            vec!["view", "x", "version"]
        );
    }
}
