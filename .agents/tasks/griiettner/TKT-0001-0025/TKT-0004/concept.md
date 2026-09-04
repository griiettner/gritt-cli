---
id: TKT-0004
namespace: griiettner
title: Close brain doc gaps and evaluate a Turso-backed local memory store
artifact: concept
status: concept
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
---

# TKT-0004 Concept: Close brain doc gaps and evaluate a Turso-backed local memory store

## Problem

A diagnostic pass over the agent brain and skill wiring found two separate
things.

First, four small documentation gaps, all decision-complete:

- `.agents/brain/architecture.md` and `.agents/brain/services.md` still carry
  a `turso` tag and describe a "Turso/libSQL file database", left over from
  the deleted Node implementation. The current Rust implementation uses
  bundled SQLite through `rusqlite` and makes no network requests.
- `.agents/brain/providers.md` says "This mode uses local libSQL and FTS5
  only", which is imprecise for the same reason.
- `.agents/memory/MEMORY.md` routes to
  `.agents/memory/operations/index.yaml`, a file and category that do not
  exist. Only `architecture/`, `decisions/`, and `principles/` are present.
- `.agents/skills/commit/SKILL.md` states "NEVER add a Co-Authored-By line
  or any AI/Warp, Cursor, Codex, Claude attribution", but every commit this
  agent has made directly in this repository (`cfd4b92`, `e2bd3c9`,
  `8126b5a`) carries one, per this harness's own convention. The two
  instructions contradict each other and nothing in the repo says which
  wins for which invocation path.

Second, the user wants the local memory store's actual backing engine
replaced: bundled SQLite (`rusqlite`) out, Turso in, on the stated reasoning
that "Turso is agentic DB and we should use it."

This second item is not decision-complete. The committed architecture
explicitly commits to the opposite trade-off today:

- ADR-004 (`.agents/memory/decisions/ADR-004-project-local-agent-cli.md`)
  gives "no runtime dependency at install time" and "one toolchain, buildable
  with nothing else installed" as reasons the CLI is a single Rust binary.
- `.agents/brain/README.md`'s Security and privacy section states "The only
  mode is local" and "the CLI reads no API keys and stores none in the
  database."
- `.agents/brain/capabilities.md` lists local FTS5 search as "Always
  available" and gates any provider that leaves the machine behind an
  explicit off-by-default environment variable.

Turso's current Rust story (checked 2026-09-04) offers a `turso` crate that
can run purely local with no account, and separately supports pushing and
pulling against a hosted Turso Cloud database once a URL and auth token are
configured. Whether the user wants the local-only engine swap (a different
SQLite-compatible file format, same guarantees) or the cloud-sync
capability (a genuine behavior and trust-boundary change) is not yet
answered, and this ticket must not guess.

## Intent

Close the four documentation gaps now; they need no new decision. Separately,
produce a decision-complete plan for the storage-engine swap that names the
crate, the sync mode, the credential mechanism, and whether the "no network
requests" guarantee is being changed or preserved, then execute it once those
answers are confirmed.

## Unknowns

- Local-only Turso/libSQL file vs. actual Turso Cloud sync. This is the
  central open question and changes almost everything else below.
- Which crate: `turso` (Turso's own current recommendation, local-first,
  optional `push()`/`pull()` sync) or `libsql` (older, more mature, `remote`
  and embedded-replica features). Neither's FTS5 support, license, or
  current maintenance state was confirmed past a web search; per `dev/cli`
  policy this must be verified against crates.io/docs.rs before adding the
  dependency, not assumed from this concept.
- Whether the existing `documents_fts` / `document_chunks_fts` SQLite FTS5
  virtual tables carry over as-is, need a different full-text mechanism, or
  need a schema migration.
- Credential storage if cloud sync is wanted: `TURSO_DATABASE_URL` and
  `TURSO_AUTH_TOKEN` must follow the same keychain-or-environment-variable
  rule the CLI already applies to provider keys; never a config file value.
- Whether this becomes a shared team database (behavior change: memory is no
  longer per-developer and per-machine) or stays one independent local file
  per developer, just on a different SQLite fork.
- Whether an existing ADR (ADR-004) is amended or a new one records the
  decision, so the "no network requests" claim in three committed docs stays
  accurate either way.

## Success Criteria

- The four documentation gaps are fixed and `ticket validate` /
  `ticket sync --check` stay clean.
- `plan.md` answers every unknown above with a specific choice, not "TBD",
  before any code changes to `.agents/cli/src/memory/`.
- If the user has not yet chosen local-only vs. cloud sync, the plan says so
  explicitly and the ticket stays at `planning` rather than being executed
  under an assumed answer.
- `memory index`, `memory search`, and `memory serve` keep their current
  behavior and output shape once the swap lands, verified without needing a
  live Turso account in CI.
- The network-and-credentials guarantee in `.agents/brain/README.md`,
  `capabilities.md`, and ADR-004 is either still true or is explicitly
  superseded by a new or amended ADR; it is never left silently wrong.
</content>
