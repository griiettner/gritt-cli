---
id: TKT-0025
namespace: griiettner
title: Execution notes for connector CLI version checks and updates
artifact: update
status: done
owner: griiettner
created: 2026-09-06
updated: 2026-09-06
---

# TKT-0025 Update: execution notes

## Trigger

The first full `cargo test --workspace --locked` after the implementation
failed five tests. None was a defect in the feature's runtime path; one
was a contract the feature's own tests exist to enforce.

## Changed files or affected behavior

- `crates/gritt-core/src/connector.rs`: `ConnectorVersionCheck::update_available`
  matched `status()`, which a `CachedStale` result also has, so a stale
  outdated answer counted as an offer. It now matches `Checked` only.
  Caught by `update_actions_are_argument_vectors_and_stale_checks_never_say_current`
  and by `a_failed_query_falls_back_to_the_stale_cache_and_stays_stale`.
- `crates/gritt-harness/src/tui/render.rs`: the help overlay took four
  fifths of the terminal height. With two more command rows the
  "Limitations" section no longer fit at 120x40, which the PTY walkthrough
  `the_fixture_home_walkthrough_runs_by_keyboard_and_never_opens_a_session`
  asserts. Help now takes the terminal height minus two rows; `j`/`k`
  still scroll what does not fit.
- `crates/gritt-harness/src/tui/command.rs`: the registry test enumerates
  every command; `Version` and `Update` were added to its list.
- `crates/gritt-harness/tests/connector_session.rs`: the new REPL test
  wrapped the Codex `text` transcript around an OpenCode connector, whose
  normalizer reads none of it, so the turn after the declined updates
  printed nothing. It now uses `fixtures/opencode/text.jsonl` and asserts
  its "PONG".
- `crates/gritt-connector/src/install.rs`: a `type_complexity` lint on the
  update-action test table; a `type Expected` alias.
- `crates/gritt-harness/tests/snapshots/*`: regenerated with
  `GRITT_UPDATE_SNAPSHOTS=1 cargo test -p gritt-harness --test tui_snapshots`
  for the screens that list commands and for the taller help overlay.
  The diff was reviewed: only the two new command rows and the help
  overlay's size changed.

## Failure or edge case observed

A stale cache carrying an `Outdated` comparison and an update command is
exactly the case acceptance criterion 3 forbids presenting as current.
The type's own predicate had the hole; the TUI and CLI already used the
`CachedStale` variant for their labels, so only the predicate and its
callers (the startup warning prefix) were affected.

## Validation performed

The commands listed in `report.md` under Validation, after these changes.

## Remaining follow-up

None from this round. The report's follow-up list stands.
