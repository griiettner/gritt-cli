use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use gritt_agent::memory::{db, index, mcp, search};
use gritt_agent::repo;
use gritt_agent::skill;
use gritt_agent::ticket;
use gritt_agent::Result;

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
    /// Ticket allocation, index sync, and validation.
    Ticket {
        #[command(subcommand)]
        command: TicketCommand,
    },
    /// Skill adapter generation.
    Skill {
        #[command(subcommand)]
        command: SkillCommand,
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

#[derive(Subcommand)]
enum SkillCommand {
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

fn run(cli: Cli) -> Result<i32> {
    let repo = repo::resolve_root(cli.repo_root.as_deref())?;
    match cli.command {
        Command::Memory { command } => match command {
            MemoryCommand::Index => {
                let summary = index::index_workspace(&repo)?;
                index::report(&summary);
                Ok(0)
            }
            MemoryCommand::Search { query, limit } => {
                let connection = db::open(&repo)?;
                let hits = search::search(&connection, &query, usize::from(limit))?;
                println!("{}", search::format_hits(&hits));
                Ok(0)
            }
            MemoryCommand::Serve => {
                mcp::serve(&repo)?;
                Ok(0)
            }
        },
        Command::Ticket { command } => match command {
            TicketCommand::New(args) => ticket::new::run(
                &repo,
                &ticket::new::NewOptions {
                    title: args.title,
                    namespace: args.namespace,
                    owner: args.owner,
                    create_concept: args.create_concept,
                    create_plan: args.create_plan,
                    no_sync: args.no_sync,
                    dry_run: args.dry_run,
                },
            ),
            TicketCommand::Sync { check } => ticket::sync::run(&repo, check),
            TicketCommand::Validate => ticket::validate::run(&repo),
        },
        Command::Skill { command } => match command {
            SkillCommand::Sync { check, prune } => {
                skill::sync::run(&repo, skill::sync::SyncOptions { check, prune })
            }
        },
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => ExitCode::from(code.clamp(0, 255) as u8),
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(error.exit_code.clamp(0, 255) as u8)
        }
    }
}
