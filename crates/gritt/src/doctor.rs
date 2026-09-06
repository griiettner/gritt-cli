//! `gritt doctor`: a local diagnostic report. Everything it prints is
//! content-free and value-free: key availability, never a key; row counts,
//! never a prompt.

use std::io::IsTerminal;
use std::path::Path;
use std::sync::Arc;

use chrono::Utc;
use gritt_core::connector::AuthState;
use gritt_core::Result;
use gritt_harness::control::ControlPlane;
use gritt_harness::store::{DatabaseLocation, Store, MEMORY_OBJECTS, MIGRATIONS};
use gritt_provider::models::ModelCache;

use crate::config;
use crate::keys;

pub struct Report {
    pub lines: Vec<String>,
}

impl Report {
    fn line(&mut self, text: impl Into<String>) {
        self.lines.push(text.into());
    }

    fn section(&mut self, title: &str) {
        if !self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.lines.push(format!("## {title}"));
    }
}

/// Builds the report. `plane` is optional so a broken connector setup still
/// produces every other section.
pub async fn report(
    workspace: &Path,
    store: &Arc<Store>,
    plane: Option<&ControlPlane>,
    config_error: Option<&str>,
) -> Result<Report> {
    let mut report = Report { lines: Vec::new() };

    report.section("platform");
    report.line(format!("gritt {}", env!("CARGO_PKG_VERSION")));
    report.line(format!(
        "os: {} {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    report.line(format!("workspace: {}", workspace.display()));

    report.section("configuration (highest precedence first)");
    report.line("1. command-line flags");
    let project = workspace.join(config::PROJECT_CONFIG);
    report.line(format!(
        "2. project config: {} ({})",
        project.display(),
        present(project.is_file())
    ));
    match config::user_config_path() {
        Some(user) => report.line(format!(
            "3. user config: {} ({})",
            user.display(),
            present(user.is_file())
        )),
        None => report.line("3. user config: no config directory on this platform"),
    }
    report.line("4. environment: GRITT_DEFAULT_PROFILE, GRITT_DEFAULT_MODEL, GRITT_FALLBACK_PROFILES, AGENT_EMBEDDING_PROVIDER, AGENT_RERANK_PROVIDER, AGENT_MEMORY_BASE_URL, AGENT_MEMORY_API_KEY");
    report.line("5. built-in defaults");
    if let Some(error) = config_error {
        report.line(format!("config error: {error}"));
    }

    let loaded = config::load(workspace, std::env::vars()).ok();
    report.section("profiles");
    match &loaded {
        Some(config) if !config.profiles.is_empty() => {
            let resolver = keys::KeyResolver {
                keychain: keys::SystemKeychain,
                env: keys::ProcessEnv,
            };
            let cache = ModelCache::default_dir().map(ModelCache::new);
            for (name, profile) in &config.profiles {
                let key = match resolver.resolve(name, &profile.key) {
                    Ok(_) => "key available".to_owned(),
                    Err(error) => error.message,
                };
                let models = match cache.as_ref().map(|cache| cache.read(name)) {
                    Some(Ok(Some(list))) => match list.fetched_at {
                        Some(fetched) => {
                            let age = Utc::now().signed_duration_since(fetched);
                            let state = if age.num_hours() < 24 {
                                "fresh"
                            } else {
                                "stale"
                            };
                            format!("{} models, {state}, fetched {}", list.models.len(), fetched)
                        }
                        None => "no successful fetch yet".to_owned(),
                    },
                    Some(Ok(None)) => "no cache".to_owned(),
                    Some(Err(error)) => format!("cache unreadable: {}", error.message),
                    None => "no cache directory on this platform".to_owned(),
                };
                report.line(format!(
                    "{name}: {:?} {} | {key} | model list: {models}",
                    profile.protocol, profile.base_url
                ));
            }
            report.line(format!(
                "default: {}/{}",
                config.default_profile.as_deref().unwrap_or("-"),
                config.default_model.as_deref().unwrap_or("-")
            ));
            report.line(format!(
                "fallback profiles: {}",
                if config.fallback_profiles.is_empty() {
                    "none".to_owned()
                } else {
                    config.fallback_profiles.join(", ")
                }
            ));
            report.line(format!(
                "embeddings: {} | reranking: {}",
                enabled(config.embeddings.as_ref().is_some_and(|e| e.is_enabled())),
                enabled(config.rerank.as_ref().is_some_and(|r| r.is_enabled()))
            ));
            report.line(format!(
                "content logging: {} ({} day retention)",
                enabled(config.logging.content_logging),
                config.logging.content_retention_days
            ));
        }
        Some(_) => report.line("no profiles configured; add [profiles.<name>] to the config"),
        None => report.line("configuration did not load; see above"),
    }
    if let Some(dir) = ModelCache::default_dir() {
        report.line(format!("model cache directory: {}", dir.display()));
    }

    report.section("database");
    let location = store.location();
    let kind = match location {
        DatabaseLocation::Explicit(_) => "explicit --database",
        DatabaseLocation::Workspace(_) => "workspace product database",
        DatabaseLocation::UserData(_) => "user data directory",
    };
    report.line(format!("path: {} ({kind})", location.path().display()));
    let applied = store.applied_migrations().await?;
    report.line(format!(
        "product migrations: {}/{} applied",
        applied.len(),
        MIGRATIONS.len()
    ));
    for (name, _) in MIGRATIONS {
        let state = if applied.iter().any(|a| a == name) {
            "applied"
        } else {
            "pending"
        };
        report.line(format!("  {name}: {state}"));
    }
    let objects = store.object_names().await?;
    let memory_present = MEMORY_OBJECTS
        .iter()
        .filter(|object| objects.iter().any(|o| o == *object))
        .count();
    report.line(format!(
        "gritt-agent memory namespace: {}",
        if memory_present == MEMORY_OBJECTS.len() {
            "present".to_owned()
        } else if memory_present == 0 {
            "absent (expected in a separate product database)".to_owned()
        } else {
            format!(
                "partial ({memory_present}/{} objects)",
                MEMORY_OBJECTS.len()
            )
        }
    ));
    for (label, table) in [
        ("sessions", "gritt_sessions"),
        ("session events", "gritt_session_events"),
        ("telemetry events", "gritt_telemetry_events"),
        ("analytics records", "gritt_analytics_records"),
        ("content log rows", "gritt_content_log"),
    ] {
        report.line(format!("{label}: {}", count(store, table).await?));
    }

    report.section("connectors");
    match plane {
        Some(plane) => {
            for (id, info) in plane.infos().await {
                match info {
                    Ok(info) => report.line(format!(
                        "{}: {} | version {} | transport {:?} | approvals {}",
                        id.as_str(),
                        match info.auth {
                            AuthState::NotInstalled => "not installed",
                            AuthState::Authenticated => "authenticated",
                            AuthState::Unauthenticated => "installed, not authenticated",
                            AuthState::Unknown => "installed, auth unknown",
                        },
                        info.version.as_deref().unwrap_or("-"),
                        info.transport,
                        if info.capabilities.approvals {
                            "relayed by gritt"
                        } else if info.auth == AuthState::NotInstalled {
                            "-"
                        } else {
                            "the agent's own"
                        }
                    )),
                    Err(error) => report.line(format!("{}: error {}", id.as_str(), error.message)),
                }
            }
        }
        None => report.line("connector setup failed; see the config error above"),
    }

    report.section("terminal");
    report.line(format!(
        "stdin: {} | stdout: {}",
        tty(std::io::stdin().is_terminal()),
        tty(std::io::stdout().is_terminal())
    ));
    report.line(format!(
        "TERM: {} | NO_COLOR: {} | COLORTERM: {}",
        std::env::var("TERM").unwrap_or_else(|_| "-".into()),
        if std::env::var_os("NO_COLOR").is_some() {
            "set (colors off)"
        } else {
            "unset"
        },
        std::env::var("COLORTERM").unwrap_or_else(|_| "-".into())
    ));
    report.line("modes: print (always), repl (stdin), tui (needs a terminal on stdin and stdout)");
    Ok(report)
}

async fn count(store: &Store, table: &str) -> Result<i64> {
    let mut rows = store
        .connection()
        .query(&format!("SELECT COUNT(*) FROM {table}"), ())
        .await
        .map_err(gritt_harness::store::storage_error)?;
    match rows
        .next()
        .await
        .map_err(gritt_harness::store::storage_error)?
    {
        Some(row) => row.get(0).map_err(gritt_harness::store::storage_error),
        None => Ok(0),
    }
}

fn present(found: bool) -> &'static str {
    if found {
        "found"
    } else {
        "not found"
    }
}

fn enabled(on: bool) -> &'static str {
    if on {
        "enabled"
    } else {
        "disabled"
    }
}

fn tty(is: bool) -> &'static str {
    if is {
        "terminal"
    } else {
        "not a terminal"
    }
}
