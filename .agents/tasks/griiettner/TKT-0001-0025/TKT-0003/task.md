---
id: TKT-0003
namespace: griiettner
title: Fix gritt-agent parity and cleanup gaps from TKT-0002 review
artifact: task
status: done
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
---

# TKT-0003 Task: Fix gritt-agent parity and cleanup gaps from TKT-0002 review

## Goal

Close every item TKT-0002's report left under Follow-up: real parsing and
data-loss bugs, inherited CLI behavior gaps, duplicated code the review
flagged, and the review that never returned a final verdict. Land the small
independent fixes as focused commits; do not fold them into one sweeping
rewrite.

## Inputs

- `.agents/tasks/griiettner/TKT-0001-0025/TKT-0002/report.md`'s Follow-up
  section is the source list for this ticket's scope; read it in full first.
- `.agents/cli/src/frontmatter.rs`, `src/skill/sync.rs`,
  `src/migrate/cursor.rs` for the three frontmatter/fence parsers.
- `.agents/cli/src/skill/new.rs`, `src/ticket/new.rs`, `src/ticket/new_chain.rs`
  for the scaffold and sync-ordering items.
- `.agents/cli/README.md` and `.agents/tools/README.md` for anything whose
  documented behavior needs to change alongside the fix.
- `dev/cli` skill for the crate's verify set and dependency policy.

## Scope

1. **Shared frontmatter fence parsing.** `frontmatter::parse_document`,
   `skill::sync::parse_skill_frontmatter`, and
   `migrate::cursor::split_frontmatter` each re-detect the `---` fence and
   disagree on CRLF (`\r\n`) openings and on a closing search that matches
   any line starting with `---` rather than a line equal to `---`. Extract
   one fence-splitting function in `frontmatter.rs` (accepting `\r\n`, and
   matching the closing fence as a whole line) and have all three parsers
   call it.
2. **`skill::sync::first_sentence` off-by-one and stop-order.** Fix the
   `<= 120` bound so a 121-character sentence does not pass, and search
   `. `, `! `, `? ` by earliest position in the text rather than in that
   fixed priority order. This function is also used by `skill new`, so keep
   both call sites passing.
3. **`migrate cursor` duplicate-slug overwrite.** Two source documents that
   slugify to the same skill, agent, or memory id (for example
   `.cursor/commands/review.md` and `.claude/commands/review.md`) currently
   overwrite each other with no signal. Detect a second write to the same
   destination within one migration run and record it under `Report.skipped`
   (or a new "conflicting" bucket) instead of silently applying the last one.
4. **`skill new` scaffold and flag gaps**, all inherited from the Node
   original: the H1 is Title Case where `skill-management/audit` wants
   sentence case; `--no-openai` has no effect unless `--no-sync` is also
   passed, because `skill::sync::run` always writes `agents/openai.yaml` for
   any skill directory with a `SKILL.md`; `--force` does not reset an
   existing skill's `agents/openai.yaml` interface block because
   `read_interface` prefers the file that is already there. Fix the heading
   case. For `--no-openai` and `--force`, either make the flags do what they
   say or, if that is a larger behavior change than this ticket should make,
   document the limit in `--help` and the CLI README instead of silently
   diverging from the flag's name; record which choice was made and why.
5. **Shared ticket frontmatter renderer.** `ticket::new::frontmatter` and
   `ticket::new_chain::frontmatter` render the same 8-field block by two
   separate format strings, and only the chain renderer emits `areas`,
   `skills`, and `dependencies`. Extract one renderer both commands call, and
   give `ticket new` the same `--areas`/`--skills`/`--dependencies` support
   `new-chain` already has, updating `--help` and any skill or doc that lists
   `ticket new`'s flags.
6. **Sync command output ordering.** `ticket::sync::run` and
   `skill::sync::run` print their own summary line as a side effect of the
   call, so `skill new`, `ticket new`, `ticket new-chain`, and
   `migrate cursor` print "created ..." after the sync's own "synced ..."
   line instead of before it. Split each `sync::run` into a pure function
   that returns a summary and a thin wrapper that prints it, so callers can
   print their own message first and the summary after, or fold it into
   their own output.
7. **Stale `.agents/tools/README.md` and `--areas` default.** Update the
   tools README so it no longer claims every deleted Node script had a
   direct replacement (`frontmatter-utils.mjs`'s single-file JSON dump has
   none); either add a small `ticket frontmatter <path>` debug subcommand
   that prints parsed metadata as JSON, or state plainly that the debug
   entry point was dropped without a replacement. Also drop
   `.agents/tools` from `ticket new-chain`'s default `--areas` list now that
   the directory holds only a README, or state why it stays.
8. **Re-run the review.** TKT-0002's `code-review` (level `high`, forked)
   never returned a final consolidated verdict; every finding in this ticket
   came from unverified finder output triaged by hand. After landing 1-7,
   run `/code-review high` (or the equivalent) over this ticket's diff and
   confirm no unresolved critical or high finding remains before closing.
9. **CI on Linux and Windows**, carried forward from TKT-0001's Follow-up.
   Add a CI workflow that runs `dev/cli`'s verify set
   (`cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`) on at
   least one Linux and one Windows runner for `.agents/cli/`. Record any
   platform-specific failure this surfaces as its own follow-up rather than
   papering over it here.

## Out of Scope

- `ticket chain-check` reading `report.md` more than once and spawning six
  `git` processes per run. TKT-0002's report called this harmless at current
  repository size; do not add caching or process-batching speculatively.
- Any new migration heuristic, chain role, or trust file format beyond what
  items 1-9 name. This ticket fixes named gaps; it does not extend behavior.
- Re-deciding the crate's dependency policy: no TOML, YAML, or git library
  additions for item 1 or item 3.
- The product Cargo workspace and Gritt's own runtime; this ticket only
  touches `.agents/cli/`.

## Acceptance Criteria

- The three frontmatter/fence parsers share one fence-splitting function; a
  CRLF-fenced document and a document containing a `----` line inside the
  body both parse correctly in all three call sites, with a regression test
  per parser.
- `first_sentence` rejects a 121-character candidate sentence and picks the
  earliest sentence-ending punctuation regardless of which mark it is, with
  tests for both cases.
- `migrate cursor` no longer silently overwrites one migrated file with
  another in the same run; the report and manifest show the conflict, with a
  test using two source documents that collide on the same slug.
- `skill new` produces a sentence-case H1; `--no-openai` and `--force`
  either behave as named or are documented as not doing so, with the choice
  stated in `report.md`.
- `ticket new` and `ticket new-chain` share one frontmatter renderer;
  `ticket new` accepts `--areas`, `--skills`, and `--dependencies` and
  writes them into `task.md`, with a test.
- `skill sync` and `ticket sync`'s summary can be obtained without printing
  it, and every caller's own "created"/"chain tickets" message prints before
  the sync summary in the actual command output.
- `.agents/tools/README.md` no longer claims parity for
  `frontmatter-utils.mjs` unless a replacement subcommand exists; the
  `new-chain` `--areas` default is updated or explicitly justified.
- `/code-review high` (or equivalent) has run over this ticket's diff and
  `report.md` states its outcome, not just that the earlier one stalled.
- CI runs the `dev/cli` verify set on Linux and Windows for `.agents/cli/`
  and the workflow file is committed.
- `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, and
  `cargo test`, run with `--manifest-path .agents/cli/Cargo.toml`, all pass.

## Verification

- `cargo fmt --manifest-path .agents/cli/Cargo.toml --all --check`
- `cargo clippy --manifest-path .agents/cli/Cargo.toml --all-targets -- -D warnings`
- `cargo test --manifest-path .agents/cli/Cargo.toml`
- `.agents/cli/target/release/gritt-agent ticket sync --check`,
  `ticket validate`, and `skill sync --check` on this repository.
- A completed, non-stalled `/code-review` pass over the diff, with its
  verdict recorded in `report.md` per the `review` skill.
- The new CI workflow observed to run and pass on a pushed branch or PR.
