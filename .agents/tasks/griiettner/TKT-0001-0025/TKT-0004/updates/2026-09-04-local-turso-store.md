---
id: TKT-0004
namespace: griiettner
title: Local Turso store
artifact: update
status: done
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
---

# TKT-0004 Update: Local Turso store

## Trigger

The user locked the storage boundary as fully local, with no Cloud sync.

## Changes

- Replaced `rusqlite` with `turso` 0.7.2 and `tokio` in `.agents/cli/`.
  Turso uses only its local `fts` feature. Its optional `sync` feature is
  disabled. Both dependencies are MIT licensed.
- Converted the memory database, indexer, search, MCP handling, and command
  entry point to Turso's asynchronous API.
- Replaced SQLite FTS5 virtual tables with Turso's Tantivy-backed FTS indexes.
  The adapter preserves all-term matching, deterministic heading preference,
  result limits, citation formatting, and the MCP tool contract.
- Replaced changed chunks through delete and insert within the indexing
  transaction. Turso FTS did not refresh reliably through the former SQLite
  upsert path.
- Added ADR-005 and updated every agent-brain document plus the CLI README to
  describe the shipped local Turso implementation.

## Validation

- `cargo fmt --manifest-path .agents/cli/Cargo.toml --all --check`: passed
  after formatting.
- `cargo clippy --manifest-path .agents/cli/Cargo.toml --all-targets -- -D warnings`:
  passed.
- `cargo test --manifest-path .agents/cli/Cargo.toml`: passed, including 5
  memory integration tests and the MCP integration test.
- `cargo build --release --manifest-path .agents/cli/Cargo.toml`: passed.
- `gritt-agent ticket validate`: passed with 0 warnings.
- `gritt-agent ticket sync --check`: passed with no drift before this update;
  rerun after the final ticket sync.
- A direct index of this checkout was not run because an existing
  `gritt-agent memory serve` process held the generated database lock. The
  integration suite exercised index and search against isolated local Turso
  files without network access.

## Completion Gate

- Acceptance: passed. The decisions are locked, the local engine is Turso,
  CLI and MCP behavior is covered, the docs and ADR match, and required build
  and test gates passed.
- Scope: passed. Changes stay within the memory subsystem, its entry point,
  dependencies, documentation, ADR, and ticket artifacts.
- Validation: passed. The direct checkout index check was unavailable because
  another local process held its generated cache, but equivalent integration
  coverage passed.
- Security and safety: passed. No network client, Cloud configuration,
  credential source, or secret-bearing field was added. Symlink exclusions
  and repository-relative indexing behavior are unchanged.
- Regression risk: the database API and FTS engine changed. Exact CLI output,
  incremental update and deletion behavior, symlink handling, and MCP calls
  are covered by existing integration tests. Turso FTS remains experimental,
  which ADR-005 records.
- Follow-up: restart the existing local memory MCP process so it loads the new
  release binary, then rebuild its generated cache if the legacy schema does
  not open cleanly.
- Assumptions: the local-only choice makes credentials and shared ownership
  not applicable. ADR-005 leaves ADR-004 intact as historical context. A Cloud
  choice would have required a separate trust boundary and configuration
  design.
