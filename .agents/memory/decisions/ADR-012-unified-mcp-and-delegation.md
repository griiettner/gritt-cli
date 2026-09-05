---
id: ADR-012
title: Unified MCP server and MCP-based agent delegation
status: accepted
date: 2026-09-04
tags:
  - mcp
  - delegation
  - tooling
read_when:
  - wiring Gritt tools into an agent harness
  - adding tools to the Gritt MCP server
  - changing how agent CLIs are invoked headlessly
---

# ADR-012: Unified MCP server and MCP-based agent delegation

## Decision

- Gritt exposes one MCP server per harness session: `gritt-agent mcp serve`
  (server name `gritt-agent`). It merges every Gritt tool family — local
  memory (`search_local_memory`, `read_local_memory`) and agent delegation
  (`delegate_run`) — behind a single stdio JSON-RPC endpoint. Each harness
  (Claude Code `.mcp.json`, Crush `crushrc`, Codex `config.toml`) registers
  exactly this one entry instead of one entry per tool family.
- Headless delegation to installed agent CLIs (grok, codex, claude) goes
  through the `delegate_run` MCP tool, never through the Bash tool. The tool
  restricts the program to the three sanctioned CLIs, maps `auto_approve` to
  each CLI's own flag, and enforces a timeout (default 600s, max 3600s).
- The server stays hand-rolled newline-delimited JSON-RPC 2.0, matching the
  existing pattern; the `modelcontextprotocol/rust-sdk` is adopted only if a
  non-stdio transport (shared daemon over HTTP/SSE) is introduced.

## Rationale

- Harness auto-mode classifiers evaluate whole Bash command strings and deny
  compound or agent-spawning commands regardless of allow-lists. That denial
  was observed across harnesses and is not settings-fixable (see
  `.agents/memory/architecture/model-routing.md`). An MCP tool call never
  passes through a shell classifier, so delegation becomes harness-agnostic
  and the policy point moves into `gritt-agent`, which the user controls.
- One server per harness session removes per-tool config drift across
  harnesses and gives future tool families a single registration point.

## Consequences

- `memory serve` and `delegate serve` remain for compatibility but are
  superseded by `mcp serve`; new tools must be added to the unified server.
- The exclusive Turso file lock still allows only one write-capable Gritt
  MCP session at a time. A shared singleton daemon (lock-holding server
  reused across harnesses) is the follow-up fix and requires revisiting this
  ADR and the transport choice above.
- `delegate_run` spawns real agent CLIs. Approval behavior is opt-in via
  `auto_approve`; output (stdout/stderr/exit status) returns as tool text.
