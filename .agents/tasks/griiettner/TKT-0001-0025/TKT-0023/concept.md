---
id: TKT-0023
namespace: griiettner
title: Expose reusable control plane API for T3Code
artifact: concept
status: concept
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

# TKT-0023 Concept: Expose reusable control plane API for T3Code

## Problem

The terminal client currently owns much of the visible orchestration, while a
future Rust T3Code frontend needs the same provider, session, permission, and
preference behavior. Duplicating those decisions in a second frontend would
cause drift. Calling the terminal UI or scraping its output would also make
structured state and streaming events difficult to consume.

## Intent

Define and expose a reusable Rust control-plane API. The CLI, REPL, TUI, and
future T3Code frontend should invoke the same services and receive typed
results and event streams. Terminal rendering and argument parsing remain
client responsibilities.

## Success Criteria

- A Rust frontend can open, resume, inspect, configure, and run sessions
  without importing Ratatui or duplicating provider and permission logic.
- Shared operations return typed data and normalized events suitable for an
  in-process client.
- CLI behavior remains compatible while moving orchestration behind the same
  seam.
- A separate-process path, if needed later, can wrap the seam with structured
  JSON or IPC rather than terminal scraping.
