---
id: TKT-0026
namespace: griiettner
title: Report each connector's own MCP inventory at session start
artifact: report
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
  - tkt
  - tkt-exec
  - dev-harness
  - dev-cli
  - tdd
  - write
  - review-ticket
---

# TKT-0026 Report: Report each connector's own MCP inventory at session start

## Summary

A connector session now shows the external agent's own MCP servers and
their status in place of the `not reported` placeholder, through one
control-plane operation (`ControlPlane::connector_mcp_inventory`) and one
line formatter (`connector_mcp_lines`) that print, REPL, and the TUI
share. The inventory is read once when a new connector session opens,
bounded by the connector's health timeout, and every failure is a typed
outcome that leaves the session usable.

`gritt-core` owns the provider-neutral shape: `ConnectorMcpServer` (name,
transport word, launch command or URL as display text, normalized
`ConnectorMcpStatus`, the agent's own hint), `ConnectorMcpInventory`, and
the typed `ConnectorMcpDiscovery` outcome (`Current`, `Unavailable`,
`Unsupported`, `CommandFailure`, `TimedOut`, `MalformedOutput`). The
`Connector` trait gained `discover_mcp_inventory` with an `Unsupported`
default, so the native connector needed no change.

`gritt-connector` owns each protocol's documented command and parser
through new `Protocol` hooks (`mcp_list_args`, `mcp_list_source`,
`mcp_list_unsupported_reason`, `parse_mcp_inventory`), and
`ExternalConnector::discover_mcp_inventory_inner` runs them through the
existing supervised probe, in the session workspace (a new `probe_in`
beside `probe`) so an agent's project-scoped servers count. Every kept
field goes through redaction
before storage: known secrets out of every field, and for the target a
new `redact::redact_target` that masks the value of any credential-shaped
option (`--api-key ...`, `--token=...`) and strips userinfo, query, and
fragment from a URL.

Per connector, verified on this machine on 2026-09-06:

| Connector | Command | What the parser reads |
| --- | --- | --- |
| Codex 0.153.x | `codex mcp list --json` | name, `enabled`, `disabled_reason`, `transport.type`, `transport.command` or `transport.url`, `auth_status`. `args`, `env`, `env_vars`, `http_headers`, and `bearer_token_env_var` are never read. |
| Claude Code 2.1.x | `claude mcp list` | one `name: <launch line> - <glyph> <status>` line per server after the health-check banner; the launch line includes the server's arguments. |
| OpenCode 1.18.29 | `opencode mcp list` | boxed list with status word, hint lines, and the command or URL last; ANSI and box prefixes stripped. |
| Cursor | none | `Unsupported`: the published reference documents `agent mcp list` as an interactive menu, which the plan forbids scraping. |

The harness carries the result on `Opened.connector_mcp` for a newly
created connector session opened through `open_with`, which print and
REPL use. The binary's `startup_notes` prints it on stderr as a `note:`
(`Current`, `Unsupported`) or `warning:` (every failure). The TUI opens
sessions through `open`, which drops the notes; after review that path
also skips the three advisory reads (models, version, inventory) it never
showed, and the TUI instead reads the inventory detached after adoption,
in parallel with the version check. The sidebar draws it under
`<connector>'s own MCP` after Gritt's own list, with `checking` until it
lands. Gritt's own MCP list, `mcp_owner`, and the `/mcp` flow are
untouched.

## Key Decisions

- Codex uses `--json`, not the table: structured output is preferred and
  the table pads columns to the widest environment line.
- A Codex server is `Enabled`, never `Connected`: its listing reads
  configuration and runs no live check. Claude Code and OpenCode run
  their own checks, so they report `Connected` or `Failed`.
- Cursor is `Unsupported` with the reason `cursor-agent mcp list opens an
  interactive menu; no machine-readable listing is documented`, mirroring
  Claude Code's unsupported model listing in TKT-0024.
- `TimedOut` is its own variant. The concept listed four failure reasons;
  the acceptance criteria name a timeout as needing its own typed
  diagnostic, and the health-timeout error is distinguishable, so it is
  not folded into `CommandFailure`.
- No cache and no refresh flag, per `plan.md`.
- Print and REPL read the inventory inline before the session opens, the
  same way model discovery does; the TUI reads it in the background and
  no longer pays for the inline reads. Both are bounded by the health
  timeout (15s default).
- The Codex parser takes the first line-leading `[` that parses as a JSON
  array, so a diagnostic line before the array, even one that starts with
  a bracket, does not turn the listing into `MalformedOutput`.
- One transport vocabulary for inferred transports: `http` for a URL and
  `stdio` for a command (`protocols::transport_from_target`). Codex's own
  `type` word (`stdio`, `streamable_http`) is kept as reported.

## Alternatives Considered

- Parsing the Codex table: rejected for the JSON above.
- Adding a `/mcp`-style REPL command for the connector's list: not asked
  for; the ticket scope names startup notes only.
- Masking credential-shaped positional tokens by pattern (`sk-...`):
  left out to keep the redaction rule the same one `extra_args` already
  uses; see Follow-up.

## Assumptions

- The Claude Code listing exits 0 even when a server fails or is pending
  (confirmed live with a broken project server), so a non-zero exit is
  treated as `CommandFailure` without trying to parse stdout.
- OpenCode's `(OAuth)` suffix and the `connected`, `needs client
  registration`, and `not initialized` words come from the CLI source
  (`packages/opencode/src/cli/cmd/mcp.ts`), not from a live run: this
  machine has no OpenCode server that reaches those states. `not
  initialized` maps to `Unknown` with the words kept in `detail`.
- The TUI reads the version and the inventory from two detached tasks;
  they are separate processes with nothing shared, so the adoption test
  now accepts the two messages in either order.

## Edge Cases and Failures

- Claude Code's live check took about five seconds here with four
  servers; with many servers it can exceed the health timeout, which is
  reported as `TimedOut`.
- An empty listing (`No MCP servers configured` from either text CLI, or
  `[]` from Codex) is `Current` with no servers and reads `reports no MCP
  servers of its own`, never an error.
- The OpenCode parser keeps the last continuation line as the target and
  earlier ones as the hint, which is the shape the CLI source prints.
- Only one build error surfaced during execution: the TUI snapshot test
  builds `IntegrationsSection` literally and needed the new field.

## Validation

- `cargo fmt --all --check`: pass.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: pass.
- `cargo test --workspace --locked`: pass, every suite green.
- `cargo test -p gritt-connector --test mcp_inventory`: 14 tests, pass
  (parser fixtures per connector, redaction of env, header, query, and
  argument secrets, empty listing, missing executable, Cursor
  unsupported, command failure, timeout under 2s, malformed output, a
  Codex listing behind a bracketed diagnostic line, and a listing read
  from the session workspace).
- `cargo test -p gritt-harness --test connector_session`: 18 tests, pass,
  including the two new control-plane tests (opened session carries the
  inventory and lines; resumed session does not re-read; native is
  `Unsupported`; a missing connector is `Unavailable`; a failed listing
  is typed and the session still runs a turn).
- `cargo test -p gritt --test e2e print_mode_notes_the_agents_own_mcp_inventory`:
  pass (print-mode stderr note with redacted `--token` value; no note on
  resume).
- `GRITT_LIVE_CONNECTOR_TESTS=1 cargo test -p gritt-connector --test live live_mcp`:
  pass against the installed CLIs: Codex 9 servers, Claude Code 4,
  OpenCode none.
- `cargo build --release --locked`: pass.
- `./.agents/gritt-agent ticket validate`: ok.
- Not run: a manual pass in a real terminal for the sidebar change. The
  snapshot and app-level tests cover the rendering; the session is
  non-interactive.

## Completion Gate

- Acceptance: yes. Installed connectors with a documented command report
  names and normalized status through the shared operation with fixtures
  from real output; print, REPL, and TUI use the same operation and the
  TUI no longer shows `not reported` for any connector; Cursor stays
  `Unsupported` and is shown as such; missing executable, failed command,
  timeout, and unparseable output are each typed, non-fatal, and scoped
  to one connector; no env value, header, bearer variable, query token,
  or credential option value reaches the kept data (asserted in the
  connector, harness, and e2e tests); the read is bounded by the health
  timeout; Gritt's list and the connector's stay in separate fields and
  separate sidebar blocks.
- Scope: within the ticket. Files touched are in the four listed crates
  plus `docs/connectors.md` and the connector fixtures. The working tree
  also carries the earlier uncommitted TKT-0024/0025 lifecycle-audit
  changes, which this ticket did not modify.
- Validation: as listed; one manual terminal pass not run.
- Security and safety: no new network access, no shell, no new way to
  add, approve, or connect to a server. The listing command is a fixed
  vector from the protocol. Codex's JSON exposes environment values and
  headers in the clear and the parser never reads those keys; Claude
  Code's launch line is masked by `redact_target`. Residual risk: a
  credential passed as a bare positional argument to a Claude Code
  server, not through an option name, is only masked when Gritt already
  knows the value.
- Regression risk: additive. `Connector` and `Protocol` gained default
  methods; `Opened`, `IntegrationsSection`, and `UiMsg` grew and every
  constructor and match compiled. Opening a new connector session now
  runs one more CLI command inline in print and REPL mode (bounded by
  the health timeout) and one more detached in the TUI. The sidebar's
  Integrations block is longer for connector sessions; snapshots for the
  fixture screens are unchanged because those are native sessions.
- Follow-up: below.
- Assumptions: listed above.
- Review: `review/ticket` ran over the diff. The harness code-review
  skill (medium) returned before its verification pass with only "the
  eight finder angles are still running"; all eight finder agents then
  completed and their findings arrived as task notifications, without a
  consolidated verdict. Findings on this diff and what was done: the
  TUI's `open` path ran the listing inline and adopt ran it again
  (fixed: `open` skips advisory reads, adopt reads once); the listing
  ran in Gritt's launch directory rather than the session workspace
  (fixed: `probe_in` with the workspace root); the Codex parser anchored
  on the first `[` (fixed); adopt serialized the version and inventory
  reads (fixed: parallel); `mcp_list_source` is derivable from the args
  (kept, for parity with `model_list_source`); the two text parsers share
  shape (partly folded: one transport helper; the status classifiers
  stay per CLI because their word lists differ); `redact_target` walks
  tokens like `diagnostic_args` (kept: the two mask different things;
  noted below). The remaining findings concern the earlier uncommitted
  TKT-0024/0025 audit changes and are listed under Follow-up.

## Follow-up

- Mask credential-shaped positional tokens (for example `sk-` prefixes)
  in `redact_target`, so a Claude Code launch line that passes a key
  without an option name is covered too; and share one token masker
  between `redact_target` and `supervise::diagnostic_args`.
- Give `health::probe` a typed failure (spawn, timeout, output too
  large) so the MCP and version paths stop sniffing `did not answer` in
  the message text.
- Findings from the review on the uncommitted TKT-0024/0025 audit changes
  in this working tree, not touched by this ticket and worth a look
  before they are committed: `App::set_session` (`tui/app.rs` near line
  3260) now clears `connector_choice` and `connector_model` on every
  call, including the phase-change path, so `/models` in a live
  connector session and a `/connect` draft followed by `/plan` lose the
  connector; the rewritten `probe` reads lines through `BufReader::lines`
  and stops at the first non-UTF-8 byte where the old `Command::output`
  path decoded lossily; the version-cache identity string uses
  `serde_json::to_string(...).expect(...)` over paths, which panics on a
  non-UTF-8 path; `update_action` returns `None` for npm and the
  `Offline` mode used at session open skips the `npm root` check, so an
  npm-installed CLI never shows its update offer at startup; the old
  version cache entries without an `installation` identity are
  discarded; `ProcessGuard::drop` runs a blocking `kill` on a runtime
  thread and then kills the group again asynchronously.
- Capture a live OpenCode `connected` and `needs authentication` line
  when such a server is configured, and replace the source-derived
  fixture lines.
- Re-verify Cursor's `mcp list` on a machine with the CLI installed, in
  case a later release documents a JSON or non-interactive form.
- A `/mcp`-style REPL and TUI command to re-read the connector's
  inventory mid-session, if the session-start read proves too coarse.
