---
purpose: Boot router for agents working on Gritt.
---

# Gritt

Gritt is a local Rust application for running native AI agent sessions and
supervising installed agent CLIs through one interface.

## Start here

1. Query `gritt-local-memory` before searching files or source code.
2. Read only the files identified by the memory results.
3. If local memory is unavailable, use `.agents/memory/MEMORY.md` and its indexes.
4. Read the relevant ticket folder for ticket-specific work.
5. Load the smallest applicable skill before editing.
6. Read `.agents/MODELS.md` before delegating work.

`.agents/plans/plan1.md` is a working proposal. It is not canonical until its
decisions are accepted in ADRs or ticket artifacts.

## Context

- `.agents/memory/` holds durable architecture, principles, and accepted decisions.
- `.agents/tasks/` holds canonical ticket history.
- `.agents/skills/` holds reusable procedures.
- `.agents/brain/` documents local memory and holds its generated database.
- `.agents/cli/` is the `gritt-agent` Rust CLI for memory, ticket, skill, Codex trust, and migration maintenance.
- `.agents/plans/` holds proposals and planning inputs.

Indexes route to canonical files. Generated indexes and `.claude/skills/`
stubs must not be edited directly. Regenerate them with `gritt-agent`, built
once per checkout with `cargo build --release --manifest-path .agents/cli/Cargo.toml`.

## Rules

- Keep provider-specific behavior behind provider adapters.
- Keep native and connector sessions on one event model.
- Keep secrets out of config, logs, errors, fixtures, and transcripts.
- Use Rust for product code and for repository tooling.
- Record durable architecture decisions as ADRs before treating them as rules.
