---
id: TKT-0024
namespace: griiettner
title: Expose current models and selection for external connectors
artifact: plan
status: ready
owner: griiettner
created: 2026-09-06
updated: 2026-09-06
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

# TKT-0024 Plan: Expose current models and selection for external connectors

## Boundary

Keep the public operation provider neutral. `gritt-core` owns the model
catalog, freshness, selection, and typed discovery outcome. The connector
adapter owns the command, arguments, environment policy, output parser, and
the mapping from a selected model to the external agent's documented option.
The harness owns orchestration and picker state. The binary remains the edge
for configuration and process permissions.

The preferred interface is structured output from the external CLI. If a CLI
only provides a documented text command, parse that command behind the
adapter. Do not scrape the full-screen terminal UI when a machine-readable or
documented command exists.

## Sequence

1. Inventory the existing connector contract, model type, setup operation,
   picker reducer, and fixture process for Codex, Claude Code, Cursor, and
   OpenCode. Record which installed CLIs expose a documented model command.
2. Add a connector model discovery interface and typed outcomes for current,
   cached-stale, unavailable, unsupported, command failure, and malformed
   output cases.
3. Implement one adapter per supported CLI. Keep command construction fixed
   and argument values typed. Never interpolate prompt text or credentials into
   a shell command.
4. Add selection to the shared setup operation and wire print, REPL, and TUI
   paths to it. Preserve the existing default when the user does not choose a
   model.
5. Add fixture tests for every parser and failure class, control-plane tests
   for selection precedence and stale results, and a live test gate for each
   CLI that is installed and authenticated.

## Decisions

- Discovery is on demand at connector setup and may use a short-lived cache,
  but it must expose freshness and support explicit refresh. It must not make
  an unauthenticated network call outside the CLI's documented behavior.
- Explicit user selection applies only to a new connector session. A resumed
  session keeps the model and connector state recorded for that session.
- If discovery cannot produce a catalog, Gritt may offer the external CLI's
  default only when the connector contract says that default is valid. It must
  not invent a model identifier or claim that the catalog is current.
- The first implementation covers the connectors already in the product
  contract, Codex, Claude Code, Cursor, and OpenCode. A connector with no
  documented model selection remains visible as unsupported rather than being
  silently treated as equivalent.
