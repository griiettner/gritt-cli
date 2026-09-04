---
id: TKT-0001
namespace: griiettner
title: Build project-local agent CLI
artifact: report
status: done
owner: griiettner
created: 2026-09-03
updated: 2026-09-03
---

# TKT-0001 Report: Build project-local agent CLI

## Summary

The `gritt-agent` Rust crate at `.agents/cli/` now owns local memory, ticket
metadata, and skill adapters. It is an independent workspace root with one
binary and no product dependencies. The command surface matches `plan.md`
exactly: `memory index`, `memory search`, `memory serve`, `ticket new`,
`ticket sync`, `ticket validate`, and `skill sync`. Every command takes a
global `--repo-root` and otherwise finds the root by walking up to the nearest
`.agents/` folder.

Parity was proven on this repository before removal. `ticket sync --check`
reported no drift against the committed indexes, `ticket validate` passed with
no warnings, and `skill sync` reproduced every `openai.yaml` byte for byte.
The only stub difference is the generated marker line, which now reads
`gritt-agent skill sync` instead of the stale Nx reference.

Removed Node code: the whole `.agents/tools/agent-memory/` tree (12 files) and
`tkt-new.mjs`, `tkt-sync.mjs`, `tkt-validate.mjs`, and `sync-skills.mjs` under
`agent-tools/`. The remaining Node scripts (`tkt-new-chain`, `create-skill`,
`migrate-cursor-setup`, `tkt-chain-check`, `tkt-identity`,
`trust-codex-project`) keep working through `runAgentCli` in
`agent-tools/lib/cli.mjs`, which maps the old targets to `gritt-agent`
subcommands.

Configuration and docs: `.mcp.json` now declares `gritt-local-memory` as the
release binary running `memory serve`; the root `.gitignore` excludes
`.agents/brain/data/`, `.agents/cli/target/`, and `.agents/.env`;
`.agents/settings.json` allows the binary and `cargo` with the crate manifest.
`AGENTS.md`, `README.md`, `MIGRATION.md`, the brain docs, the tool READMEs,
the Cursor rule, the Claude command, and the `tkt`, `tkt/store`, `tkt-sync`,
`tkt-new`, `tkt-new-chain`, `skill-management`, `memory-write`, and `reflect`
skills reference the new commands.

## Key Decisions

- MCP is implemented directly on JSON-RPC 2.0 over stdio (`initialize`,
  `ping`, `tools/list`, `tools/call`, notifications ignored). No MCP SDK or
  async runtime was added.
- `.mcp.json` points at `.agents/cli/target/release/gritt-agent`, so the
  server works even when `cargo` is not on the host application's PATH. The
  cost is one documented `cargo build --release` per checkout.
- The Node shim resolves the binary in this order: `GRITT_AGENT_BIN`, release
  build, debug build, then `cargo run --release --manifest-path`.
- The generated stub marker changed and the old Nx marker was added to the
  legacy list so existing stubs are still recognized as generated.
- `target` joined the indexer's skipped directories because the crate's build
  output lives inside the repository and contains JSON fingerprint files.
- `source_mtime` stores the file's real modification time in milliseconds.
  The Node indexer stored the index time under that name.
- `Cargo.lock` is committed because the crate is a binary.
- Dependencies, all registry, checked on crates.io on 2026-09-03: clap 4.6.6
  (MIT OR Apache-2.0), rusqlite 0.40.2 with `bundled` (MIT; ships SQLite with
  FTS5), regex 1.13.1, serde 1.0.229, serde_json 1.0.151, sha2 0.11.0,
  chrono 0.4.45 with `clock` and `std` only, and tempfile 3.27.0 as a dev
  dependency (all MIT OR Apache-2.0). `rust-version` is 1.85.

## Alternatives Considered

- The `rmcp` SDK for MCP. Rejected: it pulls tokio and a large API surface
  for two tools.
- `cargo run` in `.mcp.json`. Rejected: GUI hosts often lack `~/.cargo/bin`
  on PATH, and a cold compile can exceed client startup timeouts.
- Keeping the Nx marker in stubs. Rejected: it named a tool that no longer
  exists. The change touched 16 stub files but nothing else in them.
- Keeping `dashboard.mjs`, `gateway.mjs`, and `provider-check.mjs`. Rejected:
  they import the removed `db.mjs` and `config.mjs`, depended on packages that
  were never declared in this checkout, and their features are out of scope.
- A positional tasks root for `ticket validate`, as the old script had.
  Rejected in favor of the global `--repo-root`.

## Assumptions

- The out-of-scope memory features (dashboard, embeddings, reranking,
  provider gateway) are gone with the Node tree rather than left as dead
  files. A different choice would have kept unrunnable code in the repo.
- `tkt-identity.mjs` and `agent-tools/lib/` stay because the chain and
  scaffold scripts import them.
- Directory listings sort case-insensitively, then by bytes, to approximate
  the `localeCompare` order the Node tools used. Every file in this repository
  sorts identically under both rules.
- Scaffold headings keep the Node capitalization (`# TKT-0002 Task: ...`).
  The hand-written ticket in this repo uses lowercase `task:`; validation does
  not check heading text.
- `.agents/brain/data/.gitignore` was left untouched. The root `.gitignore`
  now ignores the directory, which is what the docs promised.
- The report sets `task.md` to `status: done` so the lifecycle gate in
  `tkt-exec` reads the finished state from the task artifact too.

## Edge Cases and Failures

- The first index run on this repository counted 264 files because
  `.agents/cli/target/**` contains JSON fingerprints. Skipping `target` brought
  it to 112.
- Two integration test expectations were wrong on the first run, not the code:
  a limit of 2 excludes the fourth-ranked guide chunk, and the custom skill
  fixture has no `disable-model-invocation`, so its policy line legitimately
  flips to `true` and four files change rather than three.
- The MCP integration test first failed to compile because a closure borrowed
  stdin while the test also wrote a notification. A free function now writes
  the request and reads the response.
- macOS temporary directories are symlinks under `/var`. `--repo-root` is
  canonicalized so relative citations stay consistent.

## Validation

Run from the repository root on macOS (arm64):

- `cargo fmt --manifest-path .agents/cli/Cargo.toml --all --check`: pass.
- `cargo clippy --manifest-path .agents/cli/Cargo.toml --all-targets -- -D warnings`: pass.
- `cargo test --manifest-path .agents/cli/Cargo.toml`: 25 unit and 23
  integration tests pass. Integration tests copy `tests/fixtures/repo/` into a
  temp dir and compare indexes, validation output, stubs, `openai.yaml`, and
  search citations with `tests/fixtures/expected/`. The MCP test drives the
  binary over stdio through `initialize`, `tools/list`, both tools, and `ping`.
- Real repository: `ticket sync --check` no drift, `ticket validate` ok with 0
  warnings, `skill sync --check` no drift after regeneration, `memory index`
  112 files, `memory search` returns cited chunks, and a manual MCP
  `initialize` plus `search_local_memory` call through the release binary.
- `node --test .agents/tools/agent-tools/agent-tools.test.mjs`: 5 pass. The
  six tests that exercised removed scripts moved to the Rust suite.
- Shim smoke test: `runAgentCli` for `tkt-validate`, `tkt-sync`, and
  `sync-skills` returned status 0 on this repository.
- Not run: builds or tests on Linux and Windows.

## Review

The harness code review ran eight finder angles over the diff. Fixed in this
change:

- The indexer followed symlinked directories, which the Node walker never did
  and which could pull files from outside the repository into the database.
  Symlinked directories are now skipped; a test covers an outside link and a
  self-referential loop.
- `ticket sync` wrote shard and router indexes before checking frontmatter
  errors, so a rolled-back `ticket new` left indexes advertising the deleted
  id. Sync now collects everything and fails before writing anything.
- A title starting with `[` or `{` produced frontmatter the parser rejects.
  `ticket new` quotes such titles; the value round-trips unchanged.
- MCP `limit` rejected integral floats such as `5.0`; they are accepted now.
- The Node shim's target map and positional filter could drop flag values.
  Callers now pass the real subcommand, and a spawn failure is printed
  instead of vanishing behind an inherited stdio status.
- Chain scaffolds, the reflect, tkt/store, tkt-new-chain, and
  skill-management skills, the migration help text, and the tools README
  still named Nx targets or a chain example that the tool rejects. All point
  at working commands.
- The cargo permission allowed every cargo verb; it now allows build, test,
  clippy, and fmt only.
- ADR-004 records the CLI location, the FTS5 baseline, and the binary-path
  MCP configuration, per the AGENTS.md rule on architecture decisions.
- Cleanup: unused `serde` dependency, three unused functions, a dead Node
  test helper, and a second read of every artifact during validation.

Deferred, recorded under Follow-up: the incremental FTS strategy, per-ticket
file reads in sync, the remaining duplicated ticket-store logic in Node, and
a launcher that builds the binary on demand.

## Completion Gate

- Acceptance: yes. Clean checkout builds with Rust only; incremental index
  with deletion; ranked search with `path:line` citations; MCP with both
  tools; contiguous allocation with rollback on sync failure; sync and
  validation reproduce the committed indexes and rules; stubs and Codex
  policy reproduced; writes stay inside the repository and no secrets are
  read or stored; replaced Node scripts and npm instructions removed.
- Scope: yes, with one recorded extension. The remaining Node scripts needed
  a small shim to keep calling sync, and the `.gitignore` gained three entries
  so the database and build output are not committed.
- Validation: pass on macOS. Linux and Windows not run.
- Security and safety: no network access; the CLI runs `git` and `gh`
  subprocesses only for root and identity discovery; SQL uses bound
  parameters; the database stores document text only; `ticket new` removes
  only the folder it just created.
- Regression risk: low for indexes and stubs (byte parity proven). Medium for
  MCP clients until the binary is built, because `.mcp.json` names a build
  artifact. Node scaffold scripts now depend on the binary or `cargo`.
- Follow-up: see below.
- Assumptions: see above.

## Follow-up

- Port `tkt-identity`, `tkt-new-chain`, `tkt-chain-check`, `create-skill`,
  `migrate-cursor-setup`, and `trust-codex-project` so Node can be dropped.
- Add CI that runs the crate verify set on Linux and Windows.
- `.agents/state/identity.local.yaml` is per-developer but tracked in Git.
- No `.cursor/mcp.json` exists. Add one if Cursor is used with this server.
- Dashboard, embeddings, and reranking are gone; open tickets if wanted.
- `lib/tkt-store.mjs`, `lib/tkt-identity.mjs`, and `frontmatter-utils.mjs`
  duplicate Rust logic for the chain scripts. Porting the chain commands
  removes the duplication.
- Performance on large repositories: `memory index` rebuilds both FTS tables
  on every run and hashes every file; `ticket sync` reads each artifact more
  than once. Neither matters at this repository's size.
- A checked-in launcher that builds the binary when it is missing or stale
  would let `.mcp.json`, the shim, and the docs share one path.
- The `MIGRATED BY nx run agent-tools:migrate-cursor-setup` marker in
  `migrate-cursor-setup.mjs` is an ownership key for migrated files and was
  left unchanged for compatibility.

## Updates

- [2026-09-03 reject symlinks during repository traversal](updates/2026-09-03-symlink-boundary.md)
