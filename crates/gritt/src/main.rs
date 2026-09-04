//! The `gritt` binary: argument parsing, configuration, key loading, and
//! mode selection. Modes and sessions land in TKT-0011.

use std::process::ExitCode;

use clap::{Parser, Subcommand};

mod config;
mod keys;

/// Run native and installed AI coding agents from one local terminal.
#[derive(Parser)]
#[command(name = "gritt", version, about)]
struct Cli {
    /// Workspace directory. Defaults to the current directory.
    #[arg(long, global = true, value_name = "PATH")]
    workspace: Option<std::path::PathBuf>,

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
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let workspace = cli
        .workspace
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default();
    match cli.command {
        Some(Command::KeySet { profile }) => key_set(&workspace, &profile),
        Some(Command::Config) | None => match config::load(&workspace, std::env::vars()) {
            Ok(config) => {
                println!("workspace: {}", workspace.display());
                println!("profiles: {}", config.profiles.len());
                println!(
                    "default: {}/{}",
                    config.default_profile.as_deref().unwrap_or("-"),
                    config.default_model.as_deref().unwrap_or("-")
                );
                let resolver = keys::KeyResolver {
                    keychain: keys::SystemKeychain,
                    env: keys::ProcessEnv,
                };
                for (name, profile) in &config.profiles {
                    // Only availability is reported. The value never leaves
                    // the resolver.
                    let state = match resolver.resolve(name, &profile.key) {
                        Ok(_) => "key available".to_string(),
                        Err(error) => error.message,
                    };
                    println!(
                        "profile {name}: {:?} {} ({state})",
                        profile.protocol, profile.base_url
                    );
                }
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::FAILURE
            }
        },
    }
}

fn key_set(workspace: &std::path::Path, profile: &str) -> ExitCode {
    let config = match config::load(workspace, std::env::vars()) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let Some(found) = config.profiles.get(profile) else {
        eprintln!("error: unknown profile `{profile}`");
        return ExitCode::FAILURE;
    };
    let mut value = String::new();
    if std::io::stdin().read_line(&mut value).is_err() || value.trim().is_empty() {
        eprintln!("error: read an empty key from stdin");
        return ExitCode::FAILURE;
    }
    let secret = gritt_core::secret::Secret::new(value.trim());
    let resolver = keys::KeyResolver {
        keychain: keys::SystemKeychain,
        env: keys::ProcessEnv,
    };
    match resolver.store(&found.key, &secret) {
        Ok(()) => {
            println!("stored key for profile `{profile}` in the keychain");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
