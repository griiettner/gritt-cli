---
id: TKT-0017
namespace: griiettner
title: Implement generic .mcp.json MCP runtime and harness tool dispatch
artifact: report
status: done
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
chain_role: worker
chain_parent: TKT-0015
dependencies:
  - TKT-0016
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

# TKT-0017 Report: Implement generic .mcp.json MCP runtime and harness tool dispatch

## Summary

Worker 2 of the TKT-0015 chain. Gritt now reads `<workspace>/.mcp.json`
itself, approves each server before running it, connects over stdio or
Streamable HTTP, discovers tools through paginated `tools/list`, and
dispatches calls through the existing permission engine and event model. No
product code knows a server name, a vendor, or how many entries a file holds.

Chain facts:

- Worktree: `/Users/griiettner/Projects/grittflow/gritt-cli-tkt-0017`
- Branch: `tkt-0017-02-mcp` from `main` at merge commit `99f9132`
- Commits: `2afc880` (core contracts and the policy default), `faad7a1`
  (provider transport), `14f940d` (the runtime and its trust store),
  `2b1d06f` (agent dispatch), `dd80b59` (tests and fixtures), `288bca6`
  (binary wiring and docs), plus the ticket-artifact commit carrying this
  report.

What landed:

- `crates/gritt-core/src/mcp.rs`: `parse_mcp_config` and `parse_mcp_value`
  take the file text plus a snapshot of the launch environment and return
  `McpConfig` with one `McpEntry` per configured key. `McpTransport`
  (`Stdio { command, args, env }`, `Http { url, headers }`) does not
  implement `Serialize` and redacts values in `Debug`, because interpolation
  may have put a credential in it. `McpConfigError` names fields and
  variables only. `interpolate` handles `${VAR}` and `${VAR:-default}` with
  no shell, so a default is literal text. `fingerprint` is FNV-1a over
  canonical JSON of the raw entry. Plus `McpServerState`,
  `McpServerSnapshot`, `McpToolRef`, `TrustRecord`, `TrustDecision`,
  `McpRuntimeSettings`, `LATEST_PROTOCOL_VERSION`,
  `SUPPORTED_PROTOCOL_VERSIONS`, and `DISPATCH_TOOL_PATTERN`.
- `crates/gritt-core/src/policy.rs`: `workspace_defaults()` gained an
  `mcp__*` / `*` rule with outcome `ask`, after `network` and before the
  catch-all deny.
- `crates/gritt-provider/src/transport.rs`: `HttpResponse.headers`
  (lower-cased) with a `header()` accessor, `Method::Delete` and
  `HttpRequest::delete`, and `FixtureResponse::header`. No request shape and
  no adapter behavior changed.
- `crates/gritt-harness/src/mcp/`: `jsonrpc` (framing and classification),
  `connection` (the shared command-channel handle that assigns request ids),
  `stdio` (child process, minimal environment, stderr tail, specified
  shutdown escalation), `http` (Streamable HTTP over the provider transport
  and SSE parser), `registry` (collision-safe names and result rendering),
  `trust` (the decision seam), and `mod` (`McpRuntime`).
- `crates/gritt-harness/src/store/mcp_trust.{rs,sql}` and migration
  `0004_mcp_trust`: `Store::mcp_trust`, `set_mcp_trust`, `clear_mcp_trust`,
  and `StoreTrustStore`.
- `crates/gritt-harness/src/agent.rs`: `NativeAgent.mcp` and `mcp_tools`,
  `refresh_mcp_tools()` at the start of every turn, MCP definitions added to
  the coding-phase request, `resource_for` routing a dispatch name to
  `mcp:<server>/<tool>`, `execute_call` routing execution, and
  `AgentBuilder.mcp` with `with_mcp`.
- `crates/gritt/src/main.rs`: one runtime per workspace wired into the
  builder, `start_mcp` and `stop_mcp` around print, REPL, and full-screen
  modes, and the `gritt mcp list|trust|forget` command.
- `docs/tools-and-permissions.md`: a new "MCP server tools" section.

## Key Decisions

- **Pinned revisions.** Gritt offers `2025-06-18` in `initialize` and accepts
  `2025-06-18`, `2025-03-26`, or `2024-11-05` in the answer; anything else
  disconnects with a stated reason. The tool surface (`tools/list`
  pagination, `tools/call`, `notifications/tools/list_changed`) is identical
  across those three, so accepting the older two costs nothing and keeps
  older servers usable. Newer revisions exist upstream (`2025-11-25`, and
  `2026-07-28`, which replaces handshake negotiation with a per-request
  `_meta` key and adds `server/discover`); supporting them is a follow-up,
  and until then a server that insists on one is refused, not guessed at.
- **No client capabilities are advertised.** `initialize` sends
  `capabilities: {}`. Roots, sampling, and elicitation are deferred by the
  feature plan, and the specification forbids using a capability that was not
  negotiated, so advertising one Gritt cannot serve would be a lie a server
  could act on.
- **No new dependency.** stdio is `tokio::process` plus `serde_json`;
  Streamable HTTP reuses `gritt-provider`'s `HttpTransport` and `SseParser`.
  That matches ADR-012's existing hand-rolled server, and the only change
  needed was two additive fields on the shared transport.
  `modelcontextprotocol/rust-sdk` was not evaluated on crates.io because
  nothing required it.
- **Trust is keyed on `(workspace, server name, fingerprint)`.** The
  fingerprint is computed from the raw entry before interpolation, so no
  environment value can reach the stored record and any edit to the
  definition invalidates the approval. Renaming an entry is a new key too,
  which is the safe direction.
- **Denied is its own state.** The plan lists awaiting approval, starting,
  ready, failed, stopped, invalid, and unsupported transport. A refusal is
  none of those, and calling it `stopped` would mislead, so
  `McpServerState::Denied` was added.
- **The default policy outcome for MCP is `ask`.** Without a rule the
  workspace catch-all would deny every MCP call, making the plan's exit
  criterion unreachable without hand-written config. MCP tools run outside
  Gritt's process in a server the workspace configured, the same risk profile
  as `shell` and `network`, both of which ask. Server annotations, including
  `readOnlyHint`, are kept for display and never consulted for a decision.
- **The resource is `mcp:<server>/<tool>`.** A rule can target one server
  (`tool = "mcp__docs__*"`) or one tool (`resource = "mcp:docs/search"`) with
  the existing wildcard engine, no new matching code.
- **Dispatch names are `mcp__<server>__<tool>`**, sanitized to
  `[A-Za-z0-9_-]` and bounded at 64 bytes, with a numbered suffix if
  sanitizing or truncation ever collides. The server name is inside the name,
  so duplicate tool names across servers are distinct by construction.
- **The child environment is an allowlist, not the parent's.** `env_clear()`
  then a fixed list (`PATH`, `HOME`, `TMPDIR`, `LC_*`, the Windows
  essentials, and so on) plus the entry's declared variables. A provider key
  configured for Gritt cannot reach a server by accident; passing one is
  possible only by declaring it.
- **`initialize` is never cancelled.** The specification forbids it, so the
  handshake uses a timeout-only path (`request_uncancellable`) and a breach
  ends the connection instead of sending a notification the server must
  ignore.
- **A per-turn tool snapshot.** `refresh_mcp_tools()` runs once when a turn
  starts. A `tools/list_changed` arriving mid-turn cannot change the set the
  model was shown, which would otherwise let a model call a tool that was
  never advertised.
- **Reload validates before it replaces.** `read_config()` parses first; a
  parse failure returns an error and touches no running server. An entry
  whose fingerprint is unchanged and whose connection is ready keeps that
  connection, so changing sessions in a workspace reuses it.

## Alternatives Considered

- **An in-process fake server for the tests.** Rejected: the stdio path is a
  real child process, and testing it without one would test a different code
  path. Driving the integration test binary as its own server was also
  rejected because libtest writes to stdout, which the transport must treat
  as a protocol violation.
- **A `[[bin]]` behind a cargo feature.** Rejected: `cargo test -p
  gritt-harness` would not enable it, so `CARGO_BIN_EXE_*` would not exist
  and the tests would not run by default.
- **Leniently skipping non-JSON lines on stdout.** Rejected: the transport
  specification says a server must not write anything else there, and quietly
  tolerating it would hide a real fault.
- **Opening the optional server-initiated `GET` stream.** Rejected for now:
  Gritt advertises no capability requiring server-to-client requests, and
  `tools/list_changed` also arrives on the POST streams.
- **Restarting a failed server automatically.** Rejected: the plan says
  restart is an explicit action in this release.
- **Retrying a call after a disconnect.** Rejected by the plan and by the
  specification's warning that a remote server may already have completed the
  side effect.
- **A `Resource::Mcp` variant.** Rejected: `Resource::Other` with an `mcp:`
  prefix needed no change to the policy engine.

## Assumptions

- The `mcp__*` policy default is the reading the plan's "dispatch calls
  through the harness permission engine" most directly supports. Leaving MCP
  under the catch-all deny would have meant no MCP tool runs until a user
  writes a rule by hand, and the plan's exit criterion could not be shown.
- `gritt mcp list|trust|forget` was added because the interactive prompt is
  later UI work and, without some way to approve, the trust boundary would
  make MCP unusable end to end in this step. It is a thin wrapper over the
  typed decision API and is expected to be joined, not replaced, by `/mcp`
  in TKT-0019.
- `type` is read first and `transport` accepted as an alias; `http`,
  `streamable-http`, and `streamable_http` all mean Streamable HTTP. With no
  `type`, a `url` means HTTP and a `command` means stdio, which is the shape
  every existing `.mcp.json` uses.
- A credential-looking field must be exactly `${VAR}`: no default and no
  surrounding text. `${TOKEN:-fallback}` is refused because the fallback is a
  literal credential. This is stricter than the plan's wording and can be
  relaxed later without a data migration.
- The child environment allowlist may be too narrow for some server (a Node
  server wanting `NODE_OPTIONS`, say). It is one constant in `mcp/stdio.rs`,
  and the entry's own `env` block is the escape hatch. The live check against
  this repository's real server passed with it.
- `McpServerState::Denied` is an addition to the plan's listed states; see
  the decision above.
- The runtime is created for every mode but launches nothing until a
  definition is approved, so adding it costs an unapproved workspace only a
  file read.

## Edge Cases and Failures

- **The live smoke check timed out at first.** The real `gritt` server
  indexes 285 memory files on its first start in a fresh worktree, which took
  longer than the 30 s initialization deadline. It was not the minimal
  environment and not the Turso lock: a manual handshake confirmed the server
  answers correctly, its startup message goes to stderr, and its stdout
  carries only JSON-RPC. Re-run after the index existed, the check reports
  `ready`, `protocol=2025-06-18`, 3 tools. The deadline is configurable,
  which is what that case is for.
- **`.cargo/config.toml` sets `artifact-dir = "."`.** Every built target
  lands at the repository root, so `cargo build` overwrote the committed
  `gritt` binary with a debug build. It was restored with `git checkout --
  gritt` before any commit. Adding a fixture binary made this worse, so
  `/gritt-mcp-fixture` and `/libgritt_*.rlib` are now gitignored. See the
  follow-up.
- Two existing tests pinned the migration count (`3/3` in `gritt`'s
  `e2e.rs`, and a literal list in the store's unit tests). The e2e assertion
  now expects `4/4` and names `0004_mcp_trust`; the store tests compare
  against `MIGRATIONS` itself so the next migration does not break them
  again.
- A response arriving for a request the caller stopped waiting for is
  discarded by the transport task, as the specification requires. The pending
  entry for a request that is never answered lives until the connection ends,
  which bounds it.
- A server that repeats a `tools/list` cursor fails with a stated reason
  instead of paginating forever, and `max_list_pages` bounds it anyway.
- A single stdout line over 8 MiB is treated as a protocol violation.
- An `http` entry in a build with no HTTP transport reports that plainly
  rather than reporting a connection failure it never attempted.
- `AgentBuilder` gained a required `mcp` field, so its five other
  construction sites were updated to `mcp: None`.

## Validation

All run from the worktree on the final tree:

- `cargo fmt --all --check`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass. One finding
  during iteration (`type_complexity` on a test tuple) was fixed with a named
  type alias. The `nix v0.28.0` future-incompatibility note is pre-existing.
- `cargo test -p gritt-core -p gritt-harness -p gritt-provider`: pass.
  gritt-core 33 (11 new); gritt-harness 64 unit (16 new), 22 mcp_runtime, 4
  mcp_native_session, 1 mcp_live_smoke (skips without the variable), 23
  native_session, 12 session_draft, 8 connector_session; gritt-provider 25
  unit, 26 contract, 6 models_cache, 1 sse_tcp, 3 live (skips).
- `cargo test --workspace`: pass. Adds gritt 14 unit, 11 e2e, 4 tui_pty;
  gritt-connector 5 unit, 25 connectors, 3 live (skips).
- `GRITT_LIVE_MCP_TESTS=1 cargo test -p gritt-harness --test mcp_live_smoke
  -- --nocapture`: pass, with the result below. The check now fails if a
  server whose executable is present, on a supported transport, does not
  become ready; it previously passed when every available server failed.
- Manual: `gritt mcp list` showed `gritt stdio awaiting approval`; `gritt mcp
  trust gritt` then showed `ready` with `mcp__gritt__search_local_memory`,
  `mcp__gritt__read_local_memory`, and `mcp__gritt__delegate_run`. No tool was
  called and the throwaway database was deleted afterwards.

### Smoke check per `.mcp.json` entry

The workspace file declares exactly one entry.

| Entry | Executable | Result |
| --- | --- | --- |
| `gritt` (stdio, `.agents/gritt-agent mcp serve`) | available | ready, protocol `2025-06-18`, 3 tools listed |

No tool was called, `delegate_run` included. That is the whole of what the
check guarantees: Gritt issues no `tools/call`. It does not and cannot
promise a server writes nothing during its own startup — this one indexes its
memory database when it starts, as the edge case below records.

That entry also holds an exclusive lock on the worktree's memory database, so
only one instance can run at a time and the check fails while another
`gritt-agent mcp serve` holds it, including one started by a reviewer in the
same worktree. That is an environment collision rather than a defect; rerun
once the other process has exited.

New tests, one per stated behavior. Core: a missing `mcpServers` object,
malformed JSON that does not echo its content, an inferred stdio entry with
verbatim arguments, isolation of a bad entry beside healthy ones,
interpolation with defaults and a missing variable, a missing variable
disabling only its own server, refused literal credentials in `env` and in
headers, resolved credential references with redaction, the fingerprint
tracking the definition but not key order, the pinned revisions, and tagged
state serialization. Harness units: JSON-RPC classification and one-line
framing, registry collisions, sanitizing and bounding, server removal, a
schema-less tool, text and structured results, an execution error,
unsupported blocks, payload bounding that does not corrupt JSON, an empty
result, trust invalidation, a remembered denial, program resolution, and a
credential-free inherited environment. Integration: the 22 runtime cases and
the 4 native-session cases listed in the commit messages.

## Completion Gate

- **Acceptance**: yes, after the three rounds of review fixes recorded in the
  update below.
  Every configured entry has a state and a non-empty explanation; supported
  servers initialize and list tools; a turn calls the exact tool identity it
  was authorized for; a denied call never reaches a server, proven by a
  marker the fixture writes only when a call arrives, and revoking a
  server's trust removes its tools and closes it; a failed server leaves
  healthy ones usable; cancellation and shutdown leave no child process,
  including descendants that outlive their parent, checked by pid on unix and
  reported as skipped elsewhere; and a credential is redacted out of server
  metadata, errors, schemas, and results, proven by fixtures that echo their
  own configured credential on both transports.
- **Scope**: yes. No Ratatui home, composer, palette, sidebar, picker, or
  benchmark UI. No hard-coded server name anywhere; every fixture uses an
  invented one. No provider wire format, event model shape, connector
  authority, or secret policy changed. No dependency added. Two additive
  fields on the shared HTTP transport and one new policy default are the
  durable contract changes; both are noted below.
- **Validation**: every check above passed; nothing was skipped except the
  pre-existing live provider tests, which need keys. The final figures after
  round 3 are 324 workspace tests over three consecutive runs.
- **Security and safety**: the change adds process launching and network
  access, both gated. Reading the file authorizes nothing; a definition runs
  only after an explicit approval keyed to its fingerprint. Children get an
  allowlisted environment, the workspace as cwd, a verbatim argument array,
  and no shell. Credential-looking fields must be references, and a refusal
  never echoes the value. Every call passes the permission engine, which no
  path bypasses; annotations never grant permission. Snapshots and errors
  carry names and reasons, never values. Shutdown escalates so no owned
  process survives. Payload size is bounded. A call is never replayed after a
  disconnect.
- **Regression risk**: `PolicyConfig::workspace_defaults()` has one more
  rule, so a caller counting rules by index would shift; the rule is additive
  and matches only `mcp__*` names, which nothing else produces.
  `HttpResponse` gained a field, which breaks any external construction of
  it; the two in-tree constructors were updated. `AgentBuilder` gained a
  field for the same reason. Migration `0004_mcp_trust` only creates a table.
  Sessions with no `.mcp.json` behave exactly as before: the runtime is
  created, finds nothing, and starts nothing.
- **Follow-up**: see below.
- **Assumptions**: recorded above.

## Follow-up

- **ADR follow-up.** Three durable changes want recording when the chain
  closes: the `mcp__*` permission default and the `mcp:<server>/<tool>`
  resource form extend ADR-009's tool and permission model; the trust record
  and its fingerprint rule extend ADR-008's configuration and secret
  boundary; and the pinned protocol revisions extend ADR-012, which currently
  assumes Gritt is only an MCP *server*.
- **Newer protocol revisions.** `2025-11-25` and `2026-07-28` are published
  upstream, and `2026-07-28` changes negotiation itself. A later ticket
  should decide whether to support it; today such a server is refused with a
  clear reason rather than guessed at.
- **`.cargo/config.toml` sets `artifact-dir = "."`** (pre-existing). Any
  `cargo build` overwrites the committed `gritt` binary at the repository
  root with a debug build, and scatters rlibs beside it. This cost one
  restore during this ticket and the TKT-0016 report records the same trap.
  Worth its own ticket: either drop the setting and install the release
  binary deliberately, or move the artifact directory out of the repository.
- **First-start latency.** A server that does heavy work before answering
  `initialize`, like this repository's own on a fresh worktree, hits the 30 s
  default. The deadline is configurable but is not yet exposed in
  `config.toml`; wiring `McpRuntimeSettings` to the config file is a small
  follow-up.
- **The interactive approval prompt** is TKT-0019's: `/mcp` should show the
  snapshots, offer restart and reload, and ask for first-use approval through
  the same typed API `gritt mcp trust` uses.
- **Connector-owned MCP state.** The plan asks the sidebar to label MCP state
  by owner when connector sessions are active. Nothing here claims otherwise,
  and the runtime is only started for native sessions, but the labelling
  itself belongs to the sidebar step.
- **Lifecycle event delivery.** There is no subscription for MCP state
  changes, so TKT-0019 has to poll `snapshots()`. Defining that delivery is
  the cheapest remaining improvement for the TUI step.
- **HTTP resumability.** `Last-Event-ID` replay and the server-initiated
  `GET` stream are not implemented. Neither is needed for tool dispatch; both
  would matter for long-running server-initiated work.

## Updates

- [2026-09-04 review fixes](updates/2026-09-04-review-fixes.md): credential
  redaction at the runtime boundary, trust enforced on restart and revoked on
  denial, CLI cleanup on cancellation and startup errors, generation-checked
  lifecycle results, frozen tool identity on dispatch, bounded writes and
  input, process-group cleanup past the direct child, owned HTTP request
  tasks, the HTTP initialization barrier and server-request answers, and
  backend-driven MCP startup. Round 2, appended to the same file: credentials
  scoped to a connection so a token rotation cannot expose a retained one,
  shutdown owning in-flight initialization, signal cleanup on every launching
  CLI path, generation-checked trust reads, cancellable queue admission, owned
  HTTP notification and reply tasks, compliant server-request replies, input
  limits without framing bypasses, and backend prediction aligned with
  `ControlPlane::open`. Round 3, also appended there: continuous launch
  ownership through shutdown, bearer-token redaction, serialized trust
  decisions, order-independent cancellation, bounded auxiliary HTTP
  admission, cross-chunk SSE framing, and opening MCP on first entry into a
  native session.
