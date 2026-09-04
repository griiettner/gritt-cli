---
id: TKT-0003
namespace: griiettner
title: Windows CI: codex trust tests build headers through project_header
artifact: update
status: done
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
---

# Windows CI: codex trust tests build headers through project_header

## Trigger

The review that closed this ticket confirmed the Windows job of
`agent-cli.yml` would fail on two `codex trust` tests. The user asked for the
fix and a push so the workflow can be observed.

## Changed files

- `.agents/cli/tests/codex.rs`

## Failure

`trust_checks_adds_and_reports_an_existing_entry` built its expected config
text as `[projects."{root}"]` from `repo.root.display()`, while
`codex::trust::project_header` doubles every backslash through
`toml_basic_string`. On a Windows temp root such as
`\\?\D:\a\_temp\gritt-agent-xyz` the two strings differ, so the equality
assertion fails. `trust_accepts_a_path_and_edits_existing_sections_in_place`
seeded `config.toml` with the same unescaped form, so `find_section` never
matched it, the tool appended a second table, and the assertion failed as
well. Both pass on macOS and Linux because those paths carry no backslash.

## Fix

The tests now build every expected header with `project_header`, the same
function the binary uses, so the escaped and unescaped forms agree on every
platform. No product code changed.

## Validation

- `cargo test --test codex`: 3 pass on macOS.
- `cargo fmt --all --check` and `cargo clippy --all-targets -- -D warnings`:
  pass.
- The `agent-cli` workflow result on the pushed commit is recorded in the
  report's Updates entry for this file once observed.

## Remaining follow-up

The Windows job has not run yet at the time of writing; see the report for
the observed result.
