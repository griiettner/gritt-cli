---
id: TKT-0004
namespace: griiettner
title: Close brain doc gaps and evaluate a Turso-backed local memory store
artifact: plan
status: done
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
---

# TKT-0004 Plan: Close brain doc gaps and evaluate a Turso-backed local memory store

## Sequence

1. Fix the four documentation gaps. Decision-complete; no dependency on the
   items below. Safe to execute first and independently.
2. Lock every item under "Decisions to lock" below, either by the user
   answering directly or by a `/tkt-plan` pass once partial answers exist.
3. Execute the storage-engine swap against the locked decisions, in phases
   that keep local-only behavior provable before any network-facing feature
   is added.
4. Update `.agents/brain/*`, the relevant ADR, and `.agents/cli/README.md` to
   describe the shipped result, not an aspiration.
5. Verify against this ticket's acceptance criteria and close.

## Decisions to lock before execution (storage-engine swap)

All six decisions are locked. The user chose a fully local implementation on
2026-09-04. The remaining answers follow from that boundary and the dependency
and compatibility checks recorded below.

### 1. Local-only file swap, or Turso Cloud sync?

Locked. Local-only engine swap. There is no Cloud sync, remote endpoint,
account, credential, or network fallback.

This is the decision that determines the shape of everything else. The user
said "Turso is agentic DB and we should use it," which reads as wanting the
sync/shared-memory capability, not merely a different SQLite-compatible file
on disk. But adopting cloud sync means:

- every developer needs a Turso account and a database URL/token to get the
  memory feature at all, which contradicts ADR-004's "nothing else installed"
  goal and `capabilities.md`'s "Always available" listing for local search;
- the CLI would make network requests by default, contradicting
  `.agents/brain/README.md`'s "The only mode is local" and "the CLI reads no
  API keys" statements;
- a shared database raises the question of who owns it, how access is
  granted, and what happens when a developer works offline.

**No default is recommended here.** State explicitly which of these you
want:

- **(a) Local-only engine swap.** Replace `rusqlite` with a Turso-compatible
  local file (the `turso` crate's `Builder::new_local` mode), no account, no
  network, functionally a drop-in replacement. This keeps every existing
  guarantee true and is the smallest reversible change, but it does not
  deliver "agentic" shared or synced memory, only a different SQLite fork
  underneath the same local-only design.
- **(b) Turso Cloud sync.** Add `push()`/`pull()` (or `libsql` remote/
  embedded-replica mode) against a real Turso Cloud database, gated behind
  credentials, with local-only FTS5 search as the fallback when the network
  or the account is unavailable, per the existing Fallback rule in
  `capabilities.md`. This is a genuine architecture change and needs an ADR,
  not just a code change.

### 2. Crate choice

Locked. `turso` 0.7.2 with its default local FTS support and without the
optional `sync` feature. Turso's official Rust documentation recommends this
crate for local databases. The crate and upstream repository are MIT licensed.
The project is actively maintained, but its own README recommends caution for
mission-critical data. This index is reproducible generated state.

Checked 2026-09-04 via Turso's own documentation and a web search (not from
model memory, per `dev/cli`'s dependency policy):

- `turso` — Turso's currently recommended crate for new projects. Supports a
  purely local mode with `Builder::new_local`, and an opt-in `sync` feature
  with `push()`/`pull()` against Turso Cloud. A local sync server
  (`tursodb :memory: --sync-server ...`) can be used to test sync without a
  Turso Cloud account.
- `libsql` — the older, more established SDK. Supports local, `remote`
  (direct HTTP to Turso Cloud), and embedded-replica modes.

Not yet confirmed and required before adding either as a dependency, per
`dev/cli`'s "Adding a dependency" step 1: current version, license,
maintenance status, and — critical for this repository — whether either
crate's local mode still exposes SQLite FTS5 virtual tables the way
`rusqlite` does, since `document_chunks_fts` and `documents_fts` in
`.agents/cli/src/memory/schema.sql` are the entire retrieval mechanism. If
neither supports FTS5, this plan needs a different full-text strategy before
proceeding, which would make this a materially bigger change than a
dependency swap.

**Recommendation once (1) is answered:** whichever crate is chosen, verify
FTS5 (or an equivalent) works against a throwaway local database before
touching `.agents/cli/src/memory/` for real.

### 3. Credentials, if cloud sync is chosen

Locked. Not applicable. The local implementation reads and stores no Turso
URL or token.

If the answer to (1) is (b): `TURSO_DATABASE_URL` and `TURSO_AUTH_TOKEN`
follow the same rule `dev/cli` already states for provider keys — resolved
from the OS keychain or an environment variable the config only names, never
a literal value in a config file, fixture, log, or error message. Decide
whether the profile lives in `.agents/settings.json`,
`.agents/state/identity.local.yaml`'s sibling, or a new file, and whether
`ticket identity`'s namespace-resolution pattern (flag, env var, local state
file, external lookup) is the right shape to reuse here.

### 4. Shared vs. per-developer database

Locked. One independent local generated database per checkout, matching
the existing behavior.

If cloud sync is chosen: is there one shared Turso Cloud database the whole
team's agents read and write (real shared agent memory across machines), or
does each developer keep an independent database that merely happens to sync
to their own cloud copy (closer to today's per-developer local file, just
backed up)? This changes whether a database identifier is committed
repo-config or stays developer-local state, and whether write conflicts
between concurrent agents need handling.

### 5. FTS5 / schema parity

Locked. Use Turso's Tantivy-backed FTS indexes. Turso 0.7.2 does not
support SQLite FTS5 virtual tables. The schema changes to `CREATE INDEX ...
USING fts`, and search changes to `fts_match` and `fts_score`. Query
normalization, all-terms matching, result limits, citations, and the public
CLI and MCP shapes stay unchanged. Embeddings remain disabled.

Confirm the chosen crate's SQL surface still supports
`.agents/cli/src/memory/schema.sql`'s FTS5 virtual tables unmodified, or
determine the smallest schema change that preserves the same query shape
`search.rs` relies on. Out of scope regardless: turning on the reserved
`F32_BLOB(1536)` embedding columns. `providers.md` already gates that behind
`AGENT_EMBEDDING_PROVIDER`, off by default, and it is a separate capability
phase even if the new engine makes vector search newly practical.

### 6. ADR treatment

Locked. Add ADR-005. ADR-004 remains as the historical record of the Rust
CLI consolidation; ADR-005 records the local Turso engine and preserves the
offline and no-credential guarantees.

Recommendation: write a new ADR rather than editing ADR-004 in place. ADR-004
records why the CLI became a single local Rust binary with no runtime
dependency; that reasoning stays true regardless of the storage engine
underneath one of its subsystems. A new ADR should record specifically:
which option from (1) was chosen, the crate from (2), and the exact new
network/credential behavior, so `.agents/brain/README.md`'s security section
can be corrected to match instead of silently becoming false.

## Verification the executed change must pass

- `.agents/cli/tests/memory.rs` and `mcp.rs` (or their replacements) pass
  with no live Turso account or network access, so CI does not gain a new
  external dependency.
- `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, and
  `cargo test`, run with `--manifest-path .agents/cli/Cargo.toml`.
- `gritt-agent memory index`, `memory search`, and `memory serve` behave
  identically from the outside: same CLI output shape, same MCP tool
  contract (`search_local_memory`, `read_local_memory`).
- `gritt-agent ticket validate` and `ticket sync --check` on this repository.
</content>
