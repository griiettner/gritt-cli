mod common;

use common::{empty_repo, run};

/// Every documented command answers `--help`, which MIGRATION.md uses as the
/// readiness check and which a bad clap attribute would break at runtime.
#[test]
fn every_subcommand_prints_help() {
    let repo = empty_repo();
    let commands: [&[&str]; 14] = [
        &[],
        &["memory", "index"],
        &["memory", "search"],
        &["memory", "serve"],
        &["ticket", "new"],
        &["ticket", "new-chain"],
        &["ticket", "identity"],
        &["ticket", "chain-check"],
        &["ticket", "sync"],
        &["ticket", "validate"],
        &["skill", "new"],
        &["skill", "sync"],
        &["codex", "trust"],
        &["migrate", "cursor"],
    ];
    for command in commands {
        let help = run(&repo.root, &[command, &["--help"]].concat());
        help.assert_status(0);
        assert!(
            help.stdout.contains("Usage:"),
            "{command:?} printed no usage:\n{}",
            help.stdout
        );
    }
}
