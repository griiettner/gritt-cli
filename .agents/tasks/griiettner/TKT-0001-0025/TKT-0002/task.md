---
id: TKT-0002
namespace: griiettner
title: Finish migrating agent-tools to gritt-agent
artifact: task
status: done
owner: griiettner
created: 2026-09-03
updated: 2026-09-03
---

# TKT-0002 Task: Finish migrating agent-tools to gritt-agent

## Goal

Make `gritt-agent` the sole driver of every tool this repository needs.
Port the six remaining Node scripts under `.agents/tools/agent-tools/` into
the `gritt-agent` crate, prove parity, then delete the Node originals, their
now-dead shared `lib/` modules, and every doc or skill reference to them.

## Inputs

- `.agents/cli/` (the existing crate: `ticket::store`, `ticket::identity`,
  `ticket::new`, `ticket::sync`, `ticket::validate`, `skill::sync`,
  `frontmatter`, `fsx`, `repo`) and its `report.md` for prior decisions and
  known follow-ups.
- The six scripts to port, read in full before starting:
  `.agents/tools/agent-tools/create-skill.mjs`
  `.agents/tools/agent-tools/tkt-identity.mjs`
  `.agents/tools/agent-tools/tkt-new-chain.mjs`
  `.agents/tools/agent-tools/tkt-chain-check.mjs`
  `.agents/tools/agent-tools/trust-codex-project.mjs`
  `.agents/tools/agent-tools/migrate-cursor-setup.mjs`
- Their shared libraries: `.agents/tools/agent-tools/lib/cli.mjs`,
  `lib/fs-utils.mjs`, `lib/tkt-store.mjs`, `lib/tkt-identity.mjs`,
  `.agents/tools/agent-tools/frontmatter-utils.mjs`, and the existing
  `.agents/tools/agent-tools/agent-tools.test.mjs` for the current behavior
  those scripts are expected to have.
- `.agents/tasks/README.md`, `MIGRATION.md`, `.agents/tools/README.md`,
  `.agents/settings.json`, and the skills that name one of the six scripts:
  `.agents/skills/tkt/store`, `.agents/skills/tkt-new-chain`,
  `.agents/skills/skill-management`, `.agents/skills/tkt-exec-chain`.
- `plan.md` for the exact command surface, module layout, and sequence.

## Scope

- Add these subcommands to `gritt-agent`: `skill new`, `ticket identity`,
  `ticket new-chain`, `ticket chain-check`, `codex trust`, `migrate cursor`.
- Preserve current ticket allocation and chunking rules (ADR-003),
  frontmatter fields, and generated file formats; match console output
  closely enough that no information the old script reported is lost, even
  where exact wording tightens to this crate's house style.
- Add unit tests for pure logic (classification, rendering, parsing) and
  integration tests that run the built binary against fixture repositories,
  matching the existing test shape in `.agents/cli/tests/`.
- Update every skill and doc listed in Inputs that names one of the six
  scripts, and the `Bash(...)` permission entries in `.agents/settings.json`
  if any name a deleted script path directly.
- Delete, once its replacement's tests pass and a manual parity check on
  this repository succeeds: the six `.mjs` scripts, `lib/tkt-store.mjs`,
  `lib/tkt-identity.mjs`, `frontmatter-utils.mjs`, and once nothing else
  imports them, `lib/cli.mjs`, `lib/fs-utils.mjs`, and
  `agent-tools.test.mjs`. Delete `.agents/tools/agent-tools/` itself once it
  is empty.

## Out of Scope

- Ticket lifecycle rules, frontmatter schema, or chunking (ADR-001/ADR-003
  stay as written).
- The Gritt product runtime and its future Cargo workspace.
- Any behavior not already present in one of the six named scripts (do not
  add new migration heuristics, new chain roles, or new trust file formats
  while porting).
- Re-deciding the CLI's dependency policy from TKT-0001; keep using
  `std::process::Command` for `git`/`gh`, no new TOML or YAML crate.

## Acceptance Criteria

- `gritt-agent skill new <name> <description>` creates the same
  `SKILL.md`/`agents/openai.yaml` shape `create-skill.mjs` did and passes
  `gritt-agent skill sync --check` immediately after.
- `gritt-agent ticket identity` matches `tkt-identity.mjs`'s three-line
  output and `.agents/state/identity.local.yaml` format, including
  `--refresh`, `--namespace`, and `--no-persist`.
- `gritt-agent ticket new-chain` reproduces `tkt-new-chain.mjs`'s scaffold
  shape (orchestrator/workers/reviewer, `chain_role`/`chain_parent`/
  `chain_children`, the "at least two `--step`" guard, scaffold placeholder
  markers) and the resulting tickets pass `gritt-agent ticket validate`.
- `gritt-agent ticket chain-check` reproduces `tkt-chain-check.mjs`'s
  checks: ticket artifact and frontmatter presence, required report
  sections, branch/base/merge-base notes, changed-file cross-ticket
  warnings, and benchmark evidence checks, with the same `--require-report`
  and `--require-benchmark` flags and exit codes.
- `gritt-agent codex trust` reproduces `trust-codex-project.mjs`'s
  trusted/not-trusted detection and `config.toml` section editing,
  including `--check` and the `CODEX_HOME`/`~/.codex` resolution.
- `gritt-agent migrate cursor` reproduces `migrate-cursor-setup.mjs`'s
  discovery, classification, migrated/skipped/ambiguous reporting, and
  `--dry-run`/`--force`/`--no-sync` behavior, and writes the same report
  and manifest files under `.agents/migrations/`.
- `.agents/tools/agent-tools/` no longer exists.
- `grep -rn "agent-tools/" .agents .claude MIGRATION.md README.md` (excluding
  this ticket's own artifacts and any historical `report.md`) returns
  nothing that points at a deleted script.
- `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`,
  and `cargo test`, run with `--manifest-path .agents/cli/Cargo.toml`, all
  pass.

## Verification

- `cargo fmt --manifest-path .agents/cli/Cargo.toml --all --check`
- `cargo clippy --manifest-path .agents/cli/Cargo.toml --all-targets -- -D warnings`
- `cargo test --manifest-path .agents/cli/Cargo.toml`
- Run each new subcommand against this repository (or a temp copy) and
  compare its output and written files against the Node original's, per
  `plan.md`'s "Parity spot checks" section.
- `gritt-agent ticket sync --check`, `gritt-agent ticket validate`, and
  `gritt-agent skill sync --check` all pass after the Node deletions and
  doc updates.
- Load `review/ticket` and `review/impact` per `tkt-exec` step 6 before
  writing `report.md`.
