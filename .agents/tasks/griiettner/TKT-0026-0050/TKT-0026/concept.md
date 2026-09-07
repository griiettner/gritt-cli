---
id: TKT-0026
namespace: griiettner
title: Report each connector's own MCP inventory at session start
artifact: concept
status: done
owner: griiettner
created: 2026-09-06
updated: 2026-09-06
areas:
  - crates/gritt-core
  - crates/gritt-connector
  - crates/gritt-harness
  - crates/gritt
skills:
  - dev-provider
  - dev-harness
  - dev-cli
  - codebase-design
  - tdd
  - write
---

# TKT-0026 Concept: Report each connector's own MCP inventory at session start

## Problem

A connector session's sidebar shows `<connector>'s own MCP: not reported`
(`crates/gritt-harness/src/tui/sidebar.rs`). That line is honest but
useless: Codex, Claude Code, and OpenCode each document a command that
lists their own configured MCP servers and each server's status, and
Gritt never asks. A user has to leave Gritt and run the connector's CLI
by hand to see what tools it has available.

Confirmed on this machine (2026-09-06):

- `codex mcp list` — a table per transport (stdio, then any remote/url
  entries) with columns `Name`, `Command`/`Url`, `Args`, `Env`, `Cwd`,
  `Status` (`enabled`/`disabled`), `Auth` (`Unsupported`, `Not logged
  in`, or a real state).
- `claude mcp list` — runs a live per-server health check, then prints
  one line per server: `name: command - <status>` where status is a
  glyph plus word (`✔ Connected`, `⏸ Pending approval (run \`claude\` to
  approve)`, etc.).
- `opencode mcp list` (alias `ls`) — "list MCP servers and their status";
  exact output shape not yet captured here and must be read from a real
  run before the parser is written.
- `cursor-agent` is not installed on this machine; whether it documents
  an equivalent command is unknown and must be checked against its
  published CLI reference before implementation. If it has none, it
  stays `Unsupported`, matching how Claude Code has no model-listing
  command in TKT-0024.

## Intent

Add one more shared, provider-neutral discovery operation to the
control plane, following the exact shape TKT-0024 (model catalogs) and
TKT-0025 (version checks) already established: a typed outcome in
`gritt-core`, one parser per connector adapter in `gritt-connector`
using each CLI's documented command, and one `ControlPlane` operation
that print, REPL, and the TUI all call when a connector session opens.
The result replaces the "not reported" placeholder with the real
inventory, or an honest typed reason when the CLI has none, is missing,
times out, or returns something unparseable.

This inventory is display-only. A connector owns its own MCP servers,
authentication, and approvals under ADR-010; Gritt does not gain any
new way to add, remove, approve, or otherwise manage them. This is not
Gritt's own MCP list (`.mcp.json`, `/mcp`, `gritt mcp`) — that already
works and is unaffected. The two are shown as clearly separate sections
so a user cannot mistake one connector's server for the other's.

## Success Criteria

- Opening a session on an installed connector that documents an MCP
  listing command shows that connector's servers and their status in
  place of "not reported", through the same operation in print, REPL,
  and the TUI.
- A connector with no documented listing command, a missing executable,
  a failed command, or unparseable output shows a typed, honest reason
  instead — never a guess, and never something that blocks the session
  or affects any other connector.
- Nothing from a listed server's command, URL, header, or environment
  value (a bearer token, an API key) reaches a log, diagnostic, fixture,
  or transcript.
