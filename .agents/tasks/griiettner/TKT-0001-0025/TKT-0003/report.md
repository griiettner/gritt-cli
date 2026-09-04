---
id: TKT-0003
namespace: griiettner
title: Fix gritt-agent parity and cleanup gaps from TKT-0002 review
artifact: report
status: done
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
---

# TKT-0003 Report: Fix gritt-agent parity and cleanup gaps from TKT-0002 review

## Summary

All nine scope items are implemented in `.agents/cli/`, with one part of
item 9 left for the user: the CI workflow file exists but has not been
observed running, because this skill does not push. Nothing was committed
either; the task asked for focused commits, but `tkt-exec` forbids commits
and the working tree still holds TKT-0002's uncommitted changes, so the
split into commits is left to the user.

What changed:

- `frontmatter::split_fence` is the one fence splitter. It accepts a `---`
  opening that ends in `\n` or `\r\n` and closes only on a line that is
  `---` after trailing whitespace is dropped. `parse_document`, `extract_block`,
  `skill::sync::parse_skill_frontmatter`, and
  `migrate::cursor::split_frontmatter` call it. Each parser has a CRLF and a
  `----` regression test.
- `skill::sync::first_sentence` now measures the candidate sentence
  (punctuation included) against 120 characters and picks the earliest of
  `. `, `! `, `? ` by position.
- `migrate cursor` keeps a per-run map of planned destinations. A second
  source that maps to a destination already claimed is recorded in
  `Report.skipped` with the reason `conflicts with <first source>, which
  already maps to this destination in this run; rename one source to migrate
  both`. `Skipped.reason` became a `String` to carry the first source.
  Discovery now dedupes on the canonical path, so a source root that is a
  symlink to a sibling (`.cursor/agent` to `.cursor/agents`) yields one
  document instead of a false conflict.
- `skill new` writes a sentence-case heading (`# Sample skill`) by default;
  `--title` still sets heading and display name verbatim. `--no-openai` and
  `--force` are documented rather than changed; see Key Decisions.
- `ticket::scaffold` holds `Frontmatter` and `render_frontmatter`, and
  `yaml_scalar` moved there. `ticket new` and `ticket new-chain` both render
  through it. `ticket new` gained `--areas`, `--skills`, and
  `--dependencies` with the same zero-or-more-values shape `new-chain` uses.
- `skill::sync` and `ticket::sync` each expose `sync()` returning a `Summary`
  with `exit_code()` and `print()`. `run()` is the printing wrapper.
  `sync_or_rollback` takes an `announce` closure and prints the caller's
  lines before the summary, so `skill new`, `ticket new`, and
  `ticket new-chain` now print "created" before "synced".
- `.agents/tools/README.md` says plainly that the `frontmatter-utils.mjs`
  single-file JSON dump was dropped without a replacement and points at
  `ticket validate`. `new-chain`'s default `--areas` is now `.agents/tasks`
  and `.agents/skills`.
- `.github/workflows/agent-cli.yml` runs fmt, clippy, and test on
  `ubuntu-latest` and `windows-latest` for changes under `.agents/cli/`.

Docs updated: `.agents/cli/README.md` (flags, ordering, fence rule, CI),
`.agents/tools/README.md`, `.agents/skills/tkt-new/SKILL.md`, and the
`--help` text for `skill new --force` and `--no-openai`.

## Key Decisions

- `--no-openai` stays as documented rather than changed. `skill sync`'s
  contract is to generate `agents/openai.yaml` for every skill directory, so
  making `skill new` skip it only delays the file until the next sync. The
  `--help` text and README now say the flag holds only with `--no-sync`.
- `--force` already resets the interface block. Running
  `skill new demo "Second." --force` over a hand-edited `openai.yaml`
  rewrote `short_description` to `Second.`; the only case where the old
  interface survives is `--force --no-openai`, where `--no-openai` asks for
  exactly that. TKT-0002's follow-up was wrong on this point. `--help` and
  the README state the behaviour and the integration test now covers both
  paths.
- The Codex display name stays Title Case (`Sample Skill`) while the heading
  is sentence case. Only the heading was flagged by `skill-management/audit`,
  and Title Case is what `skill sync` generates for a missing interface, so
  the two stay consistent.
- Conflicting migration sources use `Report.skipped` rather than a new
  bucket. The report and manifest already render skipped entries, and the
  reason text names the winning source, so a new section would add shape
  without information.
- `skill new` no longer prints `synced Claude stubs`. The sync summary line
  (`synced skill adapters (N file(s) updated)`) says the same thing with a
  count, and the ticket allowed folding.
- Missing roots are `CliError`s now instead of printed-and-return-1, so the
  pure `sync()` functions have no output. `main` prints `error: <message>`
  with exit 1, which is byte-identical to the previous output.
- `ticket new`'s list flags default to empty. Existing `ticket new` output is
  unchanged, and the repository's own slim tickets carry no `areas` or
  `skills`.
- The CI workflow sets `core.autocrlf false` before checkout. The fixture
  comparisons are byte for byte, and a CRLF checkout on the Windows runner
  would fail every expected-file assertion before any real defect surfaced.

## Alternatives Considered

- A `ticket frontmatter <path>` debug subcommand. Not added: the tools README
  already stated the drop, `ticket validate` reports the same errors, and the
  smallest reversible option was to tighten the wording.
- A `.gitattributes` with `eol=lf` for the fixtures. Rejected in favour of the
  workflow-local git config so the change stays inside the CI file.
- `dtolnay/rust-toolchain` and `Swatinem/rust-cache` actions. Not used; the
  runner's `rustup` and `actions/cache` cover the need without third-party
  actions.
- Skipping all of a document's writes when one destination conflicts. The
  per-write rule matches `filter_existing`, so a skill source that collides
  produces two skipped entries (`SKILL.md` and `agents/openai.yaml`).

## Assumptions

- A fence line is `---` after trailing whitespace and `\r` are dropped, on
  both the opening and the closing line. The first cut required an exact
  match; the review showed a hand-edited `--- ` closing fence then failed
  `ticket validate`, which the old prefix search accepted.
- An empty frontmatter block (`---\n---\n`) now parses as fenced and empty.
  The old search started after the opening line's newline and reported it
  unclosed.
- `ticket new` writes `dependencies` to `task.md` only and `areas` and
  `skills` to every created artifact, mirroring `new-chain`.
- Item 6's mention of `migrate cursor` was already satisfied: it runs the
  maintenance commands as subprocesses and stores their output in the
  manifest, so nothing prints out of order. No change made there.
- Ticket status is `done` although the CI observation and the commit split
  remain for the user; both are actions this skill is not allowed to take.

## Edge Cases and Failures

- The `----` case: with `----` on line 3 and `---` on line 5, all three
  parsers now close on line 5. The ticket parser reports `line 3: expected
  `key: value`` for the stray line; the skill and migration parsers skip it.
  A document whose only candidate is `----` is unclosed, where it used to be
  truncated at the wrong line.
- `first_sentence` with a 119-character sentence plus `.` returns 120
  characters; with 120 plus `.` it falls through to the clip rule.
- `migrate cursor` with `.cursor/commands/review.md` and
  `.claude/commands/review.md`: writes 4 (two skill files, two reports),
  skipped 2, and `SKILL.md` carries the Cursor source.
- `ticket new --areas` with no values clears the list; the task carries no
  `areas` key.

## Validation

All on macOS, from the repository root:

- `cargo fmt --manifest-path .agents/cli/Cargo.toml --all --check`: pass.
- `cargo clippy --manifest-path .agents/cli/Cargo.toml --all-targets -- -D warnings`: pass.
- `cargo test --manifest-path .agents/cli/Cargo.toml`: 90 tests pass (51
  unit, 39 integration), including the new tests listed above.
- `cargo build --release --manifest-path .agents/cli/Cargo.toml`, then
  `gritt-agent ticket sync --check`, `ticket validate`, and
  `skill sync --check`: all pass with no drift on this repository.
- Linux and Windows: not run locally. The workflow file is in place; see
  Follow-up.

## Review

`code-review high` ran over the working-tree diff, which holds TKT-0002's
uncommitted changes as well as this ticket's, so its findings cover both.
The skill forked, dispatched eight finder agents, and returned before its
own dedup and verify steps ran; the same stall TKT-0002 recorded. All
eight finders did report back to this session (ids `aba6fbdfb0a7b790f`,
`aeeffd5cd6fa7c0a2`, `a8f0f89911d4e23a5`, `af342f0869ff4cba0`,
`a37296e3efa6df288`, `a9fb4998369d3da39`, `aaf222e932aa716e7`,
`a3b569f38246f4701`), so the dedup and verification below were done in this
session by rereading each cited file, not by the skill's verifier agents.
Two finders reproduced their findings with the built binary; the rest were
checked against the current source.

Verdict: no unresolved critical or high finding remains on this ticket's
diff. Four findings on this diff were confirmed and fixed; one confirmed
finding (Windows) is recorded as a follow-up per item 9's instruction.

| Finding | Verdict | Outcome |
| --- | --- | --- |
| `split_fence` rejected a closing fence with trailing whitespace (`--- `), which the old prefix search accepted; reproduced with `ticket validate` | Confirmed | Fixed: fence lines are `---` after `trim_end()`; unit test added |
| `discover_docs` deduped on the raw path, so a symlinked source root (`.cursor/agent` to `.cursor/agents`) produced a false conflict under the new guard | Confirmed | Fixed: dedupe on the canonical path; unix integration test added |
| `.agents/tools/README.md` example passed `--step one:First step` unquoted, which clap reads as four workers | Confirmed | Fixed: quoted |
| `tkt-new/SKILL.md` said the list flags take one or more values; they take zero or more | Confirmed | Fixed |
| `tests/codex.rs` compares the config header with the raw `repo.root`, but `project_header` doubles backslashes, so the Windows CI job will fail on both trust tests | Confirmed by reading `trust.rs:63-72` and `tests/codex.rs:46-49` | Follow-up, as item 9 directs |
| `skill new` heading is sentence case where the Node scaffold used Title Case | Confirmed, intended by item 4 | No change |
| `migrate cursor` exits 1 when the target has no `.agents/skills/` because `skill sync` fails | Confirmed, pre-existing: `skill sync` already returned 1 for a missing root before the `CliError` change | Follow-up |

Findings outside this ticket's diff (TKT-0002 code, unchanged here) are
listed under Follow-up with their severity as reported. None of them is
introduced or worsened by this change.

## Completion Gate

- Acceptance: yes for items 1 to 8 and for the verify set; partial for item
  9, whose workflow file exists but has not been observed to run, and whose
  Windows job is expected to fail on the two `codex trust` tests. Next
  action: fix the test expectation (see Follow-up), push a branch that
  touches `.agents/cli/`, and watch the `agent-cli` workflow.
- Scope: yes, with two files outside `.agents/cli/` that the ticket itself
  named (`.github/workflows/agent-cli.yml`, `.agents/tools/README.md`) plus
  `.agents/skills/tkt-new/SKILL.md` for the new `ticket new` flags. No
  caching or process batching was added to `chain-check`; no TOML, YAML, or
  git crate was added.
- Validation: fmt, clippy, 91 tests, and the three repository checks pass on
  macOS. Linux and Windows not run. The review ran and its outcome is above;
  its verify stage was completed in-session because the forked skill
  stalled after dispatch.
- Security and safety: no network access. `migrate cursor` still writes only
  under `.agents/`; the conflict guard only removes writes. The workflow uses
  first-party GitHub actions only and runs no scripts beyond cargo.
- Regression risk: low to medium. The fence rule change can turn a document
  that closed on a `----` typo from "parsed" into "unclosed"; `ticket
  validate` reports that as an error with the file path. `skill new` output
  lost the `synced Claude stubs` line; the scaffold H1 is now sentence case,
  so a caller that grepped for `# Sample Skill` would break. Every
  changed output has an updated integration test.
- Follow-up: see below.
- Assumptions: see above.

## Follow-up

Blocking for CI:

- Windows: `tests/codex.rs` builds the expected `[projects."<root>"]` header
  from `repo.root.display()`, while `codex::trust::project_header` escapes
  every backslash through `toml_basic_string`. On `windows-latest` the temp
  root contains backslashes (and a `\\?\` verbatim prefix from
  `canonicalize`), so the header assertion at line 46 and the in-place edit
  test that seeds a single-backslash section both fail. Fix the tests to
  build the header through `project_header`, then observe the workflow.

From the review, pre-existing in TKT-0002 code, ordered by severity:

- High: `codex::trust::apply_trust` refuses to write when any line of
  `config.toml` contains the project path as a substring, so a sibling
  (`/work/repo-old`), a nested project, or a comment blocks `codex trust`
  for `/work/repo`, and `--check` errors instead of printing `not trusted`.
  Two finders reproduced it with the binary. Restrict the guard to header
  lines whose quoted key equals the path.
- High: migrated `SKILL.md` files carry no `disable-model-invocation: true`,
  so the `skill sync` that `migrate cursor` runs flips every imported
  skill's `allow_implicit_invocation` to `true`, against the
  skill-management rule. Reproduced by a finder on the fixture source.
- Medium: `skill sync` rewrites a migrated `agents/openai.yaml` without its
  `# MIGRATED BY` comment, so the next `migrate cursor` run treats the file
  as user-owned and skips it while re-migrating `SKILL.md`.
- Medium: `ticket new --dry-run` and `ticket new-chain --dry-run` resolve
  identity with `persist: true`, so a dry run with `--namespace` rewrites
  `.agents/state/identity.local.yaml`.
- Medium: `ticket new-chain` writes its folders with `?` and no cleanup, so
  an I/O failure part-way leaves a partial chain with consumed ids.
- Medium: `--branch-pattern` is echoed into the orchestrator contract line
  only; `worker_branch` hard-codes `tkt-{id}-{step}-{slug}`, so a custom
  pattern produces contradictory branch names across the chain.
- Medium: `migrate cursor` exits 1 when the target repo has no
  `.agents/skills/` because `skill sync` fails, even when every write
  succeeded.
- Low: `chain_check::benchmark_expected` decides by regex over task prose,
  and the scaffold always contains `Benchmark requirements:`, so every chain
  warns about missing benchmark evidence until the line is reworded.
- Low: `.agents/memory/architecture/overview.md` still says `.agents/tools/`
  holds maintenance scripts; ADR-004 and `AGENTS.md` carry similar stale
  wording about Node and the CLI's scope.
- Low: no integration test runs `--help` for the six ported subcommands.
- Low, reuse: `agents/openai.yaml` is rendered by three format strings
  (`skill/sync.rs`, `skill/new.rs`, `migrate/cursor.rs`) with three
  `short_description` rules; `new.rs` and `new_chain.rs` still carry private
  `Common` and `frontmatter()` adapters over `scaffold::Frontmatter`;
  `main.rs` copies every `*Args` into a `*Options` by hand;
  `cursor::split_frontmatter` re-implements `frontmatter::clean_scalar`;
  `codex::split_config` re-implements `frontmatter::split_lines`; the
  migrator joins `.agents/skills` by hand instead of `repo::skills_root`;
  `tests/codex.rs` and `tests/ticket.rs` copy the `Output` to `Run`
  conversion.
- Low, efficiency: the migration report and manifest are rendered and
  written twice per run; `apply_writes` repeats `filter_existing`'s
  ownership check and rewrites unchanged files; `Report` clones full file
  contents; `codex trust --check` renders the whole edited config to test
  for a change; `ticket chain-check` reads `report.md` up to three times and
  spawns six `git` processes (out of scope here by the ticket's own words).

Other:

- Split the working tree into focused commits once TKT-0002 and this ticket
  are reviewed together; both are uncommitted on `main`.

## Updates

- [2026-09-04 Windows CI: codex trust tests build headers through project_header](updates/2026-09-04-windows-codex-header.md)
