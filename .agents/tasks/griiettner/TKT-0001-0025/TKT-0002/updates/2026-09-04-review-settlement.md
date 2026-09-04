---
id: TKT-0002
namespace: griiettner
title: Settle the pre-existing findings from the TKT-0003 review
artifact: update
status: done
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
---

# Settle the pre-existing findings from the TKT-0003 review

## Trigger

TKT-0003 re-ran `code-review high` over the working tree, which still held
this ticket's uncommitted changes. Its finders reported defects in TKT-0002
code that TKT-0003 had to leave alone. The user asked for those to be fixed
and recorded here. Every follow-up this ticket's report listed was already
closed by TKT-0003 except `chain-check`'s repeated reads, which stays as is.

## Changed files

- `.agents/cli/src/codex/trust.rs`
- `.agents/cli/src/migrate/cursor.rs`
- `.agents/cli/src/skill/sync.rs`
- `.agents/cli/src/ticket/new.rs`, `new_chain.rs`, `main.rs`
- `.agents/cli/tests/help.rs` (new), `migrate.rs`, `ticket.rs`
- `.agents/cli/tests/fixtures/expected/chain-orchestrator-task.md`
- `.agents/cli/README.md`, `AGENTS.md`,
  `.agents/memory/architecture/overview.md`,
  `.agents/memory/decisions/ADR-004-project-local-agent-cli.md`

## Failures observed and fixes

- `codex trust` refused to write, and `--check` errored instead of printing
  `not trusted`, whenever any line of `config.toml` contained the project
  path as a substring: a sibling `/work/repo-old`, a nested project, or a
  comment. Two finders reproduced it. The guard now matches only a
  literal-string header `[projects.'<path>']` or an inline entry whose key
  is the quoted path. `apply_trust` takes the path instead of a pre-built
  header, so the escaped and raw forms are derived in one place. Unit test
  `other_paths_and_comments_do_not_block_the_append` added.
- Migrated `SKILL.md` files carried no `disable-model-invocation: true`, so
  the `skill sync` that `migrate cursor` runs flipped every imported skill's
  `allow_implicit_invocation` to `true`, against the skill-management rule.
  The migrator now writes the field. Verified in the maintenance test: the
  imported `review` skill ends with `allow_implicit_invocation: false` after
  the sync.
- `skill sync` rewrote a migrated `agents/openai.yaml` without its
  `# MIGRATED BY` comment, so the next migration treated the file as
  user-owned and skipped it. The sync now keeps a file's leading `#` lines.
  A full-sync migration followed by a rerun reports `skipped: 0`, and
  `skill sync --check` is clean afterwards.
- `ticket new --dry-run` and `ticket new-chain --dry-run` persisted the
  resolved identity, so a dry run with `--namespace` rewrote
  `.agents/state/identity.local.yaml`. Both pass `persist: !dry_run`. The
  dry-run test asserts the file is absent.
- `ticket new-chain` wrote its folders with `?` and no cleanup, so a
  failure part-way left a partial chain with consumed ids. It now refuses
  up front when any allocated folder or file exists, and removes everything
  it wrote when a later write fails, through the same helper the sync
  rollback uses. Test: a stray file at the second worker's id exits 1 and
  leaves no orchestrator folder.
- `--branch-pattern` was only echoed into the orchestrator's contract line
  while `worker_branch` hard-coded `tkt-{id}-{step}-{slug}`, and the
  orchestrator carried a second, contradictory literal. The pattern now
  drives every worker branch with `{id}`, `{step}`, and `{slug}`
  placeholders; the default changed from `tkt-{id}-{slug}` to
  `tkt-{id}-{step}-{slug}` so default output is unchanged, and the second
  literal line is gone. Test: `feat/{id}-{slug}` yields `feat/0005-b`.
- `migrate cursor` exited 1 when the target had no `.agents/skills/`
  because `skill sync` fails without one. `skill sync` now runs only when
  the skills root exists; the manifest records two commands in that case.
  Test added with a rules-only source into an empty target.
- No test exercised `--help`. `tests/help.rs` runs it for the root and all
  thirteen subcommands and checks for a usage line.
- `overview.md` said `.agents/tools/` holds maintenance scripts, ADR-004 said
  Node stays for scripts without a replacement, and `AGENTS.md` listed the
  CLI's scope without Codex trust or migration. All three now describe the
  tree as it is.

## Deferred, with reasons

- `chain-check`'s benchmark heuristic matches `\bbenchmark` in task prose,
  and the scaffold always contains `Benchmark requirements:`, so every chain
  warns until that line is reworded. Making it data (a frontmatter key) is
  a scaffold contract change; left for a ticket that also updates
  `tkt-exec-chain`.
- The three `agents/openai.yaml` renderers, the `Common`/`frontmatter()`
  adapters in `new.rs` and `new_chain.rs`, the hand-copied `*Args` to
  `*Options` blocks in `main.rs`, `split_frontmatter`'s quote stripping,
  `split_config`, the migrator's inline `.agents/skills` path, and the test
  `Output` to `Run` copies are reuse cleanups with no user-visible defect.
- The efficiency items (double report write, repeated ownership checks,
  content clones, `--check` rendering the whole config, `chain-check` reads
  and `git` spawns) are harmless at this repository's size.

## Validation

- `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`,
  and `cargo test`: 96 tests pass on macOS.
- `gritt-agent ticket sync --check`, `ticket validate`, and
  `skill sync --check` on this repository: clean.
- The `agent-cli` workflow ran on commit `e2bd3c9` (run 33838989615):
  `verify (ubuntu-latest)` and `verify (windows-latest)` both succeeded.

## Remaining follow-up

The deferred items above.
