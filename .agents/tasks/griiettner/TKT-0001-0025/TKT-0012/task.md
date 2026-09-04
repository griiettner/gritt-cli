---
id: TKT-0012
namespace: griiettner
title: Implement supervised native and external connectors with PTY fallback, live Codex and Claude Code tests, and normalized events
artifact: task
status: planning
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
chain_role: worker
chain_parent: TKT-0008
dependencies:
  - TKT-0011
areas:
  - .agents/tasks
  - .agents/skills
skills:
  - tkt
  - tkt-exec-chain
---

# TKT-0012 Task: Implement supervised native and external connectors with PTY fallback, live Codex and Claude Code tests, and normalized events

## Chain Role

Worker 4 of 5 in the TKT-0008 chain.
Start from a fresh worktree branched from the latest merged `feature/tkt-0008-gritt-cli` only after TKT-0011 merges and passes review.

Branch: `tkt-0012-04-connectors`

## Goal

Add connector supervision that makes native and installed agents usable through the same session interface while preserving each external agent's command and tool authority.

## Scope

- Implement the native connector plus Codex, Claude Code, Cursor, and OpenCode connector contracts.
- Prefer documented machine-readable interfaces, use PTY as fallback, and terminal scraping only as a last resort.
- Add process supervision, health checks, timeouts, cancellation, approval relay, auth and capability reporting, and normalized events.
- Add live tests for installed and authenticated Codex and Claude Code CLIs, with deterministic fixtures for unavailable environments.

## Out of Scope

- Do not change provider adapter parsing, native tool policy semantics, release packaging, or add a desktop frontend. Those remain in earlier or later steps.

## Acceptance Criteria

- Native, Codex, Claude Code, Cursor, and OpenCode connectors satisfy the normalized contract and preserve external authority.
- Process exit, cancellation, timeout, approval, missing executable, and malformed output paths are surfaced without breaking native sessions.
- Live Codex and Claude Code smoke tests pass when available; fixture coverage proves the same behavior without installed CLIs.
- Connector sessions are stored and displayed alongside native sessions.

## Verification

- `cargo fmt --all --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test -p gritt-connector` and the full workspace tests
- Run live Codex and Claude Code smoke tests when both CLIs and authentication are available; otherwise run the committed fixtures and record the reason.
- `gritt-agent ticket chain-check --ticket TKT-0012 --base feature/tkt-0008-gritt-cli`
- Run `gritt-agent ticket chain-check --ticket TKT-0012 --base main` before semantic review.

## Handoff

Report branch name, PR link, validation output, and unresolved risks to the
PM, then stop. Do not start the next step.
