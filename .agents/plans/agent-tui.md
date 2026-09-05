# Real agent TUI plan

## Goal

Turn the current Ratatui transcript view into a first-class agent workspace.
The user should be able to start a session, choose a provider profile, choose a
model from that profile's catalog, choose reasoning effort, then run planning
and coding turns with the existing streaming, tool, approval, diff, cancellation,
resume, and connector behavior.

The TUI remains a client of the existing control plane. It must not grow its
own provider HTTP clients, key handling, model resolution rules, or session
storage.

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

## Decisions

1. Provider and model are selected before a native session is created. Once a
   native session has transcript history, its provider and model stay pinned.
   The TUI offers `New session` when the user wants to change either. This
   preserves continuation state and avoids silently changing the meaning of a
   stored transcript.
2. Effort is a native session setting and can change between turns. It is
   stored with the native session and included in each `PromptRequest`.
3. The initial effort choices are `auto`, `low`, `medium`, and `high`. The
   adapter maps them to each protocol. A provider or model that explicitly
   rejects a choice produces a clear validation error. When capability data is
   absent, the choice remains available but the first event carries the same
   capability warning pattern already used by the provider layer.
4. Connector sessions show connector identity and the connector's own status.
   The native provider/model/effort picker is disabled for them. Connector
   specific model or effort flags remain in connector configuration and are
   not copied into native request options.
5. The TUI uses a single settings overlay with dependent fields: provider,
   model, effort. Provider changes invalidate the model selection. Model
   changes revalidate effort choices. The overlay has explicit Apply, Cancel,
   and New session actions.
6. Model-list refresh uses the existing daily cache policy. The overlay shows
   fresh, stale, missing, and refresh-failed state without exposing keys or
   raw provider response bodies.
7. The initial interactive setup is lazy. If `gritt tui` has no named session,
   it opens with a session draft and creates the session only after the user
   applies a valid provider/model choice. Existing `--profile` and `--model`
   flags seed the draft and remain authoritative over defaults.

## Implementation sequence

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

### 4. TUI state and interaction

- Add a `Setup` or `Settings` view and plain-data state for:
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
- Add keyboard paths to the palette and document them. Prefer discoverable
  defaults such as `Ctrl-M` for settings and `n` in the session view for a new
  session, while retaining palette commands for accessibility.
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
  management, subagents, MCP, remote workspaces, or provider-specific model
  browsing beyond the existing catalog contract.

## Completion condition

The plan is complete when the TUI can create and resume native sessions with
explicit provider, model, and effort choices, the choices are represented in
the core contracts and adapter requests, connector behavior remains honest,
and the focused plus workspace validation passes without regressions.
