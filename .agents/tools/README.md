# Agent Tools

Repository maintenance runs through two layers.

The Rust CLI at `.agents/cli/` (binary `gritt-agent`) owns local memory,
ticket allocation, index generation, validation, and skill adapters. Build it
once per checkout:

```bash
cargo build --release --manifest-path .agents/cli/Cargo.toml
```

Then run:

```text
.agents/cli/target/release/gritt-agent memory index
.agents/cli/target/release/gritt-agent memory search "query terms"
.agents/cli/target/release/gritt-agent memory serve
.agents/cli/target/release/gritt-agent ticket new --title "Ticket title"
.agents/cli/target/release/gritt-agent ticket sync
.agents/cli/target/release/gritt-agent ticket validate
.agents/cli/target/release/gritt-agent skill sync
```

The Node scripts under `agent-tools/` cover the scaffolding and migration
commands that have no Rust replacement yet. They use only Node built-ins and
run directly with `node`:

```text
node .agents/tools/agent-tools/tkt-identity.mjs
node .agents/tools/agent-tools/tkt-new-chain.mjs --title "Ticket title" --step one:First step --step two:Second step
node .agents/tools/agent-tools/tkt-chain-check.mjs --ticket TKT-0008 --base main
node .agents/tools/agent-tools/create-skill.mjs skill-name "Skill description"
node .agents/tools/agent-tools/trust-codex-project.mjs --check
node .agents/tools/agent-tools/migrate-cursor-setup.mjs --source /path/to/repository --dry-run
```

When one of these scripts needs an index or adapter sync, it calls the
matching `gritt-agent` subcommand. The lookup order is `GRITT_AGENT_BIN`, the release build, the
debug build, then `cargo run` against `.agents/cli/Cargo.toml`.

Run all generated metadata maintenance through the `/tkt-sync` skill.

## Layout

- `../cli/` contains the Rust crate and its tests.
- `agent-tools/` contains the remaining cross-platform Node commands and the
  shared `lib/` helpers they import.

The tools use Node APIs and Git subprocesses directly, without Bash,
PowerShell, or operating-system-specific command syntax.
