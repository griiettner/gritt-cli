---
id: TKT-0022
namespace: griiettner
title: Add provider failover and remember session preferences
artifact: task
status: done
owner: griiettner
created: 2026-09-06
updated: 2026-09-06
dependencies:
  - TKT-0019
areas:
  - provider
  - harness
  - cli
skills:
  - dev-provider
  - dev-harness
  - dev-cli
---

# TKT-0022 Task: Add provider failover and remember session preferences

## Goal

Add native provider failover and last-used session preferences. Gritt should
try the configured default profile, then the profiles listed in an explicit
fallback order, and use the first profile whose credentials, endpoint, and
requested model are usable. It should remember the last successful native
profile, model, and effort for subsequent new sessions.

## Inputs

- `AgentBuilder` and `ControlPlane` provider/profile resolution.
- Provider model-list and request error classification.
- Native session persistence for provider profile, model, and effort.
- CLI, REPL, and TUI draft defaults and explicit selection precedence.

## Scope

- Put fallback resolution, availability classification, preference storage, and
  precedence in reusable Rust services behind the control-plane boundary. The
  TUI and CLI must be clients of those services, not separate implementations.
- Keep provider-neutral request/session/configuration types in `gritt-core`,
  provider probing and adapter behavior in `gritt-provider`, and orchestration
  in the harness control plane. Keep terminal rendering, argument parsing, and
  keychain/config-file writes at their existing edges.
- Expose structured operations and event streams that a future Rust T3Code
  frontend can call in-process. If a separate process is later needed, provide
  a structured CLI or local protocol rather than requiring terminal scraping.
- Add an explicit ordered fallback-profile configuration field. The configured
  default remains first; duplicate and unknown profiles are rejected clearly.
- Probe credentials and the provider/model endpoint before opening a new native
  session. Treat missing credentials, authentication failures, connection
  failures, and unavailable requested models as eligible fallback failures.
- Select a compatible model for the fallback profile using its catalog or an
  explicit per-profile fallback model, and report each skipped profile without
  exposing keys or prompt content.
- Persist last-used native profile, model, and effort in existing local state
  with additive, backward-compatible loading. Never persist a secret.
- Apply last-used preferences only to new sessions. Explicit CLI flags, draft
  selections, and configured defaults take precedence. Resumed sessions stay
  pinned to their stored native profile and model.
- Cover print, REPL, and TUI startup paths with the same resolver.

## Out of Scope

- Automatic provider switching in the middle of an active turn.
- Moving an existing session to another provider or replaying its history on a
  different model.
- Connector permission or model selection, since external agents own those.
- Task-complexity scoring or automatic reasoning-effort adaptation. The saved
  effort is the exact user-selected value, including provider default.
- Storing credentials in config, the database, logs, diagnostics, or tickets.

## Acceptance Criteria

- With OpenRouter unavailable and a usable fallback configured, a new session
  starts on the fallback and identifies the selected profile and model.
- If every candidate fails, Gritt reports a concise aggregate error naming the
  profiles and failure classes without revealing key values.
- A successful session with explicit model and effort causes the next new
  session to use those values when no override is supplied.
- Explicit `--profile`, `--model`, `--effort`, draft, and resume choices retain
  precedence over last-used preferences.
- A resumed session continues using its stored provider profile and model even
  when the configured default or fallback order changes.
- Existing configs and databases without fallback or last-used fields still
  load unchanged.
- Provider fallback behavior is shared across print, REPL, and TUI paths.
- A Rust frontend can invoke the same resolver, session, and preference APIs
  without depending on Ratatui or duplicating provider logic.

## Verification

- Unit tests for fallback ordering, duplicate/unknown profiles, error
  classification, model compatibility, and precedence.
- Provider fixture tests for missing key, 401, transport failure, and model-list
  failure, including redaction assertions.
- Session round-trip tests for last-used profile, model, and effort plus legacy
  database compatibility.
- CLI, REPL, and TUI tests proving the same resolver and visible fallback
  diagnostics.
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `cargo test --workspace --locked`
- `cargo build --release --locked`
