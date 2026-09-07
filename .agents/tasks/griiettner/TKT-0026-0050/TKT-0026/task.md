---
id: TKT-0026
namespace: griiettner
title: Report each connector's own MCP inventory at session start
artifact: task
status: done
owner: griiettner
created: 2026-09-06
updated: 2026-09-06
dependencies:
  - TKT-0012
  - TKT-0024
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

# TKT-0026 Task: Report each connector's own MCP inventory at session start

## Goal

Replace the sidebar's `<connector>'s own MCP: not reported` placeholder
(`crates/gritt-harness/src/tui/sidebar.rs:371`, backed by
`IntegrationsSection::connector_mcp`, set at
`crates/gritt-harness/src/tui/app.rs:2992`) with the connector's real MCP
server inventory, using the exact CLI command each connector documents.
Use one control-plane operation for print, REPL, and TUI, on the same
shape `ControlPlane::connector_models` (TKT-0024) and
`ControlPlane::connector_version` (TKT-0025) already established.

## Inputs

- Connector contract and control plane from TKT-0012.
- The shared-discovery-operation pattern, typed outcomes, and
  `open_with` wiring from TKT-0024
  (`crates/gritt-harness/src/control.rs`, `crates/gritt-connector/src/supervise.rs`).
- The supervised process probe (`crates/gritt-connector/src/health.rs::probe`)
  and its redaction helpers (`crates/gritt-connector/src/redact.rs`,
  `is_credential_option` in `crates/gritt-connector/src/lib.rs`).
- Confirmed CLI output on this machine: `codex mcp list` (a table per
  transport with `Name`/`Command`or`Url`/`Args`/`Env`/`Cwd`/`Status`/`Auth`
  columns), `claude mcp list` (a live per-server health check, then
  `name: command - <glyph> <status>` lines). `opencode mcp list` (alias
  `ls`) exists but its exact output was not captured before this ticket
  was written; capture it before implementing its parser. Cursor's
  equivalent command is unconfirmed; check its published CLI reference
  before implementing or declaring it `Unsupported`.

## Scope

- Add a provider-neutral MCP server entry type and a typed discovery
  outcome to `gritt-core` (current inventory; unavailable, unsupported,
  command-failure, and malformed-output reasons), next to
  `ConnectorModelDiscovery` and `ConnectorVersionCheck`.
- Add `Protocol::mcp_list_args` / `mcp_list_source` with an `Unsupported`
  default, and one parser per connector CLI that actually documents a
  listing command, verified against the installed CLI or its published
  reference (never scraped from a full-screen UI, never guessed).
- Add discovery on `ExternalConnector`, run through the existing
  supervised probe and its health timeout; redact anything that looks
  like a credential out of every stored field (server command, URL,
  header names, environment values) before it is kept anywhere.
- Add `ControlPlane::connector_mcp_inventory` and a shared line
  formatter; call it from `open_with` when a new connector session
  opens, alongside the existing model and version discovery calls, and
  carry the result on `Opened`.
- Update print and REPL startup notes and the TUI sidebar's
  `Integrations` section to show the fetched rows in place of the
  current placeholder string, while leaving Gritt's own `mcp` list and
  `mcp_owner` line untouched and visually separate.
- Add fixture tests for every connector's parser and each typed
  outcome, control-plane tests for the shared operation, and a live
  test gated by the existing live-test environment variable policy.

## Out of Scope

- Adding, removing, enabling, disabling, authenticating, or approving a
  connector's own MCP servers. Gritt only displays what the connector
  reports; ADR-010's authority boundary is unchanged.
- Any change to Gritt's own MCP list, `.mcp.json`, `/mcp`, or `gritt mcp`
  trust flow.
- Native provider sessions, which have no external connector and
  nothing to report here.
- Live updates while a connector session is already running; this
  ticket covers session start only.
- Adding a persistent cache or an explicit refresh flag (see `plan.md`
  Decisions).
- Connector CLI version checks or update actions; that is TKT-0025.
- Adding a new connector beyond Codex, Claude Code, Cursor, and
  OpenCode.

## Acceptance Criteria

- Opening a session on an installed connector that documents an MCP
  listing command reports its server names and normalized status
  through the shared control-plane operation, and a fixture proves the
  parsed shape matches the connector's real (or, for an uninstalled
  CLI, documented) output.
- Print, REPL, and the TUI use the same operation and show the same
  information; the TUI sidebar no longer shows the literal string
  "not reported" for a connector that supports the check.
- A connector with no documented listing command remains `Unsupported`
  and is shown as such, not silently equivalent to one that supports it.
- A missing executable, a failed command, a timeout, or unparseable
  output produces its own typed diagnostic, leaves the connector session
  usable, and never affects native sessions or another connector.
- No secret, bearer token, API key, or other credential-shaped value
  from a listed server's command, URL, header, or environment reaches a
  log, diagnostic, fixture, or transcript.
- The check never blocks or measurably delays a connector session
  opening: it is bounded by the connector's existing health timeout and
  a failure there is non-fatal to the session.
- Gritt's own MCP list and the connector's own MCP inventory remain
  clearly separate in every client; nothing merges the two.

## Verification

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `cargo test --workspace --locked`
- Fixture tests for every connector's parser and each typed outcome.
- Control-plane tests for the shared operation across print, REPL, and
  TUI, including a connector with no listing command and a command
  failure.
- Live connector smoke tests when the CLI and authentication are
  available, gated the same way TKT-0024's live tests are; otherwise run
  the committed fixtures and record the unavailable reason.
- `cargo build --release --locked`
- `./.agents/gritt-agent ticket validate`
