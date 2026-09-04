---
id: TKT-0011
namespace: griiettner
title: Implement sessions, planning and coding phases, permissions, workspace-bounded tools, terminal modes, approvals, cancellation, and telemetry
artifact: task
status: planning
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
chain_role: worker
chain_parent: TKT-0008
dependencies:
  - TKT-0010
areas:
  - .agents/tasks
  - .agents/skills
skills:
  - tkt
  - tkt-exec-chain
---

# TKT-0011 Task: Implement sessions, planning and coding phases, permissions, workspace-bounded tools, terminal modes, approvals, cancellation, and telemetry

## Chain Role

Worker 3 of 5 in the TKT-0008 chain.
Start from a fresh worktree branched from the latest merged `feature/tkt-0008-gritt-cli` only after TKT-0010 merges and passes review.

Branch: `tkt-0011-03-harness`

## Goal

Build the usable native coding harness: named sessions, planning and coding phases, safe tools, terminal modes, approvals, cancellation, and local content-safe telemetry.

## Scope

- Implement session persistence and resume over the shared database contracts.
- Implement planning and coding state transitions and the print, REPL, and Ratatui full-screen modes.
- Implement allow/ask/deny policy evaluation, workspace-bounded file and shell tools, child-process tracking, approvals, diff review, and cancellation.
- Record local telemetry and analytics without prompt, source, secret, or transcript content.

## Out of Scope

- Do not implement provider wire parsing, external connector launchers, cross-platform packaging, or cloud services. Provider contracts come from TKT-0010; connectors and release work follow.

## Acceptance Criteria

- A user can plan, approve, execute, cancel, and resume a native tool-using session in print and REPL modes.
- Full-screen mode renders streamed events, approvals, tool activity, status, multiline input, and diff review using Ratatui 0.30.2 and Crossterm 0.29.
- File and shell tools cannot escape the workspace, and every execution passes policy evaluation.
- Telemetry and analytics are local, content-safe, and persisted in their own database namespace.

## Verification

- `cargo fmt --all --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test -p gritt-harness` and the full workspace tests
- Run native sessions against provider fixtures, including approval, cancellation, resume, and workspace-boundary cases.
- `gritt-agent ticket chain-check --ticket TKT-0011 --base feature/tkt-0008-gritt-cli`
- Run `gritt-agent ticket chain-check --ticket TKT-0011 --base main` before semantic review.

## Handoff

Report branch name, PR link, validation output, and unresolved risks to the
PM, then stop. Do not start the next step.
