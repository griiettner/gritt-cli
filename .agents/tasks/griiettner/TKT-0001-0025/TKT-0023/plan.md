---
id: TKT-0023
namespace: griiettner
title: Expose reusable control plane API for T3Code
artifact: plan
status: planning
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

# TKT-0023 Plan: Expose reusable control plane API for T3Code

## Sequence

1. Inventory current CLI, REPL, and TUI calls into `ControlPlane` and identify
   logic that is presentation-only versus reusable orchestration.
2. Define a narrow public Rust service surface for configuration, provider and
   model selection, sessions, execution modes, effort, permissions, and event
   streams.
3. Move or wrap shared behavior behind that surface without moving terminal
   rendering, keyboard handling, or platform-specific setup into core crates.
4. Add a compile-time or integration fixture representing a non-terminal Rust
   client, then verify the existing clients against the same surface.

## Decisions

- `gritt-core` remains free of filesystem, network, terminal, and provider
  dependencies. It owns only provider-neutral contracts and serialized data.
- Provider probing and adapters remain in `gritt-provider`; session, policy,
  tools, and orchestration remain in the harness control plane.
- The TUI and CLI consume the control plane. They do not become dependencies
  of it.
- The first T3Code integration is in-process, matching ADR-011. A local socket
  or structured subprocess protocol is deferred until a separate process is
  required.
- Keychain and config-file writes stay behind injected setup traits. A client
  may call the trait through the control plane or provide its own platform
  implementation; secrets never enter shared serialized state.
- The API must expose normalized events and typed errors. Clients must not
  parse human terminal output to determine state.
