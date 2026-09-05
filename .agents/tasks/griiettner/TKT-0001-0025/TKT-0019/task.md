---
id: TKT-0019
namespace: griiettner
title: Integrate conversation sidebar, sessions, MCP status, and responsive runtime
artifact: task
status: planning
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
chain_role: worker
chain_parent: TKT-0015
dependencies:
  - TKT-0018
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

# TKT-0019 Task: Integrate conversation sidebar, sessions, MCP status, and responsive runtime

## Chain Role

Worker 4 of 5 in the TKT-0015 chain.
Start from a freshly updated `main` only after TKT-0018 merges and passes review.

Branch: `tkt-0019-04-tui-integration`

## Goal

Connect the TUI foundation to real sessions, provider/model/effort setup,
MCP lifecycle state, the Crush-inspired sidebar, and an event-driven runtime
that stays responsive while work is pending or streaming.

## Scope

- Integrate lazy setup and `/connect`, `/models`, `/effort`, `/mcp`, `/sidebar`,
  `/new`, and session commands with typed control-plane operations. Render live
  session, model, usage, cost, changed-file, and MCP status where known. Add
  async loading, late-result rejection, viewport scroll hold, sidebar collapse,
  session switching, cancellation, and connector-specific authority labels.

## Out of Scope

- Do not add new MCP protocol features, change provider request mapping, add
  LSP or skill execution, redesign the visual system, or write final user
  documentation. Do not claim cost/context data when the source is unknown.

## Acceptance Criteria

- A fresh TUI can connect to configured providers or agents, create/resume
  sessions, select model and effort, show every MCP server state, invoke
  approved native MCP tools, preserve composer/scroll state through dialogs,
  and keep connector authority separate. Late async work cannot overwrite the
  active session.

## Verification

- Run formatting, clippy, focused harness/gritt tests, TUI reducer and PTY
  tests, workspace tests, the fake MCP integration suite, and chain-check. Run
  the manual OpenCode/Crush reference walkthrough with a real terminal and
  record any platform-specific input limitation.
- Run `gritt-agent ticket chain-check --ticket TKT-0019 --base main` before semantic review.

## Handoff

Report branch name, PR link, validation output, and unresolved risks to the
PM, then stop. Do not start the next step.
