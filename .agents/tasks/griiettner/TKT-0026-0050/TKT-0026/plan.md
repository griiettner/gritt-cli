---
id: TKT-0026
namespace: griiettner
title: Report each connector's own MCP inventory at session start
artifact: plan
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

# TKT-0026 Plan: Report each connector's own MCP inventory at session start

## Boundary

`gritt-core` owns the provider-neutral shape: one server entry (name,
launch command or URL as a display string only, never a live argument
vector Gritt would run, and a normalized status), and one typed
discovery outcome, on the same pattern as `ConnectorModelDiscovery`
(TKT-0024) and `ConnectorVersionCheck` (TKT-0025): a current result, and
typed reasons for unavailable, unsupported, command failure, and
malformed output. No stale-cache variant is needed here (see Decisions).

`gritt-connector` owns each protocol's documented listing command and
its own parser, run through the existing supervised `probe` helper
(`crates/gritt-connector/src/health.rs`), the same one `discover_models`
and `check_version` already use. Nothing here becomes a second way to
launch or manage a server; it only reads what the connector reports.

`gritt-harness`'s `ControlPlane` owns one operation
(`connector_mcp_inventory`, mirroring `connector_models` and
`connector_version`) called from `open_with` for a new connector
session, and a line formatter print/REPL/TUI all use. The TUI keeps this
inventory on its own field, separate from `IntegrationsSection::mcp`
(Gritt's own servers) and `mcp_owner`, so the two lists are never merged
or visually confused. The binary stays the edge for any explicit
refresh flag.

## Sequence

1. Capture `opencode mcp list`'s real output on this machine, and check
   Cursor's published CLI reference for an equivalent command, before
   writing either parser. Do not guess a shape from the command's name.
2. Add the provider-neutral types and typed outcome to
   `crates/gritt-core/src/connector.rs`, next to
   `ConnectorModelDiscovery` and `ConnectorVersionCheck`.
3. Add `Protocol::mcp_list_args` / `mcp_list_source` (mirroring
   `model_list_args` / `model_list_source`) and one parser per connector
   in `crates/gritt-connector/src/protocols/`. Redact anything that
   looks like a credential (reuse `is_credential_option` /
   `redact::redact_text`) out of every field before it is stored.
4. Add `ExternalConnector::discover_mcp_inventory`, run through the
   existing `probe` with the connector's health timeout. No disk cache
   (see Decisions): a failure or timeout produces its typed outcome
   directly, never a stale fallback.
5. Add `ControlPlane::connector_mcp_inventory` and a line formatter.
   Call it from `open_with` for a newly created connector session,
   alongside the existing `connector_models` / `connector_version`
   calls, and thread the result onto `Opened` the same way.
6. Update `startup_notes` (print/REPL) and the TUI sidebar's
   `Integrations` section: replace the `connector_mcp` placeholder
   string with the real rows (server name plus normalized status);
   keep Gritt's own `mcp` list and `mcp_owner` line exactly as they are.
7. Add fixture and control-plane tests per connector and per typed
   outcome, plus a live test gated by the existing live-test
   environment variable policy for whichever CLIs are installed.

## Decisions

- No persistent cache and no explicit refresh flag. Unlike model lists
  and version checks, an MCP inventory read is local configuration (plus,
  for Claude Code, a bounded live health check) rather than a
  rate-limited or expensive network call: a fresh read on every session
  open, bounded by the existing health timeout, is cheap enough that
  staleness is not a real concern. A future ticket may add one if a
  connector's check turns out to be slow enough to warrant it.
- The inventory is read-only and display-only. It never becomes a
  trust, approval, enable, or disable action; ADR-010 stays intact.
- A server's command, URL, or environment value is stored only as
  already-redacted display text. Gritt never re-runs, re-parses
  arguments from, or connects to a listed server itself.
- A connector with no documented command is `Unsupported`, shown as
  plainly as Claude Code's `Unsupported` model listing in TKT-0024. This
  ticket does not scrape a full-screen UI or invent a command.
