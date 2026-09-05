---
id: TKT-0015
namespace: griiettner
title: Build an OpenCode-inspired full-screen agent TUI with generic MCP harness support
artifact: task
status: planning
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
chain_role: orchestrator
chain_children:
  - TKT-0016
  - TKT-0017
  - TKT-0018
  - TKT-0019
  - TKT-0020
  - TKT-0021
dependencies:
  - TKT-0014
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

# TKT-0015 Task: Build an OpenCode-inspired full-screen agent TUI with generic MCP harness support

## Goal

Deliver a production-shaped full-screen Gritt agent workspace inspired by
OpenCode's home and command flow and Crush's sidebar and onboarding. It must
run native provider sessions and installed-agent sessions through the existing
control plane, read every MCP server from the workspace `.mcp.json`, and keep
the terminal responsive during streaming and MCP activity.

## Chain Execution Contract

- Execution mode: `tkt-exec-chain`
- Base branch: `main`
- Branch naming pattern: `tkt-{id}-{step}-{slug}` (`{id}` is the worker ticket number, `{step}` the two-digit step, `{slug}` the step slug)
- Merge policy: Each worker opens a PR against main; reviewer runs after every PR; do not wait for CI/CD before merge when quota is unreliable.
- Reviewer gate: reviewer runs after every worker PR
- Child tickets: required and fixed as TKT-0016 through TKT-0021
- Validation required on every worker step: `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, focused tests for
  the worker's crates, `cargo test --workspace` before the final hardening
  handoff, and `gritt-agent ticket chain-check` before semantic review.
- Benchmark requirements: run the deterministic TUI responsiveness fixture and
  the 10,000-message, 1,000-delta, MCP-load scenario from the feature plan;
  record p50/p95 input latency, render work, CPU, memory, queue bounds, and
  build/terminal details. A real-terminal walkthrough is also required.
- Final completion condition: all worker PRs are merged, the final reviewer
  returns `pass`, every `.mcp.json` entry is accounted for, native approved MCP
  tools work through policy-gated dispatch, the OpenCode/Crush-inspired flows
  pass focused visual and interaction checks, and existing workspace tests
  remain green.
- Concurrency: exactly one active worker; no later step starts before the previous PR merges

## Child Ticket Chain

1. [TKT-0016 Define model, effort, session-draft, and provider setup contracts](../TKT-0016/task.md)
2. [TKT-0017 Implement generic .mcp.json MCP runtime and harness tool dispatch](../TKT-0017/task.md)
3. [TKT-0018 Build the full-screen home, composer, commands, and picker UI](../TKT-0018/task.md)
4. [TKT-0019 Integrate conversation sidebar, sessions, MCP status, and responsive runtime](../TKT-0019/task.md)
5. [TKT-0020 Complete documentation, performance benchmarks, and integrated TUI hardening](../TKT-0020/task.md)
6. [TKT-0021 Review integrated OpenCode-inspired agent TUI and MCP harness chain](../TKT-0021/task.md) (final reviewer)

The orchestrator activates exactly one worker ticket at a time. Every
worker opens one PR and receives a reviewer verdict before merge. The next
worker is activated only after that merge.

## Inputs

- Read `.agents/plans/agent-tui.md`, `AGENTS.md`, ADR-006 through ADR-011,
  `docs/terminal-modes.md`, `docs/providers.md`, `docs/tools-and-permissions.md`,
  `crates/gritt-harness/Cargo.toml`, and the relevant crate README or tests.
  MCP workers must also read the linked MCP lifecycle and transport
  specifications in the feature plan.

## Scope

- The chain covers provider/model/effort setup contracts, generic MCP config and
  tool execution, Ratatui home and conversation UI, slash commands, searchable
  pickers, sidebar state, session integration, responsiveness, docs, and
  regression hardening.

## Out of Scope

- Do not add a desktop frontend, remote workspace service, child-agent
  orchestration, LSP runtime, skill execution engine, or MCP sampling,
  elicitation, prompt/resource browsing. Do not copy upstream source or add
  dependencies without review. Do not weaken native permissions, expose keys,
  re-run connector tools, or silently skip configured MCP entries.

## Acceptance Criteria

- The TUI creates and resumes sessions with explicit provider, model, and
  effort state; `/connect`, `/models`, `/effort`, `/mcp`, `/sidebar`, and
  session commands work through shared reducers; all configured MCP servers
  have visible lifecycle state; approved native MCP tools execute through the
  policy engine; the sidebar reports known session and workspace state; and
  responsiveness evidence meets or explains the plan's budgets.

## Verification

- Run the workspace format, clippy, and test commands, focused contract/MCP/TUI
  tests, ticket validation, chain-check against `main`, the required benchmark,
  and the available real-terminal walkthrough. Preserve honest skips for
  unavailable provider keys, connector binaries, or MCP executables.
- Run `gritt-agent ticket chain-check --ticket TKT-0015 --base main` before semantic review.
