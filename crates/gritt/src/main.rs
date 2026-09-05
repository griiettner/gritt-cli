//! The `gritt` binary: argument parsing, configuration, key loading, and
//! mode selection. The harness crate owns the modes themselves.

use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use clap::{Args, Parser, Subcommand};
use gritt_connector::{default_connectors_with_secrets, environment_secrets, parse_connector_id};
use gritt_core::connector::{AuthState, ConnectorId};
use gritt_core::event::{ApprovalDecision, EventKind};
use gritt_core::session::{Phase, SessionStore};
use gritt_core::{Error, Result};
use gritt_harness::agent::{AgentBuilder, ApprovalMode, SessionSelector, TurnStatus, Ui};
use gritt_harness::control::ControlPlane;
use gritt_harness::modes::print::{PrintUi, PrintUiOptions};
use gritt_harness::modes::repl::{line_prompter, run_repl, CancelSlot, LineInput};
use gritt_harness::store::{resolve_location, Store};
use gritt_harness::telemetry::Telemetry;
use gritt_harness::tools::Workspace;
use gritt_harness::tui::run_tui;
use gritt_provider::models::{ModelCache, ModelCatalog};
use gritt_provider::ReqwestTransport;

mod config;
mod doctor;
mod keys;
mod setup;

/// Run native and installed AI coding agents from one local terminal.
#[derive(Parser)]
#[command(name = "gritt", version, about)]
struct Cli {
    /// Workspace directory. Defaults to the current directory.
    #[arg(long, global = true, value_name = "PATH")]
    workspace: Option<PathBuf>,

    /// Database file override. Defaults to `.agents/brain/data/gritt.db`
    /// in agent workspaces or the user data directory.
    #[arg(long, global = true, value_name = "PATH")]
    database: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Show the resolved configuration without any secret values.
    Config,
    /// Store a provider key in the OS keychain. The key is read from stdin
    /// and never written to a file.
    KeySet {
        /// Provider profile name.
        profile: String,
    },
    /// Print mode: one prompt in, streamed text out.
    Run {
        /// The prompt.
        prompt: String,
        #[command(flatten)]
        session: SessionArgs,
        /// Print status changes, reasoning, and usage on stderr.
        #[arg(long)]
        verbose: bool,
    },
    /// REPL mode: an interactive loop with history and continuation.
    Repl {
        #[command(flatten)]
        session: SessionArgs,
        #[arg(long)]
        verbose: bool,
    },
    /// Full-screen mode. Also runs when no subcommand is given.
    Tui {
        #[command(flatten)]
        session: SessionArgs,
    },
    /// Manage sessions.
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },
    /// List connectors with installed state, version, auth, and
    /// capabilities.
    Connectors,
    /// Diagnose this installation: config locations and precedence, key
    /// availability, database and migrations, model cache freshness,
    /// connectors, and terminal capabilities. Never prints a secret.
    Doctor,
    /// Print the local, content-free telemetry and analytics records.
    Telemetry,
}

#[derive(Args, Clone, Default)]
struct SessionArgs {
    /// Session name to create or resume.
    #[arg(long)]
    session: Option<String>,
    /// Provider profile. Defaults to the configured default.
    #[arg(long)]
    profile: Option<String>,
    /// Model id or alias. Defaults to the configured default.
    #[arg(long)]
    model: Option<String>,
    /// Start in the planning phase (conversation only).
    #[arg(long, conflicts_with = "code")]
    plan: bool,
    /// Start in the coding phase (tools available).
    #[arg(long)]
    code: bool,
    /// Approve every tool request without asking.
    #[arg(long, conflicts_with_all = ["deny_all", "ask"])]
    approve_all: bool,
    /// Deny every tool request that would ask.
    #[arg(long, conflicts_with = "ask")]
    deny_all: bool,
    /// Ask on the terminal. The default when stdin is a terminal.
    #[arg(long)]
    ask: bool,
    /// Skip the model list refresh.
    #[arg(long)]
    no_models: bool,
    /// Run through an installed agent instead of the native path:
    /// codex, claude, cursor, or opencode.
    #[arg(long, value_name = "NAME")]
    connector: Option<String>,
}

#[derive(Subcommand)]
enum SessionCommand {
    /// List sessions.
    List,
    /// Show a session's events.
    Show { name: String },
    /// Rename a session.
    Rename { name: String, new_name: String },
    /// Remove a session and its events.
    Remove { name: String },
}

impl gritt_provider::adapter::KeyProvider
    for keys::KeyResolver<keys::SystemKeychain, keys::ProcessEnv>
{
    fn key(
        &self,
        profile: &str,
        reference: &gritt_core::secret::SecretRef,
    ) -> Result<gritt_core::secret::Secret> {
        self.resolve(profile, reference)
    }
}

fn resolver() -> keys::KeyResolver<keys::SystemKeychain, keys::ProcessEnv> {
    keys::KeyResolver {
        keychain: keys::SystemKeychain,
        env: keys::ProcessEnv,
    }
}

async fn open_store(workspace: &Path, database: Option<&Path>) -> Result<Arc<Store>> {
    let location = resolve_location(workspace, database)?;
    Ok(Arc::new(Store::open(location).await?))
}

async fn builder(
    workspace: &Path,
    database: Option<&Path>,
    args: &SessionArgs,
) -> Result<AgentBuilder> {
    let config = config::load(workspace, std::env::vars())?;
    let store = open_store(workspace, database).await?;
    let telemetry = Arc::new(Telemetry::new(Arc::clone(&store), config.logging.clone()));
    // Retention applies whether or not logging is still on: turning it off
    // must not preserve old content past the window.
    telemetry.purge_content(chrono::Utc::now()).await?;
    let approval = if args.approve_all {
        ApprovalMode::ApproveAll
    } else if args.deny_all {
        ApprovalMode::DenyAll
    } else if args.ask || std::io::stdin().is_terminal() {
        ApprovalMode::Ask
    } else {
        ApprovalMode::DenyAll
    };
    let cache = if args.no_models {
        None
    } else {
        ModelCache::default_dir().map(ModelCache::new)
    };
    Ok(AgentBuilder {
        config,
        store,
        telemetry,
        keys: Arc::new(resolver()),
        transport: Arc::new(ReqwestTransport::new()?),
        catalog: ModelCatalog::new(),
        cache,
        workspace: Workspace::open(workspace)?,
        approval,
    })
}

/// Wraps the builder in the control plane with every external connector
/// configured from the settings. Key values the resolver knows and every
/// credential-like variable in this process's environment are redacted
/// out of connector output; the agents themselves keep their environment
/// (ADR-010). A credential-bearing `extra_args` entry is refused here.
fn plane(builder: AgentBuilder) -> Result<ControlPlane> {
    let blocked: Vec<String> = builder
        .config
        .profiles
        .values()
        .map(|profile| profile.key.env_var_name.clone())
        .collect();
    let mut secrets: Vec<gritt_core::secret::Secret> = builder
        .config
        .profiles
        .iter()
        .filter_map(|(name, profile)| builder.keys.key(name, &profile.key).ok())
        .collect();
    secrets.extend(environment_secrets(&blocked));
    let external = default_connectors_with_secrets(&builder.config.connectors, secrets)?;
    let file_setup = Arc::new(setup::FileSetup::new(builder.workspace_root(), resolver()));
    Ok(ControlPlane::new(Arc::new(builder), external).with_setup(file_setup))
}

/// The native approval flags mean nothing on an external agent, which
/// applies its own policy (ADR-010). Says so instead of silently
/// accepting `--deny-all` next to externally authorized tool execution.
fn approval_flag_warning(args: &SessionArgs, connector: Option<ConnectorId>) -> Option<String> {
    let id = connector.filter(|id| *id != ConnectorId::Native)?;
    let flag = if args.approve_all {
        "--approve-all"
    } else if args.deny_all {
        "--deny-all"
    } else if args.ask {
        "--ask"
    } else {
        return None;
    };
    Some(format!(
        "warning: {flag} has no effect with --connector {}: that agent applies its own approval policy; \
         pass its permission flags through `[connectors.extra_args]` in the Gritt config",
        id.as_str()
    ))
}

fn warn_approval_flags(args: &SessionArgs, connector: Option<ConnectorId>) {
    if let Some(warning) = approval_flag_warning(args, connector) {
        eprintln!("{warning}");
    }
}

fn connector_flag(args: &SessionArgs) -> Result<Option<ConnectorId>> {
    match &args.connector {
        None => Ok(None),
        Some(name) => parse_connector_id(name).map(Some).ok_or_else(|| {
            Error::config(format!(
                "unknown connector `{name}`; use native, codex, claude, cursor, or opencode"
            ))
        }),
    }
}

fn phase_flag(args: &SessionArgs) -> Option<Phase> {
    if args.plan {
        Some(Phase::Planning)
    } else if args.code {
        Some(Phase::Coding)
    } else {
        None
    }
}

fn selector(args: &SessionArgs) -> SessionSelector {
    match &args.session {
        Some(name) => SessionSelector::Named(name.clone()),
        None => SessionSelector::New { name: None },
    }
}

/// Loads the model list for the profile the session will actually use
/// (a resumed session's own profile, or the one the alias or qualified
/// model resolves to), reporting a stale or missing list on stderr
/// without stopping.
async fn warm_catalog(builder: &AgentBuilder, args: &SessionArgs) {
    let profile = match builder
        .session_profile(
            &selector(args),
            args.profile.as_deref(),
            args.model.as_deref(),
        )
        .await
    {
        Ok(profile) => profile,
        // `open` reports the same problem with its full context.
        Err(_) => return,
    };
    match builder.load_catalog(&profile).await {
        Ok(None) => {
            if let Some(gritt_core::provider::ModelListStatus::Stale { fetched_at }) =
                builder.catalog.status(&profile)
            {
                eprintln!(
                    "warning: using the stale model list for `{profile}` cached at {fetched_at}"
                );
            }
        }
        Ok(Some(error)) => eprintln!("warning: {error}; capabilities are unreported"),
        Err(error) => eprintln!("warning: {error}"),
    }
}

/// Answers approvals from the shared stdin owner. The harness prompter
/// captures the running turn's cancel handle up front, so a cancelled
/// approval gives up and the next typed line reaches the loop.
fn stdin_prompter(input: LineInput, slot: CancelSlot) -> gritt_harness::modes::print::Prompter {
    line_prompter(input, slot, || {
        eprint!("approve? [y/N] ");
        let _ = std::io::stderr().flush();
    })
}

fn print_options(
    verbose: bool,
    approval: ApprovalMode,
    input: &LineInput,
    slot: &CancelSlot,
) -> PrintUiOptions {
    match approval {
        ApprovalMode::Ask => PrintUiOptions {
            verbose,
            prompter: stdin_prompter(input.clone(), Arc::clone(slot)),
        },
        ApprovalMode::ApproveAll => PrintUiOptions {
            verbose,
            prompter: Arc::new(|_, _, _| ApprovalDecision::Approved),
        },
        ApprovalMode::DenyAll => PrintUiOptions::deny_all(verbose),
    }
}

/// Cancels the running turn on Ctrl-C; a Ctrl-C with nothing running
/// exits.
fn install_ctrl_c(slot: CancelSlot) {
    tokio::spawn(async move {
        loop {
            if tokio::signal::ctrl_c().await.is_err() {
                return;
            }
            let handle = slot.lock().expect("cancel slot").clone();
            match handle {
                Some(handle) if !handle.is_cancelled() => {
                    eprintln!("\ncancelling...");
                    handle.cancel();
                }
                _ => std::process::exit(130),
            }
        }
    });
}

async fn run_print(
    workspace: &Path,
    database: Option<&Path>,
    prompt: &str,
    args: &SessionArgs,
    verbose: bool,
) -> Result<ExitCode> {
    let connector = connector_flag(args)?;
    warn_approval_flags(args, connector);
    let builder = builder(workspace, database, args).await?;
    if connector.is_none() {
        warm_catalog(&builder, args).await;
    }
    let approval = builder.approval;
    let plane = plane(builder)?;
    let mut agent = plane
        .open(
            selector(args),
            connector,
            args.profile.as_deref(),
            args.model.as_deref(),
            phase_flag(args),
        )
        .await?;
    let slot: CancelSlot = Arc::new(Mutex::new(Some(agent.handle())));
    install_ctrl_c(Arc::clone(&slot));
    let input = LineInput::from_reader(std::io::BufReader::new(std::io::stdin()));
    let mut ui = PrintUi::new(
        std::io::stdout(),
        std::io::stderr(),
        print_options(verbose, approval, &input, &slot),
    );
    // The interface already showed a failed turn's error event.
    let outcome = match agent.run_turn(prompt, &mut ui).await {
        Ok(outcome) => outcome,
        Err(error) => {
            ui.finish();
            return Ok(if error.kind == gritt_core::ErrorKind::Cancelled {
                ExitCode::from(130)
            } else {
                ExitCode::FAILURE
            });
        }
    };
    ui.finish();
    // The closing newline is the last write; a pipe that broke there
    // still means the reader did not get the whole answer.
    if let Some(message) = ui.output_error() {
        eprintln!("error: output failed: {message}");
        return Ok(ExitCode::FAILURE);
    }
    if verbose {
        eprintln!(
            "session: {} ({})",
            agent.session().name,
            agent.session().id.0
        );
    }
    Ok(match outcome.status {
        TurnStatus::Completed => ExitCode::SUCCESS,
        TurnStatus::Cancelled => ExitCode::from(130),
        TurnStatus::Failed => ExitCode::FAILURE,
    })
}

async fn run_repl_mode(
    workspace: &Path,
    database: Option<&Path>,
    args: &SessionArgs,
    verbose: bool,
) -> Result<ExitCode> {
    let connector = connector_flag(args)?;
    warn_approval_flags(args, connector);
    let builder = builder(workspace, database, args).await?;
    if connector.is_none() {
        warm_catalog(&builder, args).await;
    }
    let approval = builder.approval;
    let plane = plane(builder)?;
    let agent = plane
        .open(
            selector(args),
            connector,
            args.profile.as_deref(),
            args.model.as_deref(),
            phase_flag(args),
        )
        .await?;
    let slot: CancelSlot = Arc::new(Mutex::new(None));
    install_ctrl_c(Arc::clone(&slot));
    println!("gritt repl: {} (/help for commands)", agent.session().name);
    // One reader owns stdin: the loop takes commands from it and the
    // approval prompter takes answers from it, never both at once.
    let input = LineInput::from_reader(std::io::BufReader::new(std::io::stdin()));
    let options = print_options(verbose, approval, &input, &slot);
    run_repl(
        &plane,
        agent,
        &input,
        std::io::stdout(),
        std::io::stderr(),
        options,
        slot,
    )
    .await?;
    Ok(ExitCode::SUCCESS)
}

async fn run_tui_mode(
    workspace: &Path,
    database: Option<&Path>,
    args: &SessionArgs,
) -> Result<ExitCode> {
    let connector = connector_flag(args)?;
    warn_approval_flags(args, connector);
    let mut builder = builder(workspace, database, args).await?;
    // The full-screen mode answers approvals itself.
    if builder.approval == ApprovalMode::DenyAll && !args.deny_all {
        builder.approval = ApprovalMode::Ask;
    }
    if connector.is_none() {
        warm_catalog(&builder, args).await;
    }
    let plane = plane(builder)?;
    let agent = plane
        .open(
            selector(args),
            connector,
            args.profile.as_deref(),
            args.model.as_deref(),
            phase_flag(args),
        )
        .await?;
    run_tui(&plane, agent).await?;
    Ok(ExitCode::SUCCESS)
}

async fn session_command(
    workspace: &Path,
    database: Option<&Path>,
    command: SessionCommand,
) -> Result<ExitCode> {
    let store = open_store(workspace, database).await?;
    let config = config::load(workspace, std::env::vars())?;
    Telemetry::new(Arc::clone(&store), config.logging.clone())
        .purge_content(chrono::Utc::now())
        .await?;
    match command {
        SessionCommand::List => {
            for session in store.list().await? {
                println!(
                    "{:<24} {:<10} {:<40} {}",
                    session.name,
                    format!("{:?}", session.phase).to_lowercase(),
                    match &session.kind {
                        gritt_core::session::SessionKind::Native {
                            provider_profile,
                            model,
                            ..
                        } => format!("{provider_profile}/{model}"),
                        gritt_core::session::SessionKind::Connector { id } =>
                            format!("connector:{}", id.as_str()),
                    },
                    session.updated_at.to_rfc3339()
                );
            }
        }
        SessionCommand::Show { name } => {
            let session = store
                .find_by_name(&name)
                .await?
                .ok_or_else(|| Error::config(format!("no session named `{name}`")))?;
            println!(
                "{} {} {:?} {}",
                session.name,
                session.id.0,
                session.phase,
                session.workspace.display()
            );
            for event in store.read_events(&session.id).await? {
                let line = match &event.kind {
                    EventKind::TextDelta { text } => format!("text {text:?}"),
                    EventKind::ToolCall { call } => format!("tool_call {}", call.name),
                    EventKind::ToolResult { result } => {
                        format!("tool_result {} error={}", result.name, result.is_error)
                    }
                    EventKind::ApprovalRequested { request } => {
                        format!("approval_requested {} {}", request.tool, request.resource)
                    }
                    EventKind::ApprovalDecided { decision, .. } => {
                        format!("approval {decision:?}")
                    }
                    EventKind::StatusChanged { status } => format!("status {status:?}"),
                    EventKind::Error { message, .. } => format!("error {message}"),
                    other => format!("{other:?}"),
                };
                println!(
                    "{:>5} {} {}",
                    event.sequence,
                    event.timestamp.format("%H:%M:%S"),
                    line
                );
            }
        }
        SessionCommand::Rename { name, new_name } => {
            let session = store
                .find_by_name(&name)
                .await?
                .ok_or_else(|| Error::config(format!("no session named `{name}`")))?;
            store.rename(&session.id, new_name.clone()).await?;
            println!("renamed `{name}` to `{new_name}`");
        }
        SessionCommand::Remove { name } => {
            let session = store
                .find_by_name(&name)
                .await?
                .ok_or_else(|| Error::config(format!("no session named `{name}`")))?;
            store.remove(&session.id).await?;
            println!("removed `{name}`");
        }
    }
    Ok(ExitCode::SUCCESS)
}

async fn connectors_command(workspace: &Path, database: Option<&Path>) -> Result<ExitCode> {
    let args = SessionArgs {
        session: None,
        profile: None,
        model: None,
        plan: false,
        code: false,
        approve_all: false,
        deny_all: true,
        ask: false,
        no_models: true,
        connector: None,
    };
    let builder = builder(workspace, database, &args).await?;
    let plane = plane(builder)?;
    println!("connector    state          version    transport        capabilities");
    for (id, info) in plane.infos().await {
        match info {
            Ok(info) => {
                let state = match info.auth {
                    AuthState::NotInstalled => "not installed".to_owned(),
                    AuthState::Authenticated => "authenticated".to_owned(),
                    AuthState::Unauthenticated => "no auth".to_owned(),
                    AuthState::Unknown => "installed".to_owned(),
                };
                let mut capabilities = Vec::new();
                if info.capabilities.structured_events {
                    capabilities.push("events");
                }
                if info.capabilities.follow_up_input {
                    capabilities.push("follow-up");
                }
                if info.capabilities.approvals {
                    capabilities.push("approvals");
                } else if info.auth != AuthState::NotInstalled {
                    capabilities.push("own-approvals");
                }
                if info.capabilities.cancel {
                    capabilities.push("cancel");
                }
                if info.capabilities.resume {
                    capabilities.push("resume");
                }
                if info.capabilities.inspect {
                    capabilities.push("inspect");
                }
                println!(
                    "{:<12} {:<14} {:<10} {:<16} {}",
                    id.as_str(),
                    state,
                    info.version.as_deref().unwrap_or("-"),
                    format!("{:?}", info.transport).to_lowercase(),
                    capabilities.join(",")
                );
            }
            Err(error) => println!("{:<12} {:<14} {}", id.as_str(), "error", error.message),
        }
    }
    Ok(ExitCode::SUCCESS)
}

async fn doctor_command(workspace: &Path, database: Option<&Path>) -> Result<ExitCode> {
    let store = open_store(workspace, database).await?;
    let args = SessionArgs {
        session: None,
        profile: None,
        model: None,
        plan: false,
        code: false,
        approve_all: false,
        deny_all: true,
        ask: false,
        no_models: true,
        connector: None,
    };
    // A broken config or connector setup is itself a finding, not a reason
    // to print nothing.
    let (plane, config_error) = match builder(workspace, database, &args).await {
        Ok(builder) => match plane(builder) {
            Ok(plane) => (Some(plane), None),
            Err(error) => (None, Some(error.message)),
        },
        Err(error) => (None, Some(error.message)),
    };
    let report = doctor::report(workspace, &store, plane.as_ref(), config_error.as_deref()).await?;
    for line in report.lines {
        println!("{line}");
    }
    Ok(ExitCode::SUCCESS)
}

async fn telemetry_command(workspace: &Path, database: Option<&Path>) -> Result<ExitCode> {
    let store = open_store(workspace, database).await?;
    let config = config::load(workspace, std::env::vars())?;
    let telemetry = Telemetry::new(Arc::clone(&store), config.logging.clone());
    telemetry.purge_content(chrono::Utc::now()).await?;
    print!("{}", telemetry.dump_text().await?);
    Ok(ExitCode::SUCCESS)
}

fn show_config(workspace: &Path) -> Result<ExitCode> {
    let config = config::load(workspace, std::env::vars())?;
    println!("workspace: {}", workspace.display());
    println!("profiles: {}", config.profiles.len());
    println!(
        "default: {}/{}",
        config.default_profile.as_deref().unwrap_or("-"),
        config.default_model.as_deref().unwrap_or("-")
    );
    println!(
        "content logging: {} ({} day retention)",
        config.logging.content_logging, config.logging.content_retention_days
    );
    let resolver = resolver();
    for (name, profile) in &config.profiles {
        // Only availability is reported. The value never leaves the
        // resolver.
        let state = match resolver.resolve(name, &profile.key) {
            Ok(_) => "key available".to_string(),
            Err(error) => error.message,
        };
        println!(
            "profile {name}: {:?} {} ({state})",
            profile.protocol, profile.base_url
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn key_set(workspace: &Path, profile: &str) -> Result<ExitCode> {
    let config = config::load(workspace, std::env::vars())?;
    let found = config
        .profiles
        .get(profile)
        .ok_or_else(|| Error::config(format!("unknown profile `{profile}`")))?;
    let mut value = String::new();
    let stdin = std::io::stdin();
    if stdin.lock().read_line(&mut value).is_err() || value.trim().is_empty() {
        return Err(Error::config("read an empty key from stdin"));
    }
    let secret = gritt_core::secret::Secret::new(value.trim());
    resolver().store(&found.key, &secret)?;
    println!("stored key for profile `{profile}` in the keychain");
    Ok(ExitCode::SUCCESS)
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let workspace = cli
        .workspace
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default();
    let database = cli.database.as_deref();
    let result = match cli.command {
        Some(Command::KeySet { profile }) => key_set(&workspace, &profile),
        Some(Command::Config) => show_config(&workspace),
        None => run_tui_mode(&workspace, database, &SessionArgs::default()).await,
        Some(Command::Run {
            prompt,
            session,
            verbose,
        }) => run_print(&workspace, database, &prompt, &session, verbose).await,
        Some(Command::Repl { session, verbose }) => {
            run_repl_mode(&workspace, database, &session, verbose).await
        }
        Some(Command::Tui { session }) => run_tui_mode(&workspace, database, &session).await,
        Some(Command::Session { command }) => session_command(&workspace, database, command).await,
        Some(Command::Connectors) => connectors_command(&workspace, database).await,
        Some(Command::Doctor) => doctor_command(&workspace, database).await,
        Some(Command::Telemetry) => telemetry_command(&workspace, database).await,
    };
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error}");
            if error.kind == gritt_core::ErrorKind::Cancelled {
                ExitCode::from(130)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(approve_all: bool, deny_all: bool, ask: bool, connector: Option<&str>) -> SessionArgs {
        SessionArgs {
            session: None,
            profile: None,
            model: None,
            plan: false,
            code: false,
            approve_all,
            deny_all,
            ask,
            no_models: true,
            connector: connector.map(str::to_owned),
        }
    }

    #[test]
    fn native_approval_flags_warn_only_on_external_connectors() {
        let warning = approval_flag_warning(
            &args(false, true, false, Some("codex")),
            Some(ConnectorId::Codex),
        )
        .unwrap();
        assert!(warning.contains("--deny-all has no effect with --connector codex"));
        assert!(warning.contains("own approval policy"));
        assert!(warning.contains("[connectors.extra_args]"));
        let warning = approval_flag_warning(
            &args(true, false, false, Some("claude")),
            Some(ConnectorId::ClaudeCode),
        )
        .unwrap();
        assert!(warning.contains("--approve-all"));
        assert!(approval_flag_warning(
            &args(false, false, true, Some("opencode")),
            Some(ConnectorId::OpenCode)
        )
        .is_some());
        assert!(approval_flag_warning(
            &args(false, false, false, Some("codex")),
            Some(ConnectorId::Codex)
        )
        .is_none());
        assert!(approval_flag_warning(&args(false, true, false, None), None).is_none());
        assert!(approval_flag_warning(
            &args(false, true, false, Some("native")),
            Some(ConnectorId::Native)
        )
        .is_none());
    }
}
