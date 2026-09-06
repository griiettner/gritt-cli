---
id: TKT-0024
namespace: griiettner
title: Expose current models and selection for external connectors
artifact: task
status: ready
owner: griiettner
created: 2026-09-06
updated: 2026-09-06
dependencies:
  - TKT-0012
  - TKT-0019
  - TKT-0022
areas:
  - crates/gritt-core
  - crates/gritt-provider
  - crates/gritt-harness
  - crates/gritt
skills:
  - dev-provider
  - dev-harness
  - codebase-design
  - tdd
  - write
---

# TKT-0024 Task: Expose current models and selection for external connectors

## Goal

Allow a user to refresh and choose the current model exposed by an installed
external agent CLI before starting a connector session. Use one control-plane
operation for print, REPL, and TUI clients while keeping every CLI command and
parser inside its connector adapter.

## Inputs

- Existing connector contract and supervision from TKT-0012.
- Existing setup, picker, session, and model-selection paths from TKT-0019.
- Provider-neutral model and session contracts from TKT-0016 and ADR-007.
- Connector authority and failure behavior from ADR-010.
- Cache freshness and stale fallback rules from ADR-008.

## Scope

- Add a provider-neutral connector model catalog and discovery result with
  model id, display label when available, source, fetched time, and freshness.
- Add connector-specific discovery and selection support for Codex, Claude
  Code, Cursor, and OpenCode using their documented CLI commands or structured
  interfaces. Verify the exact commands against installed versions or
  committed fixtures before implementation.
- Add explicit refresh and selection to the shared connector setup flow and
  show loading, stale, unavailable, and unsupported states in print, REPL, and
  TUI paths.
- Pass an explicit selected model through the connector's documented startup
  or session option. Preserve default behavior when no selection is made.
- Add parser fixtures, fake process tests, malformed-output tests, missing
  executable tests, refresh failure tests, stale-cache tests, and available
  live smoke tests gated by the existing live-test environment policy.
- Update user and connector diagnostics so they identify the CLI and model
  source without printing keys, prompts, or tool content.

## Out of Scope

- Native provider model discovery or native provider selection behavior.
- Automatic model switching during an active turn or migration of a resumed
  connector session to another model.
- Guessing model names from marketing labels, hard-coded current model lists,
  or scraping a full-screen CLI when a documented interface exists.
- Provider CLI version checks or installation updates. Those belong to
  TKT-0025.
- Changing the authority of external agents or reimplementing their tools in
  Gritt.

## Acceptance Criteria

- A supported installed connector can return a current catalog through the
  shared operation, and the catalog identifies its source and fetch time.
- A user can select a catalog entry before a new connector session, and a
  fixture proves the selected identifier reaches the external CLI in the
  documented form.
- Refresh failure falls back to a cached catalog marked stale. No stale entry
  is presented as current.
- Missing, unsupported, failed, and malformed discovery paths produce typed
  diagnostics and leave native sessions and other connectors usable.
- Print, REPL, and TUI paths use the same discovery and selection service and
  agree on default and explicit-selection precedence.
- Resumed connector sessions retain their stored model choice and do not run a
  new selection implicitly.
- No secret, prompt text, tool content, or arbitrary user text enters a
  command position, log, fixture, or diagnostic.

## Verification

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `cargo test --workspace --locked`
- Fixture tests for every connector parser and each typed failure outcome.
- Control-plane tests for refresh, stale fallback, explicit selection,
  default selection, and resumed-session behavior across print, REPL, and TUI.
- Live connector smoke tests when the CLI and authentication are available;
  otherwise run the committed fixtures and record the unavailable reason.
- `cargo build --release --locked`
- `./.agents/gritt-agent ticket validate`
