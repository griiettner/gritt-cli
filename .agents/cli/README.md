# gritt-agent

`gritt-agent` is the project-local maintenance CLI for this repository. It
indexes and searches local memory, serves that memory over MCP, allocates and
validates tickets and ticket chains, regenerates ticket and memory indexes,
scaffolds skills and their generated adapters, manages the Codex trust entry,
and migrates `.cursor`/`.claude` setups. It replaced every Node script that
used to live under `.agents/tools/`.

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
| `ticket new-chain --title <title> --step <slug:title> ...` | Allocate consecutive ids for an orchestrator, one worker per `--step` (at least two), and a final reviewer; scaffold every `task.md` with `chain_role`, `chain_parent`, `chain_children`, and `dependencies`; sync indexes and roll back when the sync fails |
| `ticket identity [--refresh] [--namespace <login>] [--no-persist]` | Print the resolved GitHub login and its source, and store it in `.agents/state/identity.local.yaml` |
| `ticket chain-check --ticket <id> [--base <branch>] [--require-report] [--require-benchmark]` | Check a chain ticket's artifacts, report sections, branch and merge-base state, changed files, and benchmark evidence before semantic review |
| `ticket sync [--check]` | Regenerate `.agents/tasks/**/index.yaml` and `.agents/memory/*/index.yaml` |
| `ticket validate` | Check ticket folders, frontmatter, chain links, memory frontmatter, and the optional indexes |
| `skill new <name> <description> [--title <title>] [--force] [--no-openai] [--no-sync] [--dry-run]` | Scaffold `.agents/skills/<name>/SKILL.md` and `agents/openai.yaml`, then run `skill sync` |
| `skill sync [--check] [--prune]` | Regenerate `.claude/skills/*/SKILL.md` stubs and each skill's `agents/openai.yaml` policy block, keeping any leading `#` comment lines such as the migration marker |
| `skill audit [--skill NAME] [--strict]` | Read-only semantic checks for canonical skill metadata, local references, and output or verification contracts |
| `codex trust [path] [--check]` | Add or check the `trust_level = "trusted"` entry for a repository in `$CODEX_HOME/config.toml` (default `~/.codex`). Refuses only when the same path is keyed as a literal-string header or an inline table entry |
| `migrate cursor --source <path> [--dry-run] [--force] [--no-sync]` | Import `.cursor`/`.claude` skills, agents, and rules into `.agents/`, write `.agents/migrations/` reports, and run the maintenance commands (`skill sync` only when `.agents/skills/` exists) |

`ticket new` and `ticket new-chain` also accept `--namespace`, `--owner`,
`--areas`, `--skills`, `--dependencies`, `--create-concept`, `--create-plan`,
`--no-sync`, and `--dry-run`. The three list flags take zero or more values;
`ticket new` leaves them empty by default, `new-chain` defaults `--areas` to
`.agents/tasks` and `.agents/skills` and `--skills` to `tkt` and
`tkt-exec-chain`, and passing a list flag with no values clears it.
`new-chain` adds `--base-branch`, `--branch-pattern`, `--merge-policy`,
`--reviewer-title`, and `--no-reviewer`. `--branch-pattern` (default
`tkt-{id}-{step}-{slug}`) names every worker branch: `{id}` is the worker
ticket number, `{step}` the two-digit step, `{slug}` the step slug.
`new-chain` refuses to write when a folder already exists at any allocated
id, and removes everything it wrote when a later write fails. `--dry-run` on
either command writes nothing, not even the identity file. Both commands
render one shared
frontmatter block: `areas` and `skills` go on every artifact, `dependencies`
on `task.md` only. Identity resolution order is `--namespace`,
`GRITT_TKT_NAMESPACE`, `.agents/state/identity.local.yaml`, then
`gh api user`. `codex trust` never needs a repository root: it trusts the
positional path, else `--repo-root`, else the working directory, resolved to
its git top level when there is one.

`skill new` writes a sentence-case heading (`# Sample skill`) and a Title
Case Codex display name (`Sample Skill`); `--title` sets both verbatim.
`--no-openai` only holds when `--no-sync` is also passed, because `skill sync`
generates `agents/openai.yaml` for every skill. `--force` rewrites `SKILL.md`
and, unless `--no-openai` is passed, resets the `agents/openai.yaml` interface
block from the new description.

Commands that scaffold and then sync (`skill new`, `ticket new`,
`ticket new-chain`) print their own created-file lines first and the sync
summary last. `migrate cursor` captures its maintenance output into the
manifest instead of printing it.

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
  memory/            chunking, Turso schema, indexer, FTS search, MCP server
  ticket/            id and chunk rules, identity, allocation, shared scaffold frontmatter, chains, chain check, sync, validation
  skill/             skill scaffold, Claude stub and Codex metadata generation
  codex/             config.toml trust entry editing
  migrate/           Cursor and Claude setup import
tests/
  fixtures/repo/     small repository copied into a temp dir by every test
  fixtures/cursor-source/ a `.cursor`/`.claude` tree for the migration tests
  fixtures/expected/ committed outputs the commands must reproduce
  ticket.rs, skill.rs, codex.rs, migrate.rs, memory.rs, mcp.rs
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

`.github/workflows/agent-cli.yml` runs the same three commands on
`ubuntu-latest` and `windows-latest` for every push to `main` and every pull
request that touches `.agents/cli/`. The checkout disables `core.autocrlf` so
the LF fixtures compare byte for byte on Windows.

Frontmatter fences are split in one place, `frontmatter::split_fence`: the
opening `---` may end in `\n` or `\r\n`, and only a line that is `---`
after trailing whitespace is dropped closes the block, so `----` does not.
The ticket parser, `skill sync`, and `migrate cursor` all use it.

## Dependencies

Registry crates only, no Git dependencies:

| Crate | Purpose | License |
| --- | --- | --- |
| clap | argument parsing | MIT OR Apache-2.0 |
| tokio | async runtime for the embedded database | MIT |
| turso 0.7.2 (`sync` disabled) | local database and FTS engine | MIT |
| regex | validation patterns | MIT OR Apache-2.0 |
| serde, serde_json | MCP JSON-RPC messages | MIT OR Apache-2.0 |
| sha2 | content hashes | MIT OR Apache-2.0 |
| chrono | local dates and UTC timestamps | MIT OR Apache-2.0 |
| tempfile (dev) | temporary fixture repositories | MIT OR Apache-2.0 |

The MCP transport is implemented directly on JSON-RPC 2.0 rather than through
an SDK. It handles `initialize`, `ping`, `tools/list`, and `tools/call`, and
ignores notifications.
