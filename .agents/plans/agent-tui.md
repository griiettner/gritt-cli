# Real agent TUI plan

Status: proposal. Full-screen interaction, OpenCode-inspired design, and a
Crush-inspired information sidebar, workspace `.mcp.json` support, and a
responsive interface under load are user requirements. The detailed behaviors below are proposed implementation
choices, not new ADRs. This revision changes the plan only.

## Goal

Turn the current Ratatui transcript view into a first-class agent workspace.
The user should be able to start a session, choose a provider profile, choose a
model from that profile's catalog, choose reasoning effort, then run planning
and coding turns with the existing streaming, tool, approval, diff, cancellation,
resume, and connector behavior.

The TUI remains a client of the existing control plane. It must not grow its
own provider HTTP clients, key handling, model resolution rules, or session
storage.

## Experience and visual direction

Use the supplied OpenCode home screenshot as the primary visual reference.
Its useful qualities are generous space, a centered identity, a wide composer
with a subtle background and accent edge, model information beside the input,
and a small number of discoverable shortcuts. The supplied Codex, Claude Code,
and Grok screenshots are secondary references for readable status and input.
Use Gritt's name and visual identity rather than copying another application's
logo or promotional copy.

The full-screen view uses the existing Ratatui/Crossterm stack from ADR-009.
The application owns the terminal viewport, with two main layouts:

- Home: a centered Gritt wordmark and composer, constrained to roughly 90
  terminal columns on wide screens. Show the working directory, current
  connection, model, effort, and Plan/Code phase. With no connection, show
  “Use /connect to get started.” Keep the composer usable before setup and
  preserve its draft throughout connection dialogs.
- Conversation: a small session header, a scrollable transcript filling the
  main column, a right information sidebar, and the composer anchored above a compact footer. The
  wordmark disappears after the first submission. Use whitespace and role
  styling rather than a border around every message. Tool calls appear as
  compact rows that expand to show output; write approvals open a diff view.

Use semantic theme tokens for background, raised surfaces, text, muted text,
accent, selection, success, and error. Provide dark and light palettes and
honor `NO_COLOR`. Do not hard-code screenshot colors across widgets. On narrow
terminals, reduce margins and collapse secondary status before sacrificing
input or transcript space. Hide the large wordmark on short terminals.

## Session information sidebar

Use the supplied Crush conversation screenshot as the reference for a quiet,
right-aligned information column. Keep the home screen centered; the sidebar
appears in the conversation layout. A small Gritt identity and version leave
most of its space for useful session information.

Show information in this order:

| Section | Content and source |
| --- | --- |
| Session | Name, workspace, phase, and current activity from session state |
| Model | Model, provider or installed agent, and effective effort from the active driver |
| Usage | Reported input/output tokens; context occupancy only when current context size and model limit are both known |
| Cost | Estimated session cost only when matching usage and pricing are available; label the estimate and its scope |
| Changed files | Workspace changes, with paths and change status; selecting a file opens a read-only diff |
| Integrations | Live MCP server state and tool counts from the harness; LSP and skill status when their runtimes exist |

Unknown values show as unavailable, never zero. Cumulative session token usage
is not context occupancy. Do not derive a context percentage from it. Billing
credits or subscription balances require a real integration and are outside
this release. A discovered skill file does not prove the running agent loaded
it; distinguish available skills from confirmed active skills when supported.

The changed-files section reports workspace state rather than claiming every
change was made by Gritt. Capture a baseline when opening the workspace and
label pre-existing changes. Collect changes through a harness service, with
read-only Git status/diff where available; refresh after tools and on explicit
request without blocking rendering. In non-Git workspaces, show files observed
in successful native write events and label this as partial coverage. External
agent changes require workspace observation or reported events. Never infer
them from prose in its response.

Proposed layout defaults: a 30-column sidebar and 2-column gutter when the
terminal has at least 110 columns, leaving 78 columns for the conversation.
Below that width, collapse it automatically. `/sidebar` toggles it on wide
screens and opens the same information in a drawer on narrow screens. Closing
the drawer restores prior focus and scroll. Keep essential model and activity
information near the composer when the sidebar is hidden. Long sidebar
contents scroll independently; Tab reaches it without changing composer text.

Sections use small headings, spacing, and subtle separators. Status includes
text as well as color. Hide unimplemented integration sections entirely;
“None” is reserved for a supported inventory that was actually checked and
found empty. MCP support is part of this feature. LSP and a skill execution
engine remain future integrations.

## Commands and connection flow

Typing `/` at the start of the composer opens filtered command suggestions.
Ctrl-P opens the same command registry as a searchable palette. Commands are
handled locally and do not become provider prompts. Unknown commands show a
local error and retain the input; `//` escapes a literal leading slash.
Pasted multiline content stays text and never runs a command on paste.

| Command | Proposed Gritt behavior |
| --- | --- |
| `/connect` | Search AI providers and installed agents, inspect availability, and connect or select one |
| `/models`, `/model` | Search models for the selected provider and choose one |
| `/effort` | Choose an effort supported by the selected model or inspect why it is unavailable |
| `/plan`, `/code` | Select the existing planning or coding phase |
| `/sessions`, `/resume` | Search and resume sessions for this workspace |
| `/new` | Return to a fresh home draft without deleting the previous session |
| `/details` | Expand or collapse tool output |
| `/sidebar` | Toggle session information or open its drawer on narrow terminals |
| `/mcp` | Inspect configured servers, discovered tools, failures, and restart/reload actions |
| `/help` | Show commands, keyboard help, and current capability limitations |
| `/quit` | Exit through cancellation and terminal cleanup when needed |

The connection dialog has “AI providers” and “Installed agents” groups. Both
share search and selection behavior, but each row identifies its type and
availability. Provider profiles show credential availability and catalog state.
Installed agents show installed/auth status and supported controls. Selection
does not install software or silently launch an authentication process.

For a provider, selecting a configured profile proceeds to its model picker.
Offer setup for the supported provider presets and a custom endpoint. New
profiles require a name, protocol, endpoint, and credential reference. Persist
non-secret profile fields through the binary's configuration service to the
user config by default; project configuration requires an explicit destination
choice. Preserve unrelated configuration and explain precedence conflicts.
Secret entry is masked, never goes through transcript or command history, and
writes only through the existing keychain service. If keychain storage is
unavailable, explain environment setup and allow returning to the draft.
Opening the dialog must work without a configured default profile or model.

For an installed agent, show a detail screen before selection, including any
authentication action needed. Gritt continues to respect the agent's own
permission authority. Model and effort controls appear only when the connector
exposes a tested capability; otherwise label them “Managed by agent.” This
release keeps those external controls informational.

Model lists support filtering by display name and id, with profile labels,
the current selection, and visible fresh/stale/error states. Loading is
asynchronous and cancelable. Discard late results for a previously selected
profile. A failed refresh retains the cached list and current draft. Effort
defaults to “Model default”; only validated explicit choices are offered, and
reasoning-summary visibility remains a separate presentation preference.

## Interaction quality

Enter submits, Ctrl-J inserts a newline, and Shift-Enter works where the
terminal reports it distinctly. Tab moves focus or completes a highlighted
suggestion; ordinary typing filters a picker, including the letters j and k.
Escape closes the top overlay first, then cancels a running turn when no
overlay is open. Approvals take priority and a canceled approval cannot be
answered by a late key.

The composer supports bracketed paste, multiline cursor movement, selection,
delete and word navigation, and Unicode display width. Drafts survive failed
submission and dialog cancellation. Text selection and copying have a
keyboard-accessible path; mouse scrolling and tool expansion supplement it.
Do not bind Ctrl-M independently of Enter because terminals can encode them
identically.

Streaming follows the bottom only while the reader is already there. Scrolling
up holds the viewport and shows a new-output indicator. Return to latest is
explicit. Cache wrapped transcript layout and batch updates so long sessions
do not require rebuilding all text on every frame. Terminal escape sequences
in tool or model output are rendered safely rather than executed.

## MCP belongs to the harness

Read `<workspace>/.mcp.json` directly. Users must not translate it into another
application's configuration format. Enumerate every entry in `mcpServers`,
regardless of its name, vendor, purpose, or order. No server-name allowlist,
hard-coded local-memory integration, or fixed server count is permitted.
Each entry goes through the same validation, approval, transport, discovery,
and lifecycle handling. The current workspace declares
`gritt-local-memory` and `turso-local-memory` under `mcpServers`, each with
`command` and `args` and no explicit transport type. Support those entries as
stdio servers; they are compatibility examples, not the supported server set.
Resolve relative executable paths against the selected workspace,
launch with that workspace as cwd, and preserve the argument array without
shell interpolation. Resolve bare executable names through PATH.

The binary loads and validates configuration; a proposed
`crates/gritt-harness/src/mcp/` module owns connections and process lifecycle.
The TUI consumes snapshots and events from that module. The native agent's
tool dispatcher uses it in print and REPL modes too. MCP servers are tools and
context services, distinct from external-agent connectors.

Configuration contract for this release:

- Accept the `mcpServers` object with stdio `command`, `args`, and optional
  `env`; accept explicit `type: stdio`. Support `type: http` with a `url` and
  optional headers for Streamable HTTP. Unknown transports, including legacy
  standalone SSE, receive an explicit unsupported-transport message.
- Support `${VAR}` and `${VAR:-default}` references in environment and header
  values, resolved from the launch environment without executing shell code.
  Missing required variables disable the affected server with a safe message.
  Credential fields must reference environment/keychain values under ADR-008;
  reject embedded credentials without echoing them in errors.
- Missing files mean no configured servers. Invalid JSON produces a visible
  configuration error. Invalid individual entries are isolated; healthy servers
  remain usable. Do not silently rewrite `.mcp.json` or change its arguments.
- Account for every configured entry in `/mcp`, including entries awaiting
  approval, invalid entries, and unsupported transports. Never silently omit a
  server. Bound concurrent initialization and queue the rest without imposing
  a fixed limit on how many configured servers can be discovered.
- Reading a workspace file does not authorize executing its commands. Show
  first-use server launch/connection approval, remember trust for the exact
  workspace and server definition, and invalidate it when that definition
  changes. Pass only required runtime environment plus declared server variables;
  do not expose unrelated provider keys to every server.

Initialize each approved server asynchronously, negotiate its supported
protocol version and capabilities, then obtain its tools through paginated
discovery. Use a collision-safe registry mapping provider-valid tool names to
the original server/tool pair. Feed those schemas through the existing provider
adapters. Dispatch calls through the harness permission engine before MCP
execution and return results through the shared tool-call/result event model.
Do not trust server read-only annotations as permission grants. Planning mode
keeps the existing no-tools behavior until a separate phase-policy decision.

Snapshot the available tool set for each turn; apply tool-list notifications
between turns. Support text and structured tool results, report unsupported
content blocks explicitly, and bound payload sizes without silently corrupting
structured output. Advertise only implemented client capabilities; server
sampling, elicitation, and prompt/resource browsing are deferred.

The sidebar shows each server as awaiting approval, starting, ready, failed,
or stopped, with tool count and a safe error summary. `/mcp` opens details and
explicit restart/reload controls. Reload validates a new configuration before
replacing the active one and drains or cancels affected requests. Keep one
connection per server per workspace runtime, not one per model turn. Changing
sessions in the same workspace reuses healthy connections.

Use configurable initialization and call deadlines. Cancellation must stop
waiting immediately and send protocol cancellation where supported. A remote
server may still complete a side effect; report uncertainty and never replay
a tool call automatically after a disconnect. Shut down owned stdio processes
on application exit, escalating process termination after a bounded grace
period. Restart a failed server only through an explicit action in this release.

External agents continue to own their MCP clients. Gritt must not execute their
reported tool calls a second time or imply its `.mcp.json` controls their
configuration. Label sidebar MCP state by owner when connector sessions are
active; unknown agent-side state stays unknown.

Protocol reference: MCP's versioned
[lifecycle](https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle)
and [transports](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports)
describe capability negotiation, stdio, Streamable HTTP, timeouts, and shutdown.
The `.mcp.json` loader is Gritt's configuration compatibility contract, separate
from the wire protocol. Confirm and pin the supported protocol versions and
Rust client dependency, if used, before implementing the transport layer.

## Responsiveness is an acceptance requirement

The user's observation that Crush feels fast is a design target, not a measured
comparison. Measure Gritt on a recorded reference machine and terminal. Initial
proposed budgets for a release build are:

| Scenario | Target |
| --- | --- |
| Launch with existing config | First usable composer within 500 ms, independent of provider/MCP readiness |
| Typing, picker navigation, or scrolling | p95 input-to-frame latency below 50 ms |
| Sustained output | Smooth updates with a 30 fps render cap and p95 render work below 16 ms at 120x40 |
| Cancel input under load | Visible canceling state within 100 ms; cleanup proceeds asynchronously |
| Idle screen | No continuous full redraw; average CPU below 1% of one core over 30 seconds on the reference machine |

Use event-driven redraw with coalesced text and progress updates, cached layout,
and rendering limited to visible content. Put storage, Git scans, MCP decoding,
catalog loading, and credential resolution off the terminal event path. Use
bounded queues with backpressure; never drop approvals, tool results, or final
events. Persist ordered transcript data separately from render batching and
page old history rather than keeping every rendered line in memory.

Benchmark a 10,000-message transcript with 1,000 incoming text deltas per second,
several active MCP servers, one hung server, and a 1 MiB tool result. Record
hardware, terminal, build, fixture sizes, p50/p95 latency, CPU, and memory.
Verify queue lengths remain bounded and resident memory reaches a stable plateau
over a five-minute run after history paging is active. Keep a deterministic
timing harness for regression checks and a real-terminal run for presentation
latency; neither alone proves the other. Provider response latency is reported
separately from UI latency.

## Reference evidence

The visual observations above come from the supplied home screenshots and the
additional Crush conversation screenshot showing its information sidebar.
OpenCode documents `/connect` as provider setup, `/models` as
model selection, `/sessions` as session switching, and `/details` as a tool
detail toggle. Gritt's grouped connection picker is our proposed extension,
not a claim about OpenCode's installed-agent support.
See [OpenCode TUI documentation](https://opencode.ai/docs/tui/) and
[provider setup](https://opencode.ai/docs/providers/), consulted 2026-09-04.

[Crush](https://github.com/charmbracelet/crush) is a second interaction
reference. Its README documents provider setup through the model picker,
multiple project sessions, and model switching within a session while
preserving context. These are documented upstream behaviors, not claims
verified by running Crush locally.

Use the references for specific design questions:

| Reference | What Gritt should study |
| --- | --- |
| OpenCode, primary visual reference | Home composition, composer hierarchy, slash-command discovery, and focused dialogs |
| Crush, conversation sidebar and interaction reference | Session information hierarchy, model/usage display, changed files, provider onboarding, and continuity when changing models |

For Gritt, selecting an unconnected provider from `/models` should offer its
connection flow and return to the model selection afterward. Preserve search,
highlight, and composer draft across this round trip. `/connect` remains the
direct entry point and both paths use the same setup service.

Crush's documented model switching makes Gritt's initial session-pinning
restriction a visible product gap. Include that gap in the prototype review:
show the proposed new-session explanation instead of making a model choice
silently discard context. Do not describe seamless switching as delivered
until Gritt has a tested conversation migration contract.

Study upstream interaction behavior during implementation and record the
version used for comparison. Reimplement it in Gritt's existing Rust stack;
the reference does not require adopting another renderer or copying source.

## Vocabulary

| Term | Meaning | Excludes |
| --- | --- | --- |
| Provider profile | A configured native API endpoint and protocol, such as `openai` or `anthropic`. | A model id and an installed external CLI |
| Model | A provider-owned model id, resolved through the selected profile and its catalog. | A provider profile or a connector |
| Effort | The user-facing reasoning intensity for a native model turn. | Tool approval policy and planning/coding phase |
| Connector | An installed external agent process, such as Codex or Claude Code, with its own model and authority. | A native provider profile |
| Session draft | Uncommitted choices used to create or replace a session. | Persisted session state |

The current code uses `profile` for provider selection and `connector` for
external agents. The TUI should keep that distinction visible. A connector
selection must not pretend that Gritt controls its model or effort settings.

## Current evidence and gaps

- `crates/gritt-harness/src/tui/app.rs` has transcript, palette, sessions,
  approvals, diff, phase, and cancellation state, but no provider or model
  picker.
- `crates/gritt-harness/src/tui/run.rs` opens a driver before entering the
  event loop. There is no setup state for an interactive session that has not
  been created yet.
- `crates/gritt-harness/src/control.rs` already opens native and connector
  sessions through one control plane, but its native `profile` and `model`
  arguments are only supplied at open time.
- `crates/gritt-provider/src/models.rs` already exposes cached, fresh, and
  stale model lists through `ModelCatalog`.
- `crates/gritt-core/src/provider.rs` has `RequestOptions.reasoning: bool`,
  while Responses and Chat Completions currently hard-code medium effort.
  Anthropic Messages has no equivalent user-facing effort setting yet.
- `crates/gritt-core/src/session.rs` pins a native session to a profile and
  model, but has no persisted effort value.

These gaps point to a layered change: add a typed effort contract, add a
session draft and setup flow, then connect the existing catalog and control
plane to TUI reducers and overlays.

## Proposed behavior and tradeoffs

1. Provider and model are selected before a native session is created. Once a
   native session has transcript history, its provider and model stay pinned.
   The TUI offers `New session` when the user wants to change either. This
   preserves continuation state and avoids silently changing the meaning of a
   stored transcript.
2. Effort is a native session setting and can change between turns. It is
   stored with the native session and included in each `PromptRequest`.
3. Store `auto` for “Model default”; omit explicit effort from the request in
   that case. Offer `low`, `medium`, and `high` only where the adapter has a
   verified model/protocol mapping. Unknown support does not establish a safe
   mapping. Extra provider levels can be added through capability metadata
   without changing the picker. Effort is not equivalent to a token limit.
4. Connector sessions show connector identity and the connector's own status.
   The native provider/model/effort picker is disabled for them. Connector
   specific model or effort flags remain in connector configuration and are
   not copied into native request options.
5. Focused connection, model, and effort pickers share one selection component
   and session draft. Provider changes invalidate the model selection; model
   changes revalidate effort. This keeps common actions short while preserving
   validation in one control-plane service.
6. Model-list refresh uses the existing daily cache policy. The overlay shows
   fresh, stale, missing, and refresh-failed state without exposing keys or
   raw provider response bodies.
7. The initial interactive setup is lazy. If `gritt tui` has no named session,
   it opens with a session draft and creates the session only after the user
   submits the first prompt with valid connection choices. Existing `--profile`
   and `--model` flags seed the draft above configuration defaults; explicit
   interactive choices can then update that draft.

Session pinning is an initial implementation tradeoff, not a claimed best UX
or accepted architecture rule. When a model/provider change requires a new
session, explain that before applying it and retain the composer draft. Do not
silently reset conversation context. Seamless switching within a conversation
needs a separate decision about portable history and opaque continuation state.

## Implementation sequence

### 0. Reviewable terminal prototype

- In `crates/gritt-harness/src/tui/`, render home, conversation, command search,
  connection groups, model/effort selection, and an approval diff with fixed
  fixture data. Keep rendering usable with the real runtime in later stages.
- Add Ratatui TestBackend snapshots at 120x40, 80x24, and 60x20, in dark,
  light, and no-color modes. Include long model names, Unicode, and errors.
- Capture a real terminal walkthrough of home, `/connect`, `/models`,
  `/effort`, and a fixture conversation. Compare spacing, focus, and hierarchy
  with the supplied OpenCode reference. Fixtures must be identified as such.
- Exit: the intended design is visible and keyboard navigation works before
  network integration. A screenshot of the home screen alone is insufficient.
- Include the model-picker-to-provider-setup round trip inspired by Crush,
  plus the explanation shown when changing models needs a new session.

### 1. Core request and session contracts

- Add a provider-neutral `ReasoningEffort` enum in `gritt-core`, with serde
  names `auto`, `low`, `medium`, and `high`.
- Replace or extend `RequestOptions.reasoning` with an additive effort field.
  Preserve deserialization of old stored data and define the interaction
  between `reasoning = false` and `effort = auto` during migration.
- Add native session effort with a serde default so existing databases and
  serialized sessions continue to load.
- Extend `ModelCapabilities` only with data needed to state explicit effort
  support. Do not infer provider-specific levels from a model name.
- Add unit tests for JSON compatibility, session round trips, and invalid
  effort combinations.

### 2. Provider adapter mapping

- Map effort inside each adapter, never in the TUI:
  - Responses uses the provider's reasoning effort field.
  - Chat Completions uses the compatible reasoning effort field when the
    provider supports it.
  - Messages uses the provider's supported thinking or effort representation,
    or returns a typed unsupported-setting error when no safe mapping exists.
- Keep all wire differences behind the existing adapter boundary.
- Add deterministic request-body tests for every protocol and tests for
  explicit unsupported capability behavior.
- Keep usage and reasoning summary events unchanged.

### 3. Control-plane session drafts

- Add connection setup operations to `crates/gritt/src/config.rs` and
  `crates/gritt/src/keys.rs`, exposed through an injected setup interface in
  the harness. The TUI receives safe availability data and typed outcomes;
  config/keychain I/O stays with the binary as required by ADR-006 and ADR-008.
- Test masked input, cancellation, keychain failure, configuration precedence,
  and preservation of existing profiles. No API call requires a session row.

- Add a typed native `SessionDraft` or equivalent in `gritt-harness` containing
  optional name, profile, model, effort, and phase.
- Add a control-plane operation that validates and opens a draft, warming the
  selected profile's catalog before model resolution.
- Return catalog status and validation errors as structured values suitable for
  a TUI overlay. Do not make the TUI parse error strings.
- Add a native-agent setter for effort that persists the setting between turns.
- Keep resumed sessions pinned to their stored provider/model and load their
  stored effort.
- Add harness tests for new-session selection, provider changes invalidating a
  model, resumed-session pinning, stale catalog fallback, and connector
  picker restrictions.

### 3b. MCP runtime and tool dispatch

- Add `.mcp.json` parsing to the binary configuration layer and the proposed
  harness `mcp/` module; expose neutral server/tool state through core contracts
  without adding I/O dependencies to `gritt-core`.
- Integrate discovery with native tool selection, provider schema generation,
  policy decisions, cancellation, and the session event stream.
- Test with Rust fake stdio servers and a local HTTP fixture: handshake,
  pagination, name collisions, malformed output, missing env, tool errors,
  startup failure, timeout, cancellation, reload, and shutdown. Prove a denied
  call never reaches the server and a disconnect never replays a side effect.
- Use arbitrary server names and mixed configurations to prove enumeration is
  generic. Cover zero, one, and many entries; more entries than the startup
  concurrency limit; and adding, removing, or renaming entries on reload.
  Assert every entry has a visible state and every ready server contributes
  its discovered tools, including tools with names duplicated across servers.
- Exercise both workspace entries in an opt-in integration check when their
  executables are available. Report unavailable executables separately; fake
  fixtures do not establish live compatibility. Never mutate the memory DB as
  part of the smoke test.
- Exit: a native session can discover and invoke an approved MCP tool from
  `.mcp.json`, and one failed server leaves the UI and other tools usable.

### 4. TUI state and interaction

- Add a typed sidebar view model in `crates/gritt-harness/src/tui/`, populated
  from session events and harness workspace observations. Keep filesystem,
  pricing, and connector logic outside the renderer. Reject late updates after
  switching sessions and clear unavailable fields instead of retaining values
  from the previous driver.
- Extend the terminal prototype with populated, empty, unavailable, and long
  sidebar states. Test widths immediately above and below 110 columns, drawer
  focus restoration, independent scrolling, pre-existing file changes, and
  missing usage/pricing. Verify diffs are read-only and a resize preserves the
  conversation position and prompt draft.

- Add Home, Conversation, and modal picker state, including:
  - available provider profiles;
  - selected profile;
  - models for that profile;
  - catalog status and refresh error summary;
  - effort choices and selected effort;
  - focused field and selection indices;
  - pending session draft and validation notice.
- Add reducer actions for opening settings, moving through fields, changing
  profile/model/effort, applying a draft, creating a new session, and canceling
  the overlay.
- Keep approval and running-turn overlays modal. Do not permit settings changes
  while a turn or approval is active.
- Implement the command registry and composer behavior specified above; all
  palette, slash-command, and shortcut actions dispatch through that registry.
- Render the selected provider, model, and effort in the status bar. Show
  effort as `auto` when unset rather than hiding a default.
- Render stale and missing model catalogs as state, not as a blank picker.
- Add reducer and rendering tests for focus movement, dependent selection
  reset, cancel/apply, connector restrictions, and narrow terminals.

### 5. TUI runtime loop

- Let the event loop start without a driver for the lazy setup path.
- Run catalog loading and draft validation asynchronously through the control
  plane, keeping the draw loop responsive and showing a loading status.
- Replace the current driver after a successful new-session or resume action,
  reload history, and reset only view-local state. Preserve session events.
- On a failed apply, keep the draft and show the structured validation error.
- Ensure cancellation, panic restoration, and terminal teardown continue to
  work when the loop has no active driver.
- Add PTY integration coverage for first-run setup, native selection, resume,
  new session, and connector session display.
- Run the responsiveness fixture and record the budgets above with the sidebar
  visible, MCP initialization in progress, and one server unresponsive.

### 6. Documentation and follow-up cleanup

- Update `docs/terminal-modes.md` with the setup flow, key bindings, session
  pinning rule, and connector limitation.
- Update `docs/providers.md` with effort semantics, protocol differences, and
  the meaning of stale or unreported capabilities.
- Add a short section to `docs/getting-started.md` showing a first TUI run
  without putting a secret in the example.
- Record any accepted contract change as a follow-up ADR if it changes the
  durable provider or session model. Do not edit generated indexes directly.

## Acceptance criteria

- Full-screen home and conversation layouts follow the visual direction and
  pass the terminal prototype checks, including short and narrow viewports.
- The right sidebar shows live session/model information and known workspace
  changes; it collapses without losing access to information on narrow screens.
- Workspace `.mcp.json` loads directly and every `mcpServers` entry is accounted
  for without name-specific code. Approved servers using supported transports
  initialize and contribute tools to the native harness; all other entries
  retain a visible reason for not running. Sidebar state reflects reality.
- MCP launch and tool permissions, failure isolation, timeouts, and shutdown
  pass deterministic tests and the available live workspace smoke checks.
- The recorded responsiveness run meets the proposed budgets or identifies
  specific remaining performance gaps before implementation is called complete.
- Usage and cost distinguish reported, estimated, and unavailable values.
  Switching sessions cannot display stale data from the previous session.
- `/connect` works with no defaults configured and supports both provider
  setup and installed-agent selection without conflating their authority.
- Selecting an unconnected provider from `/models` returns to the picker after
  setup without losing its search or the prompt draft.
- Slash search and Ctrl-P reach the same actions; command text is never sent
  to the model accidentally. Multiline paste cannot trigger a command.
- The composer remains responsive while catalogs load or output streams;
  scrolling history does not jump back to the bottom on incoming text.
- Tool rows expand, approval diffs remain accessible, and cancel/quit leave
  no active child processes or broken terminal state.

- Starting `gritt tui` without a named session presents a usable setup flow.
- A user can select a configured provider profile, inspect its cached model
  list, select a model, select effort, and start a native session.
- Changing provider resets model selection and cannot open a mismatched model.
- Effort is visible, persisted with native sessions, sent through the provider
  adapter, and covered by request-body tests for all supported protocols.
- Resuming a native session restores its pinned provider, model, effort, phase,
  transcript, continuation state, and tool behavior.
- Starting a new session is the explicit path for changing provider or model.
- Connector sessions remain selectable and usable but do not expose native
  effort controls or imply control over connector permissions.
- Fresh, stale, missing, and failed model catalogs are understandable in the
  TUI and never reveal secrets.
- Existing print, REPL, approval, diff, cancellation, connector, and session
  flows continue to pass.

## Verification

Run from the repository root:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test --manifest-path .agents/cli/Cargo.toml
```

Focused checks should include:

- provider adapter contract and request-body tests;
- model cache and stale-list tests;
- harness session-draft and effort persistence tests;
- TUI reducer tests;
- `crates/gritt/tests/tui_pty.rs` and end-to-end setup/resume coverage;
- a manual run with one native profile and one connector, with no key values
  in captured output.

## Risks and boundaries

- Provider effort fields are not uniform. The adapter contract must reject an
  unsafe mapping rather than silently claim that all levels mean the same
  thing.
- Lazy session creation changes the TUI startup path and is the highest-risk
  integration point. Keep the existing eager path available for named-session
  and connector invocations until setup tests pass.
- Model catalogs can be unavailable. The UI must still allow a manually
  configured model only under the same unreported-capability rules as print
  mode, with a visible warning.
- This plan does not add a desktop frontend, background multi-agent task
  management, subagents, LSP, a skill execution engine, remote workspaces, or provider-specific model
  browsing beyond the existing catalog contract.

## Completion condition

The plan is complete when the TUI can create and resume native sessions with
explicit provider, model, and effort choices, the choices are represented in
the core contracts and adapter requests, connector behavior remains honest,
and the focused plus workspace validation passes without regressions.
