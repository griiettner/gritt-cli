---
id: TKT-0027
namespace: griiettner
title: Allow provider and model changes within an active conversation
artifact: update
status: done
owner: griiettner
created: 2026-09-08
updated: 2026-09-08
areas:
  - crates/gritt-core
  - crates/gritt-connector
  - crates/gritt-harness
skills:
  - codebase-design
  - dev-harness
  - tkt-update
  - write
---

# TKT-0027 Update: Extend pricing to connector catalogs

## Trigger

The native provider picker showed per-million-token pricing, but the OpenCode
connector picker did not. OpenCode's verbose model listing reports the missing
cost metadata.

## Changed behavior

OpenCode model discovery now calls `opencode models --verbose` and carries its
optional `cost.input` and `cost.output` values through `ConnectorModel`. The
connector picker renders both when present. Codex and Cursor remain unchanged
because their current model-list outputs do not report prices.

## Files

- `crates/gritt-core/src/connector.rs`: optional connector model pricing.
- `crates/gritt-connector/src/protocols/opencode.rs`: verbose discovery and
  cost parsing.
- `crates/gritt-connector/src/protocols/codex.rs` and `cursor.rs`: explicit
  missing-price values.
- `crates/gritt-harness/src/tui/app.rs`: connector picker pricing note.
- Parser and picker tests cover the new path.

## Validation

- `cargo fmt --all --check` passed.
- OpenCode parser pricing test passed.
- Connector picker pricing test passed.
- Workspace `cargo check` passed.

## Completion Gate

- Acceptance: satisfied for catalogs that report prices. OpenCode now shows
  per-million-token input and output costs.
- Scope: stayed within the provider-neutral connector model contract, the
  OpenCode adapter, and the harness picker.
- Security and safety: model metadata only; no credentials, prompts, or tool
  output are added to catalogs.
- Regression risk: connector prices are optional and old cache entries remain
  readable through serde defaults. Connectors without prices remain usable.
- Follow-up: add pricing parsing for another connector only if its CLI
  exposes a documented cost field.
