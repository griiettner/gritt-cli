---
id: TKT-0016
namespace: griiettner
title: Define model, effort, session-draft, and provider setup contracts
artifact: report
status: done
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
chain_role: worker
chain_parent: TKT-0015
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

# TKT-0016 Report: Define model, effort, session-draft, and provider setup contracts

## Summary

Worker 1 of the TKT-0015 chain. The provider-neutral effort contract, the
native session draft with typed validation, the driver-level effort setter,
and the provider setup interface now exist in code with tests. Later
workers can build the TUI pickers and reducers on typed values and never
parse an error string.

Chain facts:

- Worktree: `/Users/griiettner/Projects/grittflow/gritt-cli-tkt-0016`
- Branch: `tkt-0016-01-contracts` from `main` at `785af8e`
- Commits: `93ef33c` (core), `614e66c` (provider), `e0bf8ef` (harness and
  binary), plus the ticket-artifact commit that carries this report

What landed:

- `crates/gritt-core/src/provider.rs`: `ReasoningEffort` (`auto`, `low`,
  `medium`, `high`, with `Display`, `FromStr`, `ALL`, `EXPLICIT`);
  `RequestOptions.effort` with a serde default; `ReasoningIntent` and
  `RequestOptions::reasoning_intent()` documenting how the legacy
  `reasoning: Option<bool>` combines with `effort`;
  `ModelCapabilities.reasoning_efforts: Option<Vec<ReasoningEffort>>`
  (skipped when `None`, so serialized capabilities are byte-identical for
  every existing list); `EffortUnsupportedReason`, the typed refusal reason.
- `crates/gritt-core/src/session.rs`: `SessionKind::Native.effort` with
  `#[serde(default)]` and `SessionKind::effort()` (`None` for connectors).
- `crates/gritt-provider/src/effort.rs`: `effort_support(protocol,
  capabilities, effort)`, the single rule adapters and the harness share,
  and `unsupported_effort_error`, which puts the typed reason in the error
  diagnostic under `unsupported_effort`.
- `crates/gritt-provider/src/adapter.rs`: `check_capabilities` now derives
  the intent, refuses contradictory options as `Config`, and refuses an
  explicit level without a safe mapping as `UnsupportedCapability` before
  any request. The unreported-feature warning path is unchanged.
- Adapters: Responses sends `reasoning: {effort, summary: "auto"}` for an
  explicit level and `reasoning: {summary: "auto"}` for the legacy switch;
  Chat Completions sends `reasoning: {effort}` or `reasoning: {enabled:
  true}` only when the list reports reasoning support; Messages keeps the
  legacy `thinking` budget and never sees an explicit level. `auto` sends
  nothing on every protocol.
- `crates/gritt-harness/src/draft.rs`: `SessionDraft` (optional name,
  profile, model, effort, phase; `with_profile` clears the model on a
  change), `CatalogState` (fresh, stale, missing, refresh_failed,
  skipped), `DraftError` (missing_profile, unknown_profile, missing_model,
  model_outside_profile, model_resolution, effort_unsupported,
  session_pinned, connector_session, other_workspace), `DraftWarning`
  (model_not_in_catalog, deprecated_model_remapped), `ResolvedDraft`, and
  `DraftOutcome`. All serde-tagged.
- `crates/gritt-harness/src/control.rs`: `warm_catalog`, `catalog`,
  `profile_summaries`, `validate_draft`, `open_draft` (returns
  `DraftOpen::Opened { driver, catalog, warnings }` or `Rejected`), and
  `with_setup` / `setup()` for the injected writer.
- `crates/gritt-harness/src/driver.rs`: `Driver::effort()` and
  `Driver::set_effort()` returning `EffortOutcome` (`Applied`,
  `ManagedByConnector`, `Unsupported`). The connector session answers
  `ManagedByConnector`; the native agent validates against the adapter's
  protocol and capabilities, persists through
  `Store::set_native_effort`, and records a status event.
- `crates/gritt-harness/src/agent.rs`: `NativeAgent::effort`,
  `profile_and_model`, `set_effort`; the request carries the session's
  effort; `AgentBuilder::create_native` builds a session from resolved
  choices and `open` delegates to it.
- `crates/gritt-harness/src/setup.rs`: `ProviderSetup` trait
  (`save_profile`, `store_credential`), `ProfileSummary`,
  `CredentialState`, `ConfigDestination`, `ProfileSpecError`,
  `validate_profile_spec`, `ProfileSaveOutcome`, `CredentialStoreOutcome`,
  and `ReadOnlySetup` as the default.
- `crates/gritt/src/setup.rs`: `FileSetup` implements the writes: a
  profile's non-secret table into the user or project `config.toml`
  preserving other keys, with `shadowed_by` when the project file already
  defines the profile; a key into the keychain only, with a typed
  `KeychainUnavailable` naming the variable. Wired in `plane()`.

## Key Decisions

- Effort lives on `SessionKind::Native`, not `Session`. It is the one
  native-only setting that changes between turns; a connector session has
  no slot for it, which keeps the "managed by agent" case honest in the
  type rather than in a doc comment.
- Legacy `reasoning: Some(true)` with `effort: Auto` now means "reasoning
  on at the provider's default level" and no longer hard-codes `medium`.
  The plan named the hard-coded value as a gap; `auto` is defined as
  omitting the explicit level. `reasoning: Some(false)` plus an explicit
  level is a `Config` error because it is a caller contradiction, not a
  provider limit.
- Mapping verification is per protocol. Responses documents
  `reasoning.effort`, so a level is sent without list evidence. Chat
  Completions has no protocol-level field, so the OpenRouter form needs
  the list to report reasoning; an unreported model refuses an explicit
  level (`ReasoningNotReported`) while the legacy switch keeps its
  current silent omission. Messages refuses every explicit level by
  protocol: the Anthropic reference confirms `output_config.effort` is
  rejected by some models and `thinking.budget_tokens` by others, and the
  Anthropic list reports no capability flags, so any mapping would be a
  model-name guess.
- The harness reuses `effort_support` instead of restating the rule, so
  the draft validator and the adapter refuse identical cases.
- A resumed session keeps its pinned profile and model. A draft asking for
  a different one is `SessionPinned`; the draft's effort and phase are
  applied and persisted on resume. This is the plan's initial tradeoff,
  not an accepted UX rule.
- An unlisted model on a present catalog is a warning, not an error,
  matching print mode's unreported-capability rule.
- `ProviderSetup` only carries the writes. Availability (`ProfileSummary`,
  `CredentialState`) is computed in the harness from the builder's config
  and key provider, so the trait stays small and the default
  `ReadOnlySetup` reports every write as unavailable without a panic.

## Alternatives Considered

- `effort: Option<ReasoningEffort>` on `RequestOptions`. Rejected: `Auto`
  already means "unset", and a plain value avoids a three-state field.
- A new `ErrorKind` for unsupported effort. Rejected: `ErrorKind` is
  serialized into stored error events; the existing
  `UnsupportedCapability` kind with a typed diagnostic gives the same
  match surface without touching the event model.
- Storing effort but letting the next turn fail on an unsupported level.
  Rejected: the TUI needs the refusal at selection time, and the plan says
  only validated choices are offered.
- Mapping Messages effort to a thinking budget. Rejected: the plan says
  effort is not a token limit, and the representation is model-gated.
- A `Result<ResolvedDraft, DraftRejection>` for validation. Rejected in
  favor of a tagged `DraftOutcome` enum so the catalog state travels with
  both arms and serializes for an overlay.

## Assumptions

- `chain-check` expects benchmark evidence because task.md mentions
  benchmarks in its out-of-scope list. No benchmark applies to this
  contracts step; the TKT-0015 benchmark belongs to TKT-0020. Stated here
  so the checker's hint is answered truthfully.
- The catalog parsers do not populate `reasoning_efforts`. OpenRouter's
  `supported_parameters` names `reasoning` but not levels, and OpenAI and
  Anthropic lists report nothing. A different choice would have been to
  guess levels from a model name, which the ticket forbids.
- `FileSetup` writes with `toml::to_string_pretty` over a parsed table.
  Keys and values are preserved; comments in an existing file are not.
  The alternative was a new `toml_edit` dependency, which the ticket
  forbids.
- After `save_profile`, the running `AgentBuilder.config` is unchanged.
  The TUI worker decides whether to reload config or rebuild the plane.
- `NativeAgent::set_effort` uses `ProviderAdapter::capabilities`, which
  returns a default value for an unknown model. For `effort_support` a
  default value and `None` produce the same answer, so no precision is
  lost.
- The draft validator takes an id the selected profile's catalog lists
  as that model before any alias or qualified reading (review fix), then
  resolves with the profile as the alias hint. A qualified name or global
  alias that resolves elsewhere is `ModelOutsideProfile`.

## Edge Cases and Failures

- Old JSON without `effort` on `RequestOptions`, `SessionKind::Native`,
  adapter continuation state, and a raw `gritt_sessions.kind` row all load
  as `auto` (tests in core, provider contract, and the store).
- A contradictory stored continuation state (`reasoning: false` with an
  explicit level) cannot be produced by this code; if one appears, the
  adapters send nothing rather than failing a tool-result continuation.
- `Store::set_native_effort` on a connector session is a `Config` error.
- A draft that names a session in another workspace is `OtherWorkspace`,
  checked before the kind so the message never claims a pin.
- The stale fallback test uses an exhausted `FixtureTransport` as the
  failing refresh; the cache file is written three days old.
- Three clippy findings in the new test file (unused import, redundant
  field name, needless borrow) were fixed before the final run. Two stray
  `libgritt_*.rlib` files appeared at the repository root during
  iteration and were deleted, not committed.

## Validation

All run from the worktree on the final tree:

- `cargo fmt --all --check`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass (the
  `nix v0.28.0` future-incompatibility note is pre-existing).
- `cargo test -p gritt-core -p gritt-provider -p gritt-harness`: pass.
  gritt-core 22; gritt-provider 25 unit, 26 contract, 6 models_cache, 1
  sse_tcp, 3 live (skips); gritt-harness 48 unit, 8 connector_session, 23
  native_session, 8 session_draft.
- `cargo test --workspace`: pass. Adds gritt 14 unit (three new setup
  tests), 11 e2e, 4 tui_pty; gritt-connector 5 unit, 25 connectors, 3
  live (skips).
- `.agents/gritt-agent ticket sync`: synced.
- `.agents/gritt-agent ticket validate`: `tkt_validate ok (0 warnings)`.
- `.agents/gritt-agent ticket chain-check --ticket TKT-0016 --base main`:
  before this report, `tkt_chain_check ok (2 warning(s))`, both about the
  then-missing report.md; after it, `tkt_chain_check ok (0 warning(s))`
  with base `785af8ed24da`, head `e0bf8efab494`, and 22 changed files.

New tests, one per stated behavior: effort serde names and parsing; old
`RequestOptions` JSON; the legacy/effort combination table; capabilities
without levels round-tripping unchanged; old native sessions loading as
`auto`; explicit effort round trip; `effort_support` per protocol; a
Messages refusal with a typed diagnostic; per-protocol request bodies for
explicit effort; the legacy switch's wire shape; refusals for Messages,
unreported Chat Completions, a level not offered, a non-reasoning model,
and a contradictory request; effort surviving continuation and old state
restoring as `auto`; the store setter and a pre-effort row; draft builder
profile change; outcome serialization tags; profile spec validation; the
read-only setup; and the eight session_draft cases (new-session
selection through to the request body, provider change invalidating the
model, effort against protocol and catalog, resumed-session pinning with
stored effort, driver refusal of an unsupported level, stale catalog
fallback with an unlisted model, connector and other-workspace
restrictions, profile summaries without values). Binary: profile save
preserving the file and reporting shadowing, invalid spec and missing
user directory outcomes, failing keychain outcome without the value.

## Completion Gate

- Acceptance: yes. A later worker can build a draft, validate profile,
  model, and effort together, persist native effort with old data still
  loading, and send typed request options; every existing provider and
  session fixture still deserializes.
- Scope: yes. No MCP, rendering, slash commands, sidebar, benchmark, or
  docs beyond doc comments. No dependency added. Event model, connector
  authority, and secret policy unchanged.
- Validation: all checks above passed; nothing was skipped.
- Security and safety: no new network or file access outside the existing
  config paths and keychain; `FileSetup` writes only `SecretRef` tables
  and refuses to echo a malformed file; every outcome type is asserted
  free of key values; capability enforcement is stricter, not weaker.
- Regression risk: the legacy `reasoning: true` request no longer sends
  `effort: medium` on Responses and Chat Completions. Nothing in the
  harness sets that switch, so only external callers of the provider
  crate would notice; the contract tests pin the new shape. Adding a
  field to `SessionKind::Native` required `..` at four match sites, all
  compiled and tested.
- Follow-up: see below.
- Assumptions: recorded above.

## Follow-up

- ADR follow-up: `SessionKind::Native.effort`, `RequestOptions.effort`,
  and the per-protocol effort rule extend the ADR-007 provider and
  session contract. Record them in an ADR update when the chain closes.
- Anthropic's Models API now reports `capabilities` per model. A later
  ticket could parse it into `reasoning_efforts` and lift the Messages
  refusal for models that report `effort` support, without a name guess.
- Config reload after `save_profile` (or a rebuilt plane) is TKT-0019's
  call; `ProfileSaveOutcome::Saved` does not claim the running config
  changed.
- Comment preservation in `config.toml` needs `toml_edit`; decide with
  the docs step whether the loss matters.
- `DriverInfo` does not carry effort; the status bar should read
  `Driver::effort()` and show `auto` (TKT-0018).

## Updates

- [2026-09-04 review fixes](updates/2026-09-04-review-fixes.md): catalog
  ids with a profile-name prefix stay in the selected profile, a removed
  stored profile is a typed rejection, resume warms the catalog before
  resolution, and old-continuation tests cover every protocol.
