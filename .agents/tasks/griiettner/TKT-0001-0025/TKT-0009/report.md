---
id: TKT-0009
namespace: griiettner
title: Establish the Rust workspace, MIT licensing, domain contracts, unified events, configuration, and single embedded Turso database schema
artifact: report
status: done
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
chain_role: worker
chain_parent: TKT-0008
areas:
  - .agents/tasks
  - .agents/skills
skills:
  - tkt
  - tkt-exec-chain
---

# TKT-0009 Report: Establish the Rust workspace, MIT licensing, domain contracts, unified events, configuration, and single embedded Turso database schema

## Summary

Worker 1 of the TKT-0008 chain. The repository root now holds a Cargo
workspace with five crates laid out per ADR-006: `gritt-core` (contracts, no
I/O), `gritt-provider` and `gritt-connector` (compilable skeletons),
`gritt-harness` (embedded Turso store with product-namespace migrations),
and the `gritt` binary (clap, layered config, keychain-first key resolution).
The root `LICENSE` is MIT. Every later worker builds on these seams.

## Chain Evidence

- Base branch: `feature/tkt-0008-gritt-cli` at `ab3e34a`.
- Worktree: `/Users/griiettner/Projects/grittflow/gritt-cli-tkt-0009`.
- Branch: `tkt-0009-01-contracts`.
- Commit and PR: recorded in the PM handoff and the PR description.

## Key Decisions

- `gritt-core` exposes async traits through `BoxFuture` and
  `futures_core::Stream` so the crate needs no runtime. `tokio` first appears
  in `gritt-harness`.
- The event envelope is `{session_id, sequence, source, timestamp, kind,
  diagnostic}` with `kind` as an internally tagged enum. Provider data only
  travels in `diagnostic`.
- Product tables are `gritt_` prefixed and tracked in
  `gritt_schema_migrations`. The store never creates, alters, or drops the
  `gritt-agent` memory tables. The coexistence test applies the memory
  schema from `.agents/cli/src/memory/schema.sql` first, then the product
  migrations, then re-applies the memory schema.
- Database location: `<workspace>/.agents/brain/data/agent-memory.db` when
  the workspace has `.agents/`, else `<user data dir>/gritt/gritt.db`, with
  an explicit override.
- Config layers are merged lowest first: environment, user file, project
  file, flags. Profile and alias maps merge by key; scalar sections replace.
  Any string field named `key`, `api_key`, `apikey`, `token`, or `secret`
  (or ending in `_<name>`) fails the load with `SecretInConfig`. A key
  reference is a table, so it passes.
- `Secret` prints `[redacted]` for Debug and Display and has no Serialize
  impl. `SecretRef` is the only persisted form.
- Keys resolve keychain first, environment second, through `Keychain` and
  `EnvSource` traits so tests use fakes. `gritt key-set <profile>` reads the
  key from stdin and writes only the keychain.

## Dependency Checks

Versions and licenses verified with `cargo info` on 2026-09-04:

| Crate | Version | License |
| --- | --- | --- |
| serde | 1.0.229 | MIT OR Apache-2.0 |
| serde_json | 1.0.151 | MIT OR Apache-2.0 |
| chrono | 0.4.45 | MIT OR Apache-2.0 |
| thiserror | 2.0.20 | MIT OR Apache-2.0 |
| futures-core | 0.3.34 | MIT OR Apache-2.0 |
| clap | 4.6.6 | MIT OR Apache-2.0 |
| toml | 1.1.5 | MIT OR Apache-2.0 |
| dirs | 6.0.0 | MIT OR Apache-2.0 |
| keyring | 4.2.0 | MIT OR Apache-2.0 |
| tokio | 1.53.1 | MIT |
| turso | 0.7.2 (pinned to match gritt-agent; 0.8.0-pre.8 is a prerelease) | MIT |
| tempfile (dev) | 3.27.0 | MIT OR Apache-2.0 |

## Assumptions

- Workspace `rust-version` is 1.88, not 1.85, because keyring 4.2.0 declares
  an MSRV of 1.88. A 1.85 claim would be false.
- keyring 4.2.0 has no `apple-native` feature; its default `v1` feature set
  covers macOS Keychain, Windows Credential Manager, and Secret Service.
- Any keychain error other than a missing entry is treated as "no keychain"
  so the environment fallback from ADR-008 applies.
- Session and event persistence over the store, telemetry writes, and policy
  evaluation are left to TKT-0011 as the task's out-of-scope section says.

## Validation

All run from the worktree root on 2026-09-04:

- `cargo fmt --all --check`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo test --workspace`: pass, 22 tests (12 core, 4 harness, 6 binary).
- `cargo build --release`: pass, 1m 34s cold.
- `cargo test --manifest-path .agents/cli/Cargo.toml`: pass, 69 tests; the
  memory command contract is unchanged.
- `gritt --version`, `gritt --help`, `gritt config`: run.
- `gritt-agent ticket validate` and `chain-check`: see PM handoff.

## Completion Gate

- Acceptance: yes. Workspace compiles, MIT license present, contracts cover
  events, sessions, tools, policy, providers, connectors, config, secrets,
  errors, telemetry, and embeddings without provider fields; a temp database
  migrates both namespaces; gritt-agent memory tests pass; secrets exist only
  as references.
- Scope: yes. No HTTP, SSE, terminal rendering, tool execution, or connector
  process code was added.
- Validation: yes, as listed above.
- Security and safety: no network code. Secret values are redacted and
  unserializable. Config with a literal key fails to load. Keychain writes
  only through `key-set` from stdin.
- Regression risk: low. The `.agents/cli` crate is excluded from the
  workspace and untouched. The store only adds `gritt_` tables.
- Follow-up: none blocking. See below.
- Assumptions: recorded above.

## Follow-up

- TKT-0011 implements `SessionStore` over `gritt_session_*` tables and
  telemetry writes.
- `gritt key-set` echoes stdin in an interactive terminal; the harness
  ticket should add a no-echo prompt.

## Updates

- None.
