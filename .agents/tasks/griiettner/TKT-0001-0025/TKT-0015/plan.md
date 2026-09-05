---
id: TKT-0015
namespace: griiettner
title: Build an OpenCode-inspired full-screen agent TUI with generic MCP harness support
artifact: plan
status: planning
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
chain_role: orchestrator
areas:
  - crates/gritt-core
  - crates/gritt-provider
  - crates/gritt-harness
  - crates/gritt
  - docs
  - .agents/plans
skills:
  - tkt
  - tkt-exec-chain
  - dev-harness
  - dev-provider
  - codebase-design
  - tdd
  - write-plan
---

# TKT-0015 Plan: Build an OpenCode-inspired full-screen agent TUI with generic MCP harness support

## Sequence

1. TKT-0016 on `tkt-0016-01-contracts`. Freeze effort, session-draft,
   provider setup, and control-plane contracts needed by later workers.
2. TKT-0017 on `tkt-0017-02-mcp`. Implement generic `.mcp.json` loading,
   MCP lifecycle, discovery, permission-gated dispatch, and harness state.
3. TKT-0018 on `tkt-0018-03-tui-foundation`. Build the visual home,
   conversation shell, composer, command registry, and searchable pickers with
   deterministic fixture state.
4. TKT-0019 on `tkt-0019-04-tui-integration`. Connect the real control plane,
   sessions, MCP state, sidebar, asynchronous work, and responsive runtime.
5. TKT-0020 on `tkt-0020-05-hardening`. Complete docs, performance evidence,
   workspace MCP smoke checks, cross-feature hardening, and regression fixes.
6. TKT-0021 runs the final integrated review.

After each merge the reviewer runs the chain check, then a semantic pass.

## Decisions To Lock Before Execution

- Workers use fresh worktrees and branches from `main`, one at a time. Each
  worker opens a PR against `main`; the reviewer runs chain-check and semantic
  review before merge. The PM merges a passing PR before activating the next
  worker.
- Open implementation decisions are resolved by the worker within its scope
  and recorded in its report. No worker may change the provider-neutral event
  model, connector authority, or secret policy without recording the required
  ADR follow-up.
- Model switching across an existing conversation remains a product gap. The
  chain must preserve the prompt and explain when a new session is required;
  seamless history migration is not silently added.
