---
id: TKT-0017
namespace: griiettner
title: Implement generic .mcp.json MCP runtime and harness tool dispatch
artifact: task
status: planning
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
chain_role: worker
chain_parent: TKT-0015
dependencies:
  - TKT-0016
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

# TKT-0017 Task: Implement generic .mcp.json MCP runtime and harness tool dispatch

## Chain Role

Worker 2 of 5 in the TKT-0015 chain.
Start from a freshly updated `main` only after TKT-0016 merges and passes review.

Branch: `tkt-0017-02-mcp`

## Goal

Implement the harness MCP runtime that reads every `.mcp.json` server entry,
negotiates supported transports, discovers tools, and dispatches approved
calls through Gritt's native tool/event contract.

## Scope

- Add generic stdio and Streamable HTTP configuration parsing, environment
  interpolation, trust and secret boundaries, lifecycle and timeout handling,
  paginated discovery, collision-safe tool registry, cancellation, reload, and
  safe server snapshots/events. Use fake servers and fixtures for protocol
  behavior. Keep I/O in harness/provider layers and expose neutral state to the
  later TUI worker.

## Out of Scope

- Do not build the final Ratatui home, composer, command palette, sidebar
  rendering, model/effort picker, or performance benchmark UI. Do not hard-code
  `gritt-local-memory`, `turso-local-memory`, or any other server name.

## Acceptance Criteria

- Every configured server has a state and reason, supported servers can
  initialize and list tools, duplicate tool names remain safely addressable,
  denied calls never reach a server, failed servers do not disable healthy
  ones, cancellation and shutdown do not leak child processes, and no secret
  enters logs/errors/events.

## Verification

- Run formatting, clippy, focused harness/provider tests, fake stdio and HTTP
  fixture tests, workspace tests for changed contracts, and chain-check. Run an
  opt-in smoke check against every available executable listed in the current
  `.mcp.json`, recording unavailable entries without mutating their data.
- Run `gritt-agent ticket chain-check --ticket TKT-0017 --base main` before semantic review.

## Handoff

Report branch name, PR link, validation output, and unresolved risks to the
PM, then stop. Do not start the next step.
