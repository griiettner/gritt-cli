---
id: TKT-0023
namespace: griiettner
title: Expose reusable control plane API for T3Code
artifact: task
status: ready
owner: griiettner
created: 2026-09-06
updated: 2026-09-06
areas:
  - core
  - harness
  - cli
skills:
  - codebase-design
  - dev-harness
  - dev-cli
---

# TKT-0023 Task: Expose reusable control plane API for T3Code

## Goal

Expose a stable, reusable Rust control-plane seam that the existing terminal
clients and a future T3Code frontend can share for provider, session, mode,
effort, permission, configuration, and event behavior.

## Inputs

- ADR-006 product crate boundaries.
- ADR-011 release and frontend boundary.
- Existing `ControlPlane`, `AgentBuilder`, `Driver`, setup traits, and event
  contracts.
- Existing provider failover and last-used preference behavior.

## Scope

- Inventory and document the public control-plane operations needed by a Rust
  non-terminal client.
- Extract or wrap provider/profile/model resolution, session lifecycle,
  execution-mode and effort selection, permission decisions, last-used
  preferences, and normalized event streams behind reusable Rust APIs.
- Keep CLI argument parsing, REPL input, TUI rendering, keyboard handling, and
  terminal setup outside the shared seam.
- Keep configuration and keychain I/O injected through platform-facing traits.
- Add a minimal non-terminal client fixture or integration test that exercises
  the same API as the terminal clients.

## Out of Scope

- Building the T3Code UI itself.
- Adding a local socket or network service before a separate-process requirement
  exists.
- Replacing normalized events with terminal text or requiring ANSI parsing.
- Duplicating provider adapters, policies, tools, or session storage in T3Code.
- Changing connector-owned permissions or external agent behavior.

## Acceptance Criteria

- A non-terminal Rust fixture can create or resume a native session, select a
  profile/model/mode/effort, submit a prompt, and consume normalized events.
- The fixture uses the same provider resolution, fallback, policy, and
  preference services as the CLI and TUI.
- No shared API module depends on Ratatui, Crossterm, terminal dimensions, or
  terminal escape sequences.
- Shared errors identify typed causes and preserve secret redaction.
- Existing CLI, REPL, and TUI behavior remains covered by their current tests.
- API boundaries and ownership are documented for the future T3Code client.

## Verification

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `cargo test --workspace --locked`
- `cargo build --release --locked`
- A dedicated non-terminal integration test exercises the shared seam without
  starting a terminal UI.
