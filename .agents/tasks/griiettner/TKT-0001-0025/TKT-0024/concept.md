---
id: TKT-0024
namespace: griiettner
title: Expose current models and selection for external connectors
artifact: concept
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

# TKT-0024 Concept: Expose current models and selection for external connectors

## Problem

The connector can launch an installed agent, but Gritt cannot reliably show or
select the models that the agent currently supports. The TUI therefore leaves
the user with a connector that works only with the agent's implicit default.
Provider CLIs own their model catalog and may change it independently of a
Gritt release, so a static list in Gritt would become stale.

## Intent

Give each external connector a provider-specific model discovery adapter that
uses the CLI's documented command or machine-readable interface, normalizes
the result into the existing provider-neutral model choice, and passes an
explicit selection back to the connector when a session starts. The model
catalog must be visible in print, REPL, and full-screen setup paths through the
same control-plane operation.

## Success Criteria

- Installed supported CLIs expose their current model identifiers and display
  names when the user requests connector models.
- The user can choose a discovered model before a new connector session, and
  the connector receives that choice through its documented launch or session
  option.
- A missing CLI, unsupported discovery interface, command failure, malformed
  output, or stale catalog produces a typed, useful diagnostic and never
  breaks native provider sessions.
- The catalog records its source and freshness. A cached result is marked
  stale rather than presented as current after refresh failure.
- Connector-owned capabilities stay behind connector adapters. No model name
  guessing or provider-specific branching leaks into the shared session model.
