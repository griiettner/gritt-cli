use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use gritt_agent::memory::{db, index, mcp, search};
use gritt_agent::ticket::new_chain::{
    DEFAULT_AREAS, DEFAULT_BASE_BRANCH, DEFAULT_BRANCH_PATTERN, DEFAULT_MERGE_POLICY,
    DEFAULT_SKILLS,
};
use gritt_agent::{codex, migrate, repo, skill, ticket, Result};

/// Project-local agent CLI for Gritt: local memory, tickets, and skills.
#[derive(Parser)]
#[command(name = "gritt-agent", version, about)]
struct Cli {
    /// Repository root. Defaults to the nearest ancestor containing `.agents/`.
    #[arg(long, global = true, value_name = "PATH")]
    repo_root: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Local memory index, search, and MCP server.
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
    /// Ticket allocation, chains, identity, index sync, and validation.
    Ticket {
        #[command(subcommand)]
        command: TicketCommand,
    },
    /// Skill scaffolding and adapter generation.
    Skill {
        #[command(subcommand)]
        command: SkillCommand,
    },
    /// Codex CLI configuration.
    Codex {
        #[command(subcommand)]
        command: CodexCommand,
    },
    /// Import other agent setups into `.agents/`.
    Migrate {
        #[command(subcommand)]
        command: MigrateCommand,
    },
}

#[derive(Subcommand)]
enum MemoryCommand {
    /// Index supported project documents into the local database.
    Index,
    /// Search indexed chunks and print citations.
    Search {
        /// Free-text query. Every term must match.
        query: String,
        /// Maximum number of results.
        #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u16).range(1..=50))]
        limit: u16,
    },
    /// Serve the local memory tools over MCP on stdin and stdout.
    Serve,
}

#[derive(Subcommand)]
enum TicketCommand {
    /// Create a ticket in the current developer's namespace.
    New(NewArgs),
    /// Create an orchestrator, worker, and reviewer ticket chain.
    NewChain(NewChainArgs),
    /// Resolve and store the GitHub login used as the ticket namespace.
    Identity(IdentityArgs),
    /// Run deterministic branch and ticket checks for chain review.
    ChainCheck(ChainCheckArgs),
    /// Regenerate ticket shard indexes, the chunk router, and memory indexes.
    Sync {
        /// Report generated-index drift without writing.
        #[arg(long)]
        check: bool,
    },
    /// Validate ticket folders, frontmatter, and chain links.
    Validate,
}

#[derive(Args)]
struct NewArgs {
    /// Ticket title.
    #[arg(long)]
    title: String,
    /// GitHub login override.
    #[arg(long)]
    namespace: Option<String>,
    /// Owner frontmatter value (default: the namespace).
    #[arg(long)]
    owner: Option<String>,
    /// Areas frontmatter list, written to every created artifact.
    #[arg(long, num_args = 0.., value_name = "ITEM")]
    areas: Vec<String>,
    /// Skills frontmatter list, written to every created artifact.
    #[arg(long, num_args = 0.., value_name = "ITEM")]
    skills: Vec<String>,
    /// Ticket ids this ticket depends on, written to task.md.
    #[arg(long, num_args = 0.., value_name = "ID")]
    dependencies: Vec<String>,
    /// Also create concept.md.
    #[arg(long)]
    create_concept: bool,
    /// Also create plan.md.
    #[arg(long)]
    create_plan: bool,
    /// Do not run the ticket index sync.
    #[arg(long)]
    no_sync: bool,
    /// Show the planned ticket without writing.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args)]
struct NewChainArgs {
    /// Orchestrator ticket title.
    #[arg(long)]
    title: String,
    /// Worker step as `slug:title`. Repeat the flag or pass several values; at least two are required.
    #[arg(long, num_args = 1.., value_name = "SLUG:TITLE")]
    step: Vec<String>,
    /// GitHub login override.
    #[arg(long)]
    namespace: Option<String>,
    /// Owner frontmatter value (default: the namespace).
    #[arg(long)]
    owner: Option<String>,
    /// Chain base branch.
    #[arg(long, default_value = DEFAULT_BASE_BRANCH)]
    base_branch: String,
    /// Worker branch pattern with {id}, {step}, and {slug} placeholders.
    #[arg(long, default_value = DEFAULT_BRANCH_PATTERN)]
    branch_pattern: String,
    /// Merge policy text recorded in task.md.
    #[arg(long, default_value = DEFAULT_MERGE_POLICY)]
    merge_policy: String,
    /// Final reviewer ticket title.
    #[arg(long)]
    reviewer_title: Option<String>,
    /// Do not create the final reviewer ticket.
    #[arg(long)]
    no_reviewer: bool,
    /// Skills frontmatter list. Pass the flag with no values to clear it.
    #[arg(long, num_args = 0.., value_name = "ITEM", default_values_t = DEFAULT_SKILLS.map(str::to_owned))]
    skills: Vec<String>,
    /// Areas frontmatter list. Pass the flag with no values to clear it.
    #[arg(long, num_args = 0.., value_name = "ITEM", default_values_t = DEFAULT_AREAS.map(str::to_owned))]
    areas: Vec<String>,
    /// Orchestrator ticket dependencies.
    #[arg(long, num_args = 0.., value_name = "ID")]
    dependencies: Vec<String>,
    /// Also create concept.md on the orchestrator.
    #[arg(long)]
    create_concept: bool,
    /// Also create plan.md on the orchestrator.
    #[arg(long)]
    create_plan: bool,
    /// Do not run the ticket index sync.
    #[arg(long)]
    no_sync: bool,
    /// Show the planned chain without writing.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args)]
struct IdentityArgs {
    /// Ignore the stored identity and query GitHub again.
    #[arg(long)]
    refresh: bool,
    /// Override the GitHub login.
    #[arg(long)]
    namespace: Option<String>,
    /// Print the login without writing identity.local.yaml.
    #[arg(long)]
    no_persist: bool,
}

#[derive(Args)]
struct ChainCheckArgs {
    /// Ticket id, for example TKT-0042 or login/TKT-0042.
    #[arg(long)]
    ticket: String,
    /// Base branch.
    #[arg(long, default_value = DEFAULT_BASE_BRANCH)]
    base: String,
    /// Treat a missing report.md as an error.
    #[arg(long)]
    require_report: bool,
    /// Require explicit benchmark evidence.
    #[arg(long)]
    require_benchmark: bool,
}

#[derive(Subcommand)]
enum SkillCommand {
    /// Create a project-local skill with its Codex metadata.
    New(SkillNewArgs),
    /// Regenerate Claude Code stubs and Codex policy metadata.
    Sync {
        /// Report drift and exit non-zero instead of writing.
        #[arg(long)]
        check: bool,
        /// Report user-owned Claude skill dirs without a canonical skill.
        #[arg(long)]
        prune: bool,
    },
}

#[derive(Args)]
struct SkillNewArgs {
    /// Skill name; normalized to lowercase kebab-case.
    name: String,
    /// Skill discovery description.
    description: String,
    /// Human-readable heading and display name.
    #[arg(long)]
    title: Option<String>,
    /// Overwrite an existing skill, including its agents/openai.yaml interface unless --no-openai is passed.
    #[arg(long)]
    force: bool,
    /// Do not write agents/openai.yaml. Holds only with --no-sync: `skill sync` regenerates the file for every skill.
    #[arg(long)]
    no_openai: bool,
    /// Do not refresh generated adapters.
    #[arg(long)]
    no_sync: bool,
    /// Validate and show planned files without writing.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Subcommand)]
enum CodexCommand {
    /// Check or add the Codex trust entry for a repository.
    Trust(TrustArgs),
}

#[derive(Args)]
struct TrustArgs {
    /// Repository path to trust. Defaults to `--repo-root`, else the current directory.
    path: Option<PathBuf>,
    /// Only check trust state. Exit 0 when trusted, 1 when not trusted.
    #[arg(long)]
    check: bool,
}

#[derive(Subcommand)]
enum MigrateCommand {
    /// Migrate a `.cursor`/`.claude` setup into this repository's `.agents/`.
    Cursor(CursorArgs),
}

#[derive(Args)]
struct CursorArgs {
    /// Full path to the existing repository.
    #[arg(long, value_name = "PATH")]
    source: PathBuf,
    /// Plan and print a summary without writing files.
    #[arg(long)]
    dry_run: bool,
    /// Overwrite files not created by this migrator.
    #[arg(long)]
    force: bool,
    /// Do not run the gritt-agent maintenance commands.
    #[arg(long)]
    no_sync: bool,
}

async fn run(cli: Cli) -> Result<i32> {
    let resolve = || repo::resolve_root(cli.repo_root.as_deref());
    match cli.command {
        Command::Memory { command } => {
            let repo = resolve()?;
            match command {
                MemoryCommand::Index => {
                    let summary = index::index_workspace(&repo).await?;
                    index::report(&summary);
                    Ok(0)
                }
                MemoryCommand::Search { query, limit } => {
                    let connection = db::open(&repo).await?;
                    let hits = search::search(&connection, &query, usize::from(limit)).await?;
                    println!("{}", search::format_hits(&hits));
                    Ok(0)
                }
                MemoryCommand::Serve => {
                    mcp::serve(&repo).await?;
                    Ok(0)
                }
            }
        }
        Command::Ticket { command } => {
            let repo = resolve()?;
            match command {
                TicketCommand::New(args) => ticket::new::run(
                    &repo,
                    &ticket::new::NewOptions {
                        title: args.title,
                        namespace: args.namespace,
                        owner: args.owner,
                        areas: args.areas,
                        skills: args.skills,
                        dependencies: args.dependencies,
                        create_concept: args.create_concept,
                        create_plan: args.create_plan,
                        no_sync: args.no_sync,
                        dry_run: args.dry_run,
                    },
                ),
                TicketCommand::NewChain(args) => ticket::new_chain::run(
                    &repo,
                    &ticket::new_chain::NewChainOptions {
                        title: args.title,
                        steps: args.step,
                        namespace: args.namespace,
                        owner: args.owner,
                        base_branch: args.base_branch,
                        branch_pattern: args.branch_pattern,
                        merge_policy: args.merge_policy,
                        reviewer_title: args.reviewer_title,
                        no_reviewer: args.no_reviewer,
                        skills: args.skills,
                        areas: args.areas,
                        dependencies: args.dependencies,
                        create_concept: args.create_concept,
                        create_plan: args.create_plan,
                        no_sync: args.no_sync,
                        dry_run: args.dry_run,
                    },
                ),
                TicketCommand::Identity(args) => ticket::identity::run(
                    &repo,
                    &ticket::identity::ResolveOptions {
                        namespace: args.namespace,
                        refresh: args.refresh,
                        persist: !args.no_persist,
                    },
                ),
                TicketCommand::ChainCheck(args) => ticket::chain_check::run(
                    &repo,
                    &ticket::chain_check::ChainCheckOptions {
                        ticket: args.ticket,
                        base: args.base,
                        require_report: args.require_report,
                        require_benchmark: args.require_benchmark,
                    },
                ),
                TicketCommand::Sync { check } => ticket::sync::run(&repo, check),
                TicketCommand::Validate => ticket::validate::run(&repo),
            }
        }
        Command::Skill { command } => {
            let repo = resolve()?;
            match command {
                SkillCommand::New(args) => skill::new::run(
                    &repo,
                    &skill::new::NewOptions {
                        name: args.name,
                        description: args.description,
                        title: args.title,
                        force: args.force,
                        no_openai: args.no_openai,
                        no_sync: args.no_sync,
                        dry_run: args.dry_run,
                    },
                ),
                SkillCommand::Sync { check, prune } => {
                    skill::sync::run(&repo, skill::sync::SyncOptions { check, prune })
                }
            }
        }
        // `codex trust` names its target directly: the positional path, else
        // an explicit `--repo-root`, else the working directory, like the
        // script it replaced. It never needs `.agents/` to exist.
        Command::Codex {
            command: CodexCommand::Trust(args),
        } => {
            let project = args
                .path
                .or(cli.repo_root)
                .unwrap_or_else(|| PathBuf::from("."));
            codex::trust::run(&project, args.check)
        }
        Command::Migrate { command } => {
            let repo = resolve()?;
            match command {
                MigrateCommand::Cursor(args) => migrate::cursor::run(
                    &repo,
                    &migrate::cursor::CursorOptions {
                        source: args.source,
                        dry_run: args.dry_run,
                        force: args.force,
                        no_sync: args.no_sync,
                    },
                ),
            }
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli).await {
        Ok(code) => ExitCode::from(code.clamp(0, 255) as u8),
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(error.exit_code.clamp(0, 255) as u8)
        }
    }
}
