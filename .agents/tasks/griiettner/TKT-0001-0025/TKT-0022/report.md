---
id: TKT-0022
namespace: griiettner
title: Add provider failover and remember session preferences
artifact: report
status: done
owner: griiettner
created: 2026-09-06
updated: 2026-09-06
dependencies:
  - TKT-0019
areas:
  - provider
  - harness
  - cli
skills:
  - tkt
  - tkt-exec
  - dev-cli
  - dev-provider
  - dev-harness
  - review-ticket
---

# TKT-0022 Report: Add provider failover and remember session preferences

## Summary

A new native session now goes through one startup resolver in
`crates/gritt-harness/src/startup.rs`, whichever mode opens it. The resolver
picks the profile, model, and effort, tries the default profile and then the
profiles in the new `fallback_profiles` list, and reports every profile it
moved past by failure class with key values redacted. The last new session
to complete a turn records its profile, model, and effort per workspace in a
new `gritt_last_used` table, and later new sessions take those values for
whatever the flags and the draft leave open, ahead of the configured
defaults. Resumed sessions never enter the resolver and keep their stored
profile and model.

What landed:

- `Config.fallback_profiles` with load-time validation (`Config::fallback_order`,
  `Config::validate`): an unknown name or a repeat, the default included,
  fails the configuration. `GRITT_FALLBACK_PROFILES` is the environment form.
- `ProviderProfile.fallback_model`, optional and omitted when absent, so
  profiles written before it still load and save unchanged.
- `LastUsedNative` in `gritt-core`, migration `0005_last_used`, and
  `Store::last_used` / `Store::set_last_used` keyed by workspace root.
- The resolver: `StartupRequest`, `PrimarySource`, `FailureClass`,
  `SkippedProfile`, `StartupOutcome`, `AgentBuilder::resolve_startup`,
  `open_with`, and `start_native`. `ControlPlane::validate_new` delegates to
  it, and `ControlPlane::open_with` returns the driver with the startup
  notes and the model-list state.
- New draft types: `DraftError::NoUsableProfile`,
  `DraftWarning::ProfileSkipped`,
  `LastUsedApplied`, and `EffortReset`, plus `Display` for both enums and
  `DraftError::into_error` so print and REPL report the same typed
  rejection as the full-screen mode. `SessionDraft.explicit_profile` marks a
  profile the user chose as pinned.
- `ErrorKind::NoUsableProfile` for the aggregate failure.
- `gritt run|repl|tui --effort auto|low|medium|high`. Print and REPL print
  each note on stderr and, when the chain moved, the profile and model the
  session runs on. The full-screen mode shows the notes as transcript lines
  and a skipped profile as a notice, seeds the home screen from the
  resolver's own primary-profile rule, and shows the remembered effort in
  the status bar and the effort picker.
- `probe_models` in `gritt-provider`, a live list fetch that shares the
  cache-recording step with `load_models` and returns the raw failure.
  Model-list parse failures now carry the HTTP status in their diagnostic
  like error bodies do.
- Docs: `docs/providers.md` gained "Startup failover" and "Remembered
  choices", `docs/terminal-modes.md` lists `--effort` and the notes,
  `docs/config.example.toml` shows `fallback_profiles` and `fallback_model`,
  and ADR-008 records the decision.

## Key Decisions

**Remembered choices sit between the flags and the configuration.** For
each field the order is flag or picker, the remembered value, then the
configured default. The first pass put the configured default above the
remembered value, reading the scope's "configured defaults take
precedence" literally; that left the concept's problem ("new sessions
return to the configured model") unsolved for anyone with `default_model`
set, and the owner asked for it the other way. Now `default_profile` and
`default_model` are where a workspace starts and what it returns to when
the remembered profile disappears from the configuration; after the first
completed turn the last session's choices carry forward until a flag or a
picker names something else.

**Failover checks are strict only when there is somewhere to fall over to.**
With more than one candidate, every profile is probed live (`GET /models`,
bypassing the daily interval), a missing key skips the profile, and a
model the list lacks moves to the next profile. With one candidate,
whether pinned or the only one configured, the list is loaded the way it
always was, an unlisted model is a warning as before, and a missing key is
reported by the adapter on the first request as before. A configuration
without `fallback_profiles` therefore keeps its behaviour in full. The
first pass rejected a missing key at startup even for a single candidate;
that was the one behaviour change for existing setups and was removed on
the owner's request, along with the `DraftError::MissingCredentials`
variant it needed.

**A profile the user chose is pinned, and on the command line a model
name outranks the profile hint.** `--profile` and the `/connect` picker
pin. A qualified model name or global alias names its own profile and wins
over `--profile`, as `alias::resolve` always did, so `gritt run --profile
openrouter --model anthropic/claude-x` runs on `anthropic` in print and
REPL mode again; the first pass rejected it. The full-screen draft keeps
its own rule, covered by the existing `session_draft` tests: a model from
another profile under a picked profile is `ModelOutsideProfile`, so the
interface can clear the model. `StartupRequest.profile_is_hint` carries
the difference. The one guard on the command line is an id the hinted
profile's own list carries (OpenRouter's `openai/gpt-5-nano` next to an
`openai` profile), which stays that profile's model. On a cold cache in
print mode the list is not loaded before this check, which matches the
pre-ticket `resolve_model` behaviour and is noted under Follow-up.

**The last-used record is written on a completed turn of a new session.**
"Successful" means the provider accepted a request and the turn completed,
not that the session opened. The write happens once, again after `/effort`
changes the session's effort, and is best effort: the answer has already
been shown, so a storage failure is retried on the next completed turn
rather than turning a completed turn into an error. Resumed sessions never
write it.

**Model choice on a fallback profile.** The requested model is tried first,
then the profile's `fallback_model`, then the remembered model when it was
chosen on that profile, then the configured default model, and the first
one the profile's list contains wins.
Without a list, only an explicit `fallback_model` is accepted. The first
draft of this ordering put `fallback_model` ahead of the requested model;
review pointed out that this let a config value override a `--model` flag
that the fallback profile could serve, contradicting the precedence rule.

**Print and REPL no longer pre-warm the catalog.** The resolver loads or
probes the list for the profile it settles on, so the pre-warm was a second
fetch and, on a failover, a warning about the wrong profile. The stale or
missing list note now comes from the resolver's `CatalogState` on the opened
session. The full-screen eager path lost the same await; the lazy path keeps
its background warm for the `/models` picker.

## Alternatives Considered

- **Always probe live, fallbacks or not.** Rejected: it would make the daily
  refresh interval meaningless for every print-mode invocation and add a
  round trip to scripted use for users who never configured failover.
- **Strictness per candidate ("has a successor") instead of per chain.**
  Suggested by review. Rejected because the last candidate of a chain would
  then return its own typed error instead of contributing to the aggregate
  `NoUsableProfile`, which the acceptance criteria require.
- **Derive last-used from the sessions table instead of a new table.**
  Rejected: the sessions table does not know whether a turn completed, and
  the concept excludes resumed sessions, which a most-recent query would
  count.
- **Keep `SessionDraft` user-only and seed display state separately.** Would
  remove `explicit_profile` and `StartupRequest.pinned`. Left as a follow-up
  because the model picker and `/effort` read `draft.profile` today.
- **A typed `status` field on `gritt_core::Error`.** `classify` reads the
  status from the diagnostic, which is the convention `provider_error`
  already uses. A typed field is cleaner and is recorded as a follow-up.

## Assumptions

- Last-used preferences are per workspace, keyed by the canonical workspace
  root, because sessions are workspace-bound and the user data database is
  shared across projects. A global record would have leaked one project's
  choice into another.
- With no `default_profile`, `fallback_profiles` on its own is the chain.
- The remembered effort applies whatever the profile, and returns to the
  provider default with a note when the selected model cannot take it. A
  requested effort is refused as before.
- The full-screen mode seeds the draft's profile and model from the
  resolver's own order (remembered first, configured default second),
  unpinned, so the home screen names what the first prompt opens on. The
  remembered effort stays out of the draft and is shown from
  `App.remembered_effort`, so the resolver can return it to the provider
  default with a note where the model cannot take it.

## Edge Cases and Failures

- The end-to-end failover test first failed on its resume step because the
  stand-in provider thread ends after its last canned response, closing the
  port; the fixture now serves both turns from one provider.
- The last-used tests first read the record under the temporary directory
  path while the agent wrote it under the canonical root; the tests now use
  `AgentBuilder::workspace_root`.
- An unparseable model list was classified as a connection failure because
  `fetch_models` attached no status to the parse error; it now does.
- Review findings applied: the `LastUsedApplied` note fired for a profile
  taken from a qualified model name; a remembered profile removed from the
  configuration blocked the whole chain with `UnknownProfile`; a qualified
  `default_model` no longer selected its profile; `fallback_model` outranked
  a requested model the fallback profile could serve; the last-used write
  could fail a completed turn; the effort picker and status bar disagreed on
  the remembered effort; the TUI never let a qualified model name pin its
  profile; `resolve_model` and `StartupSelection::skipped` were dead;
  `probe_models` duplicated `load_models`; the keychain was asked twice per
  candidate; the pre-warm fetched the list a second time.
- Second pass, on the owner's request after the first report: the
  precedence flip, the single-candidate missing-key rule, and the model
  name over the profile hint, each with tests and docs updated. The shared-
  resolver test changed with the flip: after the draft path's completed turn
  on `backup`, the flag path now leads its chain with `backup` and moves on
  to `openrouter` when it is down.
- Review findings not applied, with reasons: per-candidate strictness (see
  Alternatives); a typed status on `Error` (follow-up); removing
  `explicit_profile` (follow-up); replacing the table with a sessions query
  (see Alternatives); `last_used` being read up to three times at launch
  (small, follow-up).
- `cargo test --workspace` has one failure that predates this ticket:
  `tui_pty::the_fixture_home_walkthrough_runs_by_keyboard_and_never_opens_a_session`
  waits for the effort picker label `Model default`, which the uncommitted
  execution-modes work already in the tree renamed to `Provider default` in
  `crates/gritt-harness/src/tui/app.rs` without updating
  `crates/gritt/tests/tui_pty.rs:522`. Not changed here; see Follow-up.

## Validation

Ran, in order, against the working tree that already carried the
uncommitted execution-modes change:

- `cargo fmt --all --check`: passes.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: passes.
- `cargo test --workspace --locked --no-fail-fast`: every target passes
  except the one pre-existing `tui_pty` case named above.
- `cargo build --release --locked`: passes.

The set was run again after the second pass with the same result.

Ticket verification, mapped to tests:

- Fallback ordering, duplicates, unknown profiles: `gritt-core` config
  tests; `gritt/src/config.rs` tests for the file and environment forms.
- Error classification, model compatibility, precedence, pinning:
  `crates/gritt-harness/tests/startup_failover.rs` (nine tests) and the
  unit tests in `startup.rs`.
- Provider fixtures for missing key, 401, transport failure, and model-list
  failure with redaction: `every_failure_class_moves_on_and_the_aggregate_names_them_without_a_key`
  and `a_probe_fetches_live_and_reports_the_raw_failure_without_the_key`.
  The single-candidate missing-key path and the model-name-over-hint rule:
  `an_explicit_profile_is_pinned_and_keeps_the_single_candidate_rules`.
- Session round trip and legacy database:
  `last_used_choices_round_trip_per_workspace_and_hold_no_secret`,
  `a_database_from_before_the_last_used_table_upgrades_and_remembers_nothing`,
  and the end-to-end `doctor` check for `5/5 applied`.
- Print, REPL, and TUI on the same resolver:
  `the_draft_path_and_the_flag_path_share_the_resolver_and_its_notes`, and
  the end-to-end tests
  `startup_falls_over_to_the_next_profile_when_the_default_is_unreachable`,
  `a_new_session_reuses_the_last_successful_choices_and_flags_win`, and
  `the_repl_starts_on_the_fallback_profile_and_says_so` through the real
  binary with a local provider and a closed port.

Review: the repository's ticket review ran over the diff. The harness's
separate `/code-review` skill also ran as a forked execution. Its eight
finder agents each reported (reuse, simplification, efficiency, line scan,
removed behaviour, conventions, altitude, cross-file), and the findings that
held up were applied or answered above. The fork's own verify phase and
final verdict had not returned when this ticket closed; the findings were
triaged here rather than waiting on it.

## Completion Gate

- Acceptance: met. Each criterion in `task.md` maps to a test in Validation.
- Scope: within the ticket. Files outside it that changed are the review
  fixes in the same modules and the docs and ADR the work required. The
  execution-modes change already in the tree was left as found.
- Validation: the four commands above pass, with the one pre-existing
  `tui_pty` failure recorded and not caused by this work.
- Security and safety: no new file or network access beyond the model-list
  probe against configured endpoints. The last-used table holds a profile
  name, a model id, and an effort label. Skipped-profile messages are
  redacted against the resolved key, and tests assert the fixture key never
  appears in errors, diagnostics, or terminal output. No policy bypass.
- Regression risk: for a configuration without `fallback_profiles` and
  no completed session, startup resolves exactly as before. Two visible
  changes for everyone: print and REPL print the stale or missing list
  note from the resolver instead of the removed pre-warm, and once a
  session completes, later new sessions start on its choices rather than
  the configured defaults, which is the feature. Users who configure
  `fallback_profiles` get a live list fetch per new session.
- Follow-up: listed below.
- Assumptions: listed above.

## Follow-up

- `primary_profile` decides whether a qualified model id belongs to the
  hinted profile from the in-memory catalog, which print mode has not
  loaded on a cold cache. A profile named like a vendor prefix in an
  OpenRouter id (`openai`) then wins over `--profile openrouter` until the
  list is cached. Same as the pre-ticket `resolve_model`; a cache read in
  `primary_profile` would close it.
- The pre-existing `tui_pty` failure: `crates/gritt/tests/tui_pty.rs:522`
  should wait for `Provider default`.
- Observations from review on the uncommitted execution-modes work in the
  tree, outside this ticket: `agent_for` no longer restores `told_phase`,
  so every resume re-sends the phase note; `/code`, `/plan`, and Shift-Tab
  reset launch-time approval authority to `Ask`; the native `SetPhase`
  branch lacks the in-flight guard `SetMode` has; the mode picker keeps a
  second copy of `ExecutionMode::description`; the approval-to-mode mapping
  is written in four places.
- A typed `status` on `gritt_core::Error` so `classify` and
  `provider_error` stop passing it through the diagnostic.
- `SessionDraft.explicit_profile` could go if the full-screen mode stopped
  seeding the draft and displayed the resolver's primary instead.
- `last_used` is read by `seed_draft`, the lazy warm, and the resolver at
  launch; a `resolve_startup_with(last_used)` would make it one read.
