---
id: TKT-0008
namespace: griiettner
title: Build the complete Gritt local AI coding agent CLI
artifact: plan
status: planning
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
chain_role: orchestrator
areas:
  - .agents/tasks
  - .agents/skills
skills:
  - tkt
  - tkt-exec-chain
---

# TKT-0008 Plan: Build the complete Gritt local AI coding agent CLI

## Sequence

1. TKT-0009 on `tkt-0009-01-contracts`. Establish the workspace, MIT license, domain contracts, unified events, layered configuration, and one embedded Turso/libSQL schema with namespaced memory, session, telemetry, and analytics tables.
2. TKT-0010 on `tkt-0010-02-providers`. Implement provider adapters, streaming normalizers, daily model caching, capability checks, and opt-in embedding and reranking configuration.
3. TKT-0011 on `tkt-0011-03-harness`. Implement sessions, planning and coding phases, permissions, native tools, terminal modes, approvals, cancellation, and local telemetry.
4. TKT-0012 on `tkt-0012-04-connectors`. Implement native and external connector supervision, PTY fallback, normalized events, and live Codex and Claude Code coverage.
5. TKT-0013 on `tkt-0013-05-release`. Complete reproducible cross-platform builds, diagnostics, documentation, end-to-end tests, and integrated hardening.
6. TKT-0014 runs the final integrated review.

After each merge the reviewer runs the chain check, then a semantic pass.

## Decisions To Lock Before Execution

- The chain delivers all five implementation phases, but no non-terminal frontend.
- Base branch is `main`. The chain uses one feature branch and worktree per worker, opens and merges each worker PR into the chain feature branch, then opens one final master PR from that branch to `main`.
- The chain feature branch is `feature/tkt-0008-gritt-cli`, created once from `main`. Worker branches and worktrees are created from its latest merged state.
- The project license is MIT. Release verification requires reproducible builds and checksums, not signed installers.
- One embedded local Turso/libSQL database is shared by `gritt-agent` and Gritt. Product tables are namespaced and migrated independently from memory tables.
- Telemetry and analytics are enabled only as local, content-safe records. No Gritt Cloud or Turso Cloud is used. Embedding and reranking are opt-in through environment variables, matching the prior Node behavior.
- Deprecated model aliases automatically remap using provider-declared replacements or an explicit configured alias map. If neither exists, Gritt refuses the alias with an actionable error.
- Live connector tests run when Codex or Claude Code is installed and authenticated; deterministic fixtures cover unavailable environments.
