---
id: TKT-0015
namespace: griiettner
title: Build an OpenCode-inspired full-screen agent TUI with generic MCP harness support
artifact: concept
status: concept
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
chain_role: orchestrator
areas:
  - crates/gritt-core
  - crates/gritt-provider
  - crates/gritt-harness
  - crates/gritt
  - docs
  - .agents/plans
skills:
  - tkt
  - tkt-exec-chain
  - dev-harness
  - dev-provider
  - codebase-design
  - tdd
  - write-plan
---

# TKT-0015 Concept: Build an OpenCode-inspired full-screen agent TUI with generic MCP harness support

## Problem

Gritt has a working Ratatui transcript loop, but it does not yet feel like a
real agent workspace. It lacks the spacious home and conversation layouts,
discoverable slash commands, provider/model/effort setup, session sidebar,
generic MCP execution, and the responsiveness needed for long streaming
sessions. The workspace already contains `.mcp.json`; the harness must account
for every configured MCP server rather than special-casing the two local-memory
entries currently present.

## Intent

Build and integrate an OpenCode-inspired full-screen TUI using Gritt's existing
Rust control plane, provider adapters, native tool policy, session store, and
connector supervision. Crush is a secondary reference for the session
sidebar, provider onboarding, and model-switching experience. MCP servers are
owned by the harness, configured from the workspace file, and exposed through
the same provider-neutral tool event model.

## Success Criteria

- The TUI supports home and conversation layouts, a responsive composer,
  slash commands, searchable connection/model/effort pickers, approvals, tool
  details, session navigation, and the Crush-inspired sidebar.
- Every `.mcp.json` `mcpServers` entry is enumerated, validated, shown with a
  state, and either initialized or given a visible reason for not running.
- Native sessions can discover and invoke approved MCP tools through the
  harness permission engine, with safe lifecycle, cancellation, timeout, and
  shutdown behavior.
- The UI remains responsive while streaming, loading catalogs, and initializing
  MCP servers, with recorded performance evidence and no regression in print,
  REPL, connector, or approval flows.
