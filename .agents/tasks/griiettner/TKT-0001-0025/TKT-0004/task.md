---
id: TKT-0004
namespace: griiettner
title: Close brain doc gaps and evaluate a Turso-backed local memory store
artifact: task
status: done
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
---

# TKT-0004 Task: Close brain doc gaps and evaluate a Turso-backed local memory store

## Goal

Two tracks. Track A: close four documentation gaps in the agent brain and
skill set; decision-complete, executable now. Track B: replace the local
memory store's backing engine, bundled SQLite via `rusqlite`, with a
Turso-based store, executed only once `plan.md`'s open decisions are locked,
since the current committed architecture (ADR-004, `capabilities.md`,
`.agents/brain/README.md`) explicitly guarantees no network requests and no
runtime dependency, and Track B may change that guarantee.

The storage decisions are locked in `plan.md`. Track A is complete and Track
B is in progress.

## Inputs

- `.agents/tasks/griiettner/TKT-0001-0025/TKT-0004/plan.md` for the specific
  open decisions Track B must resolve before code changes start.
- `.agents/brain/architecture.md`, `services.md`, `providers.md`, and
  `README.md` for the current (partly stale) documentation.
- `.agents/memory/MEMORY.md` for the dangling `operations/index.yaml` route.
- `.agents/skills/commit/SKILL.md` for the `Co-Authored-By` conflict, and
  `git log` on this repository for the actual convention in use.
- `.agents/memory/decisions/ADR-002-memory-routing.md` and
  `ADR-004-project-local-agent-cli.md` for the architecture Track B must
  reconcile with or explicitly supersede.
- `.agents/cli/src/memory/` (`db.rs`, `schema.sql`, `index.rs`, `search.rs`,
  `mcp.rs`) and `.agents/cli/Cargo.toml` for the implementation Track B
  replaces.
- `dev/cli` skill for the dependency-vetting and key/credential policy Track
  B's crate choice and credential storage must follow.

## Scope

1. Fix `.agents/brain/architecture.md` and `services.md`: remove the `turso`
   tag and the "Turso/libSQL file database" description until Track B
   actually ships a Turso-backed engine; describe the current SQLite
   implementation accurately in the meantime, or the shipped Turso
   implementation once Track B lands.
2. Fix `.agents/brain/providers.md`'s "This mode uses local libSQL and FTS5
   only" line the same way.
3. Fix `.agents/memory/MEMORY.md`: either create
   `.agents/memory/operations/` with real content and let `ticket sync`
   generate its index, or remove the dangling `operations` row from the
   router until something is filed there.
4. Resolve the `Co-Authored-By` conflict in `.agents/skills/commit/SKILL.md`.
   State explicitly which convention applies to `/commit` (a user-invoked
   quick commit) versus an agent committing directly as part of a larger
   task, matching what this repository's own commit history actually shows,
   and update the skill text so it no longer contradicts observed practice.
5. Once plan.md's decisions are locked: replace the memory store's backing
   engine per the locked answers, touching `.agents/cli/Cargo.toml` (drop
   `rusqlite`, add the chosen crate, with its license and version recorded
   in `report.md`), `.agents/cli/src/memory/{db,schema.sql,index,search,mcp}.rs`,
   and every doc under `.agents/brain/` plus the applicable ADR.
6. Keep `memory index`, `memory search`, and `memory serve` (the
   `gritt-local-memory` MCP tools `search_local_memory` and
   `read_local_memory`) working with the same CLI output shape and MCP
   contract, so no skill, `.mcp.json` entry, or downstream caller needs to
   change merely because the backing engine changed.

## Out of Scope

- Turning on embeddings, reranking, or any `AGENT_*_PROVIDER` capability.
  `providers.md`'s off-by-default contract stays as is; a later ticket
  decides whether Turso's vector features are worth activating it.
- Any change to `ticket`, `skill`, `codex trust`, or `migrate cursor`
  subcommands. This ticket only touches the memory/brain subsystem and its
  documentation.
- Choosing a specific embedding or reranking model.
- Executing Track B under an assumed answer to plan.md's question 1 (local
  file swap vs. Turso Cloud sync). If that answer is not yet given, Track B
  stays unexecuted and this ticket's status stays `planning`.

## Acceptance Criteria

- The three brain docs and `MEMORY.md` describe the shipped implementation
  accurately, with no dangling route and no description of a system that
  is not running.
- `.agents/skills/commit/SKILL.md` no longer contradicts this repository's
  actual commit practice; the stated rule and `git log` agree.
- `plan.md`'s six "Decisions to lock" items each have a specific, named
  answer, not "TBD" or an assumed default the user never confirmed.
- If and only if plan.md is fully locked: `gritt-agent memory index`,
  `memory search`, and `memory serve` work against the new backing store
  with output equivalent to today's SQLite implementation, proven by
  `.agents/cli/tests/memory.rs` and `mcp.rs` (adapted as needed) passing
  with no live Turso account or network access in CI.
- A new or amended ADR records the storage-engine decision, and
  `.agents/brain/README.md`'s Security and privacy section is corrected to
  match whatever guarantee is actually true after the change.
- `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, and
  `cargo test`, run with `--manifest-path .agents/cli/Cargo.toml`, all pass
  without a live Turso account.
- `gritt-agent ticket validate` and `ticket sync --check` are clean.

## Verification

- `gritt-agent ticket validate` and `ticket sync --check` on this repository
  after Track A.
- A read-through confirming no doc under `.agents/brain/` describes the
  deleted Node implementation or a capability that is not actually enabled.
- The `dev/cli` verify set, per Acceptance Criteria above, after Track B.
- `/code-review high` (or the project's `review/ticket` skill) before
  closing, given the architectural weight of Track B: a genuine change to a
  committed no-network guarantee is exactly the kind of change that review
  should catch if it is incomplete or inconsistently applied.
</content>
