---
id: TKT-0018
namespace: griiettner
title: Build the full-screen home, composer, commands, and picker UI
artifact: task
status: done
owner: griiettner
created: 2026-09-04
updated: 2026-09-05
chain_role: worker
chain_parent: TKT-0015
dependencies:
  - TKT-0017
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

# TKT-0018 Task: Build the full-screen home, composer, commands, and picker UI

## Chain Role

Worker 3 of 5 in the TKT-0015 chain.
Start from a freshly updated `main` only after TKT-0017 merges and passes review.

Branch: `tkt-0018-03-tui-foundation`

## Goal

Build the reviewable Ratatui foundation: OpenCode-inspired home and
conversation composition, fast composer, slash-command registry, searchable
pickers, and deterministic rendering state that later workers can connect to
the real control plane.

## Scope

- Implement Home, Conversation, picker, command suggestions, multiline input,
  focus movement, overlay priority, theme tokens, narrow-terminal behavior,
  and tool/approval presentation using fixture state and TestBackend snapshots.
  Include the layout needed for the later right sidebar without implementing
  live MCP or Git state.

## Out of Scope

- Do not change provider wire formats, session persistence, MCP process
  lifecycle, keychain/config writes, live control-plane opening, or benchmark
  harnesses. Do not make slash commands send prompts.

## Acceptance Criteria

- Fixtures render a spacious home, a readable transcript, compact tool rows,
  command search, provider/model/effort pickers, and approval/diff views at
  120x40, 80x24, and 60x20. Keyboard tests prove slash commands, paste,
  cancellation, overlay precedence, Unicode cursor movement, and scroll hold.

## Verification

- Run formatting, clippy, `cargo test -p gritt-harness`, TUI reducer and
  TestBackend tests, and the available TUI PTY tests. Perform a real-terminal
  walkthrough at wide and narrow sizes and run chain-check before review.
- Run `gritt-agent ticket chain-check --ticket TKT-0018 --base main` before semantic review.

## Handoff

Report branch name, PR link, validation output, and unresolved risks to the
PM, then stop. Do not start the next step.
