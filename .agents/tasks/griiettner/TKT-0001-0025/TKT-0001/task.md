---
id: TKT-0001
namespace: griiettner
title: Build project-local agent CLI
artifact: task
status: done
owner: griiettner
created: 2026-09-03
updated: 2026-09-03
---

# TKT-0001 task: Build project-local agent CLI

## Goal

Build a small project-local Rust CLI that replaces the essential Node-based
local-memory, ticket, and skill-maintenance commands.

## Inputs

- `.agents/brain/README.md`
- `.agents/memory/decisions/ADR-001-agent-ticket-lifecycle.md`
- `.agents/memory/decisions/ADR-002-memory-routing.md`
- `.agents/memory/decisions/ADR-003-ticket-id-and-chunking.md`
- `.agents/tools/agent-memory/`
- `.agents/tools/agent-tools/`
- `.agents/skills/tkt/`
- `.agents/skills/skill-management/`

## Scope

- Add the independent `.agents/cli/` Rust crate and `gritt-agent` binary.
- Implement the command surface defined in `plan.md`.
- Preserve current database location, ticket artifacts, namespaces, chunking, and generated metadata formats.
- Add unit and integration tests using temporary repositories and deterministic fixtures.
- Update `.mcp.json`, agent instructions, and tool documentation to use the Rust CLI.
- Remove Node implementations only after their replacement commands pass parity tests.

## Out of scope

- The Gritt product runtime and its future Cargo workspace.
- A memory dashboard.
- Embeddings, reranking, remote AI gateways, or cloud synchronization.
- Ticket chains, migration helpers, trust configuration, commit automation, or skill creation.
- Changes to ticket lifecycle, frontmatter, chunking, or model routing.

## Acceptance criteria

- A clean checkout can build the CLI with the Rust toolchain only.
- `memory index` incrementally indexes supported project documents and removes deleted entries.
- `memory search` returns ranked results with path and line citations.
- `memory serve` provides working `search_local_memory` and `read_local_memory` MCP tools.
- Ticket creation preserves contiguous namespace allocation and rolls back on sync failure.
- Ticket sync and validation reproduce the current generated indexes and validation rules.
- Skill sync reproduces generated Claude Code stubs and Codex policy metadata.
- No command writes outside the repository or stores secrets in the memory database.
- Replaced Node scripts and undeclared Node dependency instructions are removed.

## Verification

- `cargo fmt --manifest-path .agents/cli/Cargo.toml --all --check`
- `cargo clippy --manifest-path .agents/cli/Cargo.toml --all-targets -- -D warnings`
- `cargo test --manifest-path .agents/cli/Cargo.toml`
- Build and run every documented command against a temporary fixture repository.
- Compare generated ticket indexes, validation results, skill stubs, and memory search citations with committed fixtures.
- Start the MCP server and call both tools through an MCP client test.
