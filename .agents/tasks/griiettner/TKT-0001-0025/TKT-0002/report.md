---
id: TKT-0002
namespace: griiettner
title: Finish migrating agent-tools to gritt-agent
artifact: report
status: done
owner: griiettner
created: 2026-09-03
updated: 2026-09-03
---

# TKT-0002 Report: Finish migrating agent-tools to gritt-agent

## Summary

`gritt-agent` now owns every tool the repository needs. The six Node scripts
became subcommands with the surface `plan.md` specified: `skill new`,
`ticket identity`, `ticket new-chain`, `ticket chain-check`, `codex trust`,
and `migrate cursor`. `.agents/tools/agent-tools/` is deleted, including the
shared `lib/` modules, `frontmatter-utils.mjs`, and `agent-tools.test.mjs`.
Only `.agents/tools/README.md` remains, pointing at the CLI.

Parity was proven before deletion by running each Node script and its Rust
replacement on identical inputs and diffing the results. The generated
`SKILL.md`, `openai.yaml`, all six chain scaffold files, the identity file,
the Codex `config.toml`, and the migrated skill, agent, and memory files were
byte-identical apart from two intended changes: scaffolds name
`gritt-agent ticket chain-check` instead of the deleted script, and migrated
files carry the marker `<!-- MIGRATED BY gritt-agent migrate cursor; DO NOT
EDIT -->`. The three legacy markers are still recognised as migrator-owned.

New modules: `skill/new.rs`, `ticket/new_chain.rs`, `ticket/chain_check.rs`,
`codex/trust.rs`, `migrate/cursor.rs`. Shared helpers added: `fsx::kebab_case`,
`fsx::relative_path_posix`, `fsx::list_files_recursive`,
`fsx::read_text_lossy`, `fsx::normalize_lexical`, `repo::expand_home`,
`repo::home_dir`, `ticket::store::find_ticket_dir` and `find_ticket_dir_with`,
`ticket::new::sync_or_rollback`, `skill::sync::display_name`. `repo::git_toplevel`,
`skill::sync::quote_yaml`, `ticket::sync::render_list`, and
`ticket::new::yaml_scalar` became shared.

Docs and skills updated: `tkt/store`, `tkt-new`, `tkt-new-chain`,
`skill-management`, `tkt-exec-chain`, `dev`, `MIGRATION.md`, `README.md`,
`AGENTS.md` (the Node rule), `.agents/tools/README.md`, `.agents/cli/README.md`,
and `.agents/settings.json` (two `Bash(node ...)` permissions removed).

## Key Decisions

- `migrate cursor` runs the three maintenance commands through the same binary
  (`std::env::current_exe()`) with captured output, not in-process as `plan.md`
  said. The review showed the in-process version wrote empty `stdout` and
  `stderr` into `cursor-migration-manifest.json`, which `MIGRATION.md` tells
  the user to read for maintenance results, so manifest parity won over the
  plan's implementation preference. The console shows only the summary, as
  Node did.
- `codex trust` with no positional path trusts `--repo-root` when given, else
  the working directory, resolved to its git top level when there is one. It
  never requires `.agents/`, matching the Node default of `.`. The first
  version required a repository root and the review caught the regression.
- `codex trust` refuses to write when `config.toml` mentions the project path
  in any form other than the exact `[projects."<path>"]` header it edits, such
  as an inline table or a literal-string header. Appending would create a
  duplicate table and invalid TOML. No TOML crate was added, per the ticket.
- Project paths for `codex trust` are made absolute and lexically normalised
  (`..` and trailing slashes removed) so the TOML key matches the working
  directory Codex looks up, the way Node's `path.resolve` behaved.
- `ticket chain-check` compares changed files against the ticket's full
  `.agents/tasks/<namespace>/<chunk>/<id>/` path instead of the bare id
  substring the Node script used, so a same-numbered ticket in another
  namespace is flagged as another ticket's folder.
- `ticket chain-check` resolves the developer identity only when a bare id
  matches more than one namespace. Node called `gh api user` on every run;
  the new `find_ticket_dir_with` takes the lookup as a closure.
- `ticket new-chain` rolls back every scaffolded folder when the index sync
  fails, and `sync_or_rollback` now also removes the chunk and namespace
  folders it created when they are empty. `ticket new` shares the helper.
- `skill new`'s `short_description` keeps the whole description when it fits
  120 characters and otherwise uses `skill::sync::first_sentence`. The
  Node scaffold preferred the whole text; `skill sync` preserves whatever the
  file has, so `skill sync --check` passes right after creation.
- `--step` accepts repeated flags or several values in one flag; `--skills`,
  `--areas`, and `--dependencies` accept zero values to clear the defaults,
  matching the deleted `parseCli` multi type.
- `migrate cursor` reads source files with `from_utf8_lossy`, as Node's
  `readFile(..., 'utf8')` did, so one Latin-1 rule file no longer aborts the
  whole migration.
- Chain titles starting with `[` or `{` are quoted through `yaml_scalar`, the
  fix TKT-0001 added to `ticket new`. Node did not quote them.
- The `TODO(tkt)` scaffold placeholder and the "at least two `--step` values"
  guard are unchanged. The closing console line now says `gritt-agent ticket
  validate` instead of `tkt-validate`.

## Alternatives Considered

- Keeping maintenance in-process for `migrate cursor`. Rejected: it drops the
  command output the manifest is documented to hold.
- A shared `expand_home` in `fsx`. Placed in `repo.rs` beside `git_toplevel`
  instead, since both are environment lookups.
- A `toml_edit` dependency for `codex trust`. Rejected by the ticket's
  out-of-scope list; the duplicate-mention guard covers the failure the
  reviewer raised.
- Sentence-case H1 in `skill new` scaffolds. Left as Title Case for parity;
  see Follow-up.
- Making `--no-openai` skip `openai.yaml` even when `skill sync` runs.
  Rejected: sync generates that file for every skill by contract, and the
  Node flag had the same limit. Documented in the CLI README instead.

## Assumptions

- "Resulting tickets pass `gritt-agent ticket validate`" in the acceptance
  criteria means the chain structure validates once the scaffold markers are
  replaced. Validate errors on the `TODO(tkt)` placeholder by design, and the Node scaffold
  produced the same markers. The integration test replaces the markers and
  asserts `tkt_validate ok (0 warnings)`.
- The acceptance grep's exclusion of "historical `report.md`" extends to
  TKT-0001's `task.md`, which lists `.agents/tools/agent-tools/` as an input.
  Ticket history is canonical and was not rewritten.
- The `--areas` default still includes `.agents/tools`, which now holds only a
  README. Changing defaults is behaviour the ticket rules out.
- `identity --refresh` was not integration-tested because it calls `gh`.
  The stored, flag, and env paths are covered.
- Directory sorting for report ordering uses case-insensitive then byte order,
  the approximation TKT-0001 chose for `localeCompare`.

## Edge Cases and Failures

- The classifier scores the source path too, so `.cursor/rules/plain.txt`
  lands in `principles` on the word `rule`. Node behaved the same; the fixture
  uses `.claude/memory/plain.txt` for the low-confidence case.
- The console `writes:` count includes the report and manifest files, so five
  content writes print `writes: 7`. Node printed the same; a first test
  expectation was wrong, not the code.
- After a synced migration, `skill sync` rewrites each migrated
  `openai.yaml` without the marker comment and with
  `allow_implicit_invocation: true`, so a rerun skips those files as not
  migrator-owned. Inherited from Node's identical sync step.
- `cargo fmt` rewrapped code between two scripted edit passes, so two edits
  had to be reapplied. No behaviour impact.

## Validation

Run from the repository root on macOS (arm64), Rust 1.93.1:

- `cargo fmt --manifest-path .agents/cli/Cargo.toml --all --check`: pass.
- `cargo clippy --manifest-path .agents/cli/Cargo.toml --all-targets -- -D warnings`: pass.
- `cargo test --manifest-path .agents/cli/Cargo.toml`: 45 unit and 37
  integration tests pass (codex 3, mcp 1, memory 5, migrate 2, skill 7,
  ticket 19). New fixtures: `tests/fixtures/cursor-source/` and eight
  `expected/` files (`chain-*.md`, `skill-new-*`). Chain expectations use a
  `{{TODAY}}` placeholder for dates.
- Parity spot checks per `plan.md`, Node 20.18.0 against the release binary
  on the same inputs: `skill new` output identical and `skill sync --check`
  clean; `ticket identity` three-line output and identity file identical;
  six chain files identical after normalising the chain-check command name;
  `ticket chain-check` notes, warnings, and summary identical on a scratch
  git repository; `codex trust --check` verdicts and `config.toml` identical
  before and after trusting; `migrate cursor --dry-run` and real run produced
  identical counts and identical skill, agent, and memory files after
  normalising the marker.
- Real repository after deletion: `skill sync` 0 files updated, `ticket sync`
  ok, `ticket validate` ok with 0 warnings, `skill sync --check` and
  `ticket sync --check` no drift.
- `grep -rn "agent-tools/" .agents .claude MIGRATION.md README.md` excluding
  this ticket and historical reports: only TKT-0001's `task.md` input list.
- Not run: builds or tests on Linux and Windows.

## Review

`review/ticket` self-review: every acceptance criterion has a test or a
recorded parity run; no file outside scope changed except `AGENTS.md` and
`dev/SKILL.md`, whose Node sentences contradicted the deleted tree.

The harness `code-review` skill (level `high`, forked) dispatched eight finder
agents: reuse, simplification, efficiency, altitude, conventions, cross-file
tracer, removed-behaviour audit, and line-by-line scan. All eight returned
findings, but the lead never produced its final verified verdict, so the
triage below is mine, not the review's. Fixed in this change: `codex trust`
requiring a repository root; manifest `stdout`/`stderr` lost; non-UTF-8
source files aborting migration; `--step` multi-value and empty list flags
rejected; chain-check substring match; chain-check calling `gh` on every run;
rollback leaving empty chunk and namespace folders; duplicate dispatch and
`unreachable!` in `main.rs`; `..` and trailing slashes in trust keys; the
duplicate-table risk in `config.toml`; three copies of the kebab-case loop;
two copies of `expand_home`; two copies of the sync-rollback block; a
duplicated list renderer and clip tail; `AGENTS.md` and `dev/SKILL.md` still
allowing Node; the tools README overclaiming that every script had a
replacement. Deferred items are under Follow-up.

## Completion Gate

- Acceptance: yes. Six subcommands exist with unit and integration coverage;
  `.agents/tools/agent-tools/` is gone; every skill, doc, and permission that
  named a script now names the subcommand; fmt, clippy, and tests pass.
- Scope: yes, with two recorded extensions. `AGENTS.md` and `dev/SKILL.md`
  changed one sentence each so the Rust-only rule matches the tree, and
  `ticket new` gained the empty-parent cleanup through the shared rollback
  helper.
- Validation: pass on macOS. Linux and Windows not run.
- Security and safety: no network access added. `git` runs only for root and
  branch discovery, `gh` only for identity and only on ambiguous bare ids.
  `codex trust` writes one file under `CODEX_HOME` and refuses ambiguous
  edits. Rollback removes only folders the command created. Migration skips
  symlinks and never writes outside `.agents/`.
- Regression risk: low. Existing commands are untouched except `ticket new`,
  which now removes empty parents after a failed sync. Chain scaffolds and
  migrated files match the Node output byte for byte.
- Follow-up: see below.
- Assumptions: see above.

## Follow-up

- The harness review lead did not return a final verdict; a later
  `code-review` run over this diff would confirm or dispute the triage above.
- CRLF frontmatter fences are rejected by all three parsers (`frontmatter.rs`,
  `skill::sync::parse_skill_frontmatter`, `migrate::cursor::split_frontmatter`),
  and the `\n---` closing search matches a line starting with `----`. One
  shared fence splitter would fix all three.
- `skill::sync::first_sentence` accepts a 121-character sentence and searches
  `. ` before `! ` and `? `. Pre-existing; now also used by `skill new`.
- Two source documents with the same slug (for example `.cursor/commands/review.md`
  and `.claude/commands/review.md`) overwrite each other silently. Inherited.
- `skill new` H1 is Title Case, which `skill-management/audit` flags as
  advisory. Node did the same.
- `skill new --no-openai` has effect only with `--no-sync`; `--force` does not
  reset a hand-edited `openai.yaml` interface. Both inherited.
- `ticket new` and `ticket new-chain` render frontmatter separately; a shared
  `ticket/scaffold.rs` would let `ticket new` emit `areas` and `skills`.
- `ticket::sync::run` and `skill::sync::run` print their own summaries, so
  callers announce creation after the sync line. Splitting compute from print
  would fix the ordering.
- `frontmatter-utils.mjs` had a debugging entry point that printed one file's
  parsed frontmatter as JSON; no subcommand replaces it.
- The `--areas` default still lists `.agents/tools`, which now holds only a
  README.
- `ticket chain-check` reads `report.md` up to three times and spawns six
  `git` processes; harmless at this size.
- CI on Linux and Windows, carried from TKT-0001.
