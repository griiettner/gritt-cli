---
id: TKT-0002
namespace: griiettner
title: Finish migrating agent-tools to gritt-agent
artifact: plan
status: ready
owner: griiettner
created: 2026-09-03
updated: 2026-09-03
---

# TKT-0002 Plan: Finish migrating agent-tools to gritt-agent

## Command surface

```text
gritt-agent skill new <name> <description> [--title <title>] [--force] [--no-openai] [--no-sync] [--dry-run]
gritt-agent ticket identity [--refresh] [--namespace <login>] [--no-persist]
gritt-agent ticket new-chain --title <title> --step <slug:title> [--step ...]
    [--namespace <login>] [--owner <owner>] [--base-branch <branch>]
    [--branch-pattern <pattern>] [--merge-policy <text>] [--reviewer-title <title>]
    [--no-reviewer] [--skills <item>...] [--areas <item>...] [--dependencies <id>...]
    [--create-concept] [--create-plan] [--no-sync] [--dry-run]
gritt-agent ticket chain-check --ticket <id> [--base <branch>] [--require-report] [--require-benchmark]
gritt-agent codex trust [path] [--check]
gritt-agent migrate cursor --source <path> [--dry-run] [--force] [--no-sync]
```

Every new subcommand accepts the existing global `--repo-root`; there is no
per-command `--repo`/`--repo-root` duplicate flag, unlike the Node originals.

## Decisions

- Read every Node source file in full before porting it (already done for
  this plan); do not port from memory or from the `--help` text alone.
- Preserve current file formats, frontmatter fields, ticket allocation and
  chunking rules, and console output shape closely enough that a human
  reading the old and new output side by side sees the same information.
  Exact wording may tighten to match this crate's existing house style
  (see TKT-0001's report for precedent: `sync_skills ok`-style summaries,
  `error:`/`warning:` prefixes) as long as scripts or tests do not depend on
  the literal old wording.
- `skill new` renders the same starter body Node does (Purpose/Workflow/
  Output sections) using `frontmatter.rs` and `skill::sync`'s existing
  `render_stub`/interface helpers where they overlap; after writing
  `SKILL.md` (and `agents/openai.yaml` unless `--no-openai`), call
  `skill::sync::run` in-process instead of shelling out, since this is now
  native Rust calling Rust, not a subprocess bridge.
- `ticket identity` is a thin CLI wrapper over the existing
  `ticket::identity::resolve_ticket_identity`/`persist_identity` functions.
  No new logic; only argument parsing and print formatting matching the
  Node script's three output lines (login, `source: ...`, optional
  `stored: ...`).
- `ticket new-chain` builds chain ticket ids from `ticket::store`'s existing
  `next_ticket_number`, `ticket_dir`, `pad_ticket_number`, resolves identity
  through `ticket::identity`, and renders orchestrator/worker/reviewer
  `task.md` (plus optional `concept.md`/`plan.md` on the orchestrator) with
  the same `chain_role`, `chain_parent`, `chain_children`, and
  `dependencies` fields the frontmatter parser already supports. After
  writing, call `ticket::sync::run` in-process when `--no-sync` is absent,
  with the same rollback-on-failure behavior `ticket::new::run` already has.
  Keep the "at least two `--step` values" guard and the scaffold placeholder
  marker convention `ticket::validate` already checks for.
- `ticket chain-check` shells out to `git` via `std::process::Command`
  (`rev-parse --show-toplevel`, `rev-parse --abbrev-ref HEAD`,
  `rev-parse <ref>`, `merge-base`, `diff --name-only`), the same pattern
  `ticket::identity` already uses for `gh`. No git library dependency.
  Resolve the ticket through `ticket::store::find_ticket_dir` (a new
  function analogous to the Node `findTicketDir`; `ticket::store` does not
  have one yet) using the same preferred-namespace-from-identity behavior
  the Node version has.
- `codex trust` ports the Node script's hand-rolled TOML section editor
  line by line (find `[projects."<path>"]`, check or insert
  `trust_level = "trusted"`) rather than adding a TOML crate, since the
  Node version never needed a real parser either and the file format here
  is narrow and fully under this tool's control. Reads `$CODEX_HOME` or
  `~/.codex`, matching the Node expansion of `~`.
- `migrate cursor` ports the classifier and renderers as pure functions
  first (frontmatter splitting, memory-category scoring, marker detection,
  slug/title helpers) so they are unit-testable without touching disk, then
  a thin orchestration layer for discovery, planning, writing, and the
  maintenance-command handoff to `skill::sync::run`/`ticket::sync::run`/
  `ticket::validate::run` in-process. Rename the migration marker to
  `<!-- MIGRATED BY gritt-agent migrate cursor; DO NOT EDIT -->` and keep
  recognizing the two legacy marker strings already in the Node code
  (`migrate-cursor-setup.mjs`, `migrate_cursor_setup.py`) plus the current
  Node marker, the same legacy-list pattern `skill::sync` already uses for
  stub markers.
- Test with real fixtures under `.agents/cli/tests/fixtures/`, extending the
  existing repo fixture where a new command can reuse it (`ticket
  identity`, `ticket new-chain`, `ticket chain-check` all fit naturally
  next to the existing ticket fixtures) and adding new ones only where the
  scenario does not fit (a `.cursor`/`.claude` source tree for `migrate
  cursor`; a `CODEX_HOME` temp dir for `codex trust`, following the same
  isolation the Node test used).
- Delete a Node script only after its Rust replacement's tests pass and a
  manual parity run against this repository produces the same practical
  result. Delete `lib/tkt-store.mjs`, `lib/tkt-identity.mjs`,
  `frontmatter-utils.mjs`, `lib/cli.mjs`, `lib/fs-utils.mjs`, and
  `agent-tools.test.mjs` only once no remaining script imports them; delete
  `.agents/tools/agent-tools/` itself once it is empty.
- Update every reference to a deleted script: `.agents/skills/tkt/store`,
  `.agents/skills/tkt-new-chain`, `.agents/skills/skill-management`,
  `.agents/skills/tkt-exec-chain` (its `tkt-chain-check.mjs` commands),
  `MIGRATION.md`, `.agents/tools/README.md`, `.agents/settings.json`
  permissions, and the Node call sites inside `tkt-new-chain.mjs`'s own
  generated task scaffolds (moot once that file is deleted, since the
  replacement command renders the new scaffold text directly).

## Sequence

1. `skill new` and `ticket identity`: simplest, no new cross-module
   dependencies, prove the in-process `skill::sync`/`ticket::identity`
   reuse pattern.
2. `ticket new-chain`: reuses `ticket::store` and `ticket::identity`
   directly; add `find_ticket_dir` to `ticket::store` if `chain-check` will
   also need it, to avoid writing it twice.
3. `ticket chain-check`: needs the git-shelling helpers and
   `find_ticket_dir`.
4. `codex trust`: independent of ticket/skill modules; do any time after
   step 1.
5. `migrate cursor`: largest, do last, once the smaller commands have
   proven the fixture and in-process-sync patterns this one also needs.
6. Run the full verify set, then the parity spot checks below, then delete
   the superseded Node files and update every doc/skill reference, then
   run `gritt-agent ticket sync` and `gritt-agent skill sync` once more to
   confirm the repository is still internally consistent.
7. Self-review with `review/ticket` and `review/impact` before closing,
   per `tkt-exec` step 6.

## Parity spot checks

Before deleting each Node script, run its Rust replacement against this
repository (or a temp copy) and confirm:

- `skill new` produces a `SKILL.md` that passes `gritt-agent skill sync
  --check` immediately.
- `ticket identity` prints the same three-line shape and writes the same
  `.agents/state/identity.local.yaml` format `ticket::identity` already
  reads.
- `ticket new-chain` on a 2-step chain reproduces the same `chain_role`/
  `chain_parent`/`chain_children` shape the existing Node-generated fixture
  in `tests/fixtures/repo` would produce, and passes `gritt-agent ticket
  validate`.
- `ticket chain-check` run against this repository's own current branch
  produces sensible, non-crashing notes even outside a real chain (no
  ticket required to still exit cleanly on `--help`).
- `codex trust --check` against a scratch `CODEX_HOME` matches the Node
  version's trusted/not-trusted verdict before and after trusting.
- `migrate cursor --dry-run` against a small synthetic `.cursor/skills/`
  source tree produces the same migrated/skipped/ambiguous counts shape.
