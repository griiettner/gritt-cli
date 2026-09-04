# gritt-agent

`gritt-agent` is the project-local maintenance CLI for this repository. It
indexes and searches local memory, serves that memory over MCP, allocates and
validates tickets, regenerates ticket and memory indexes, and produces the
generated skill adapters. It replaced the Node scripts that used to do this.

The crate is its own Cargo workspace root. The future product workspace at the
repository root does not include it, and it does not depend on product code.

## Build

Only the Rust toolchain is required:

```bash
cargo build --release --manifest-path .agents/cli/Cargo.toml
```

The binary is written to `.agents/cli/target/release/gritt-agent`. `.mcp.json`
points at that path, so build before starting an MCP client.

## Commands

Every command accepts `--repo-root <path>`. Without it the CLI walks up from
the working directory to the nearest folder that contains `.agents/`.

| Command | Purpose |
| --- | --- |
| `memory index` | Index `*.md`, `*.mdx`, `*.yaml`, `*.yml`, and `*.json` files into `.agents/brain/data/agent-memory.db`, removing entries for deleted files |
| `memory search <query> [--limit N]` | Print ranked chunk citations as `path:start-end` |
| `memory serve` | Reindex, then serve `search_local_memory` and `read_local_memory` over stdio MCP |
| `ticket new --title <title>` | Allocate the next contiguous id in the developer namespace, scaffold `task.md`, and sync indexes. Rolls back when the sync fails |
| `ticket sync [--check]` | Regenerate `.agents/tasks/**/index.yaml` and `.agents/memory/*/index.yaml` |
| `ticket validate` | Check ticket folders, frontmatter, chain links, memory frontmatter, and the optional indexes |
| `skill sync [--check] [--prune]` | Regenerate `.claude/skills/*/SKILL.md` stubs and each skill's `agents/openai.yaml` policy block |

`ticket new` also accepts `--namespace`, `--owner`, `--create-concept`,
`--create-plan`, `--no-sync`, and `--dry-run`. Identity resolution order is
`--namespace`, `GRITT_TKT_NAMESPACE`, `.agents/state/identity.local.yaml`, then
`gh api user`.

Exit codes: 0 on success, 1 on a failed check or operation, 2 on a usage or
identity error.

## Layout

```text
src/
  main.rs            clap command tree and exit codes
  lib.rs             module exports for the integration tests
  error.rs           CliError with exit code
  frontmatter.rs     restricted YAML frontmatter parser shared by tickets and memory
  fsx.rs             sorted directory listing, posix relative paths, file helpers
  repo.rs            repository root discovery and date helpers
  memory/            chunking, SQLite schema, indexer, FTS5 search, MCP server
  ticket/            id and chunk rules, identity, allocation, sync, validation
  skill/             Claude stub and Codex metadata generation
tests/
  fixtures/repo/     small repository copied into a temp dir by every test
  fixtures/expected/ committed outputs the commands must reproduce
  ticket.rs, skill.rs, memory.rs, mcp.rs
```

Directory listings are sorted case-insensitively so generated output does not
depend on the platform's raw byte order.

## Verify

```bash
cargo fmt --manifest-path .agents/cli/Cargo.toml --all --check
cargo clippy --manifest-path .agents/cli/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path .agents/cli/Cargo.toml
```

Integration tests spawn the built binary against a copy of
`tests/fixtures/repo/` and compare generated files with
`tests/fixtures/expected/`. Regenerate an expected file only when the output
format changes on purpose, and review the diff before committing it.

## Dependencies

Registry crates only, no Git dependencies:

| Crate | Purpose | License |
| --- | --- | --- |
| clap | argument parsing | MIT OR Apache-2.0 |
| rusqlite (`bundled`) | SQLite with FTS5 compiled in | MIT |
| regex | validation patterns | MIT OR Apache-2.0 |
| serde, serde_json | MCP JSON-RPC messages | MIT OR Apache-2.0 |
| sha2 | content hashes | MIT OR Apache-2.0 |
| chrono | local dates and UTC timestamps | MIT OR Apache-2.0 |
| tempfile (dev) | temporary fixture repositories | MIT OR Apache-2.0 |

The MCP transport is implemented directly on JSON-RPC 2.0 rather than through
an SDK. It handles `initialize`, `ping`, `tools/list`, and `tools/call`, and
ignores notifications.
