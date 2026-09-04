---
id: TKT-0004
namespace: griiettner
title: Close brain doc gaps and evaluate a Turso-backed local memory store
artifact: report
status: done
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
---

# TKT-0004 Report: Close brain doc gaps and evaluate a Turso-backed local memory store

## Summary

Track A is done. The brain docs, the memory router, and the commit skills now
describe what runs today. Track B, the storage-engine swap, was not executed.
`plan.md` still lists its six decisions as open, and `task.md` forbids running
Track B under an assumed answer, so the ticket stays at `planning`.

Files changed:

- `.agents/brain/architecture.md`: dropped the `turso` tag; replaced the
  "remaining Node scaffolding scripts" line, since `.agents/tools/` now holds
  only a README that points at the CLI.
- `.agents/brain/services.md`: dropped the `turso` tag.
- `.agents/brain/providers.md`: "local libSQL and FTS5" is now "the bundled
  SQLite and FTS5".
- `.agents/memory/MEMORY.md`: removed the route to the nonexistent
  `operations/index.yaml`; the ticket-history route now includes the
  `<github-login>/` namespace segment.
- `.agents/skills/commit/SKILL.md` and `.agents/skills/commitpush/SKILL.md`:
  the attribution rule is scoped to the user-invoked quick-commit flows and
  states that an agent committing directly as part of a ticket follows its
  harness convention.

No code under `.agents/cli/` changed. No dependency was added.

## Key Decisions

- Attribution rule. `git log` shows the trailer
  `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>` on the three
  commits an agent made directly as part of ticket work (`cfd4b92`, `e2bd3c9`,
  `8126b5a`). The two earlier commits (`990f732`, `679a404`) carry no trailer;
  their invocation path is not recorded. The skill text therefore scopes the
  "no attribution" rule to `/commit` and `/commitpush` and defers to the
  harness convention for direct agent commits. The evidence lives here, not in
  the skill, so the skill does not rot when the model name or history changes.
- `operations/` route. Removed rather than created. Creating the category with
  no real content would be a placeholder, which `tkt/artifacts` forbids.
  Re-add the row when a runbook is filed there.
- Ticket status. Kept at `planning` per `task.md`. `report.md` records the
  Track A completion; the validator accepts a report at this status.

## Alternatives Considered

- Creating `.agents/memory/operations/` with a stub index. Rejected as a
  placeholder.
- Keeping the one-line "NEVER add a Co-Authored-By" rule and instead changing
  the harness to stop adding trailers. Rejected: the harness convention is
  not controlled from this repository, and the observed history already
  carries the trailer.
- Amending the commit skill only and leaving `commitpush`. Rejected: both
  skills carried the identical line, so fixing one would leave the skill set
  contradicting itself.

## Assumptions

- `commitpush/SKILL.md` was edited although `task.md` names only
  `commit/SKILL.md`. Same line, same contradiction. Leaving it would have
  failed the acceptance criterion that the stated rule and `git log` agree.
- The `.agents/tools/` sentence in architecture.md was corrected although the
  concept lists only the `turso` tag for that file. The acceptance criterion
  covers the whole doc ("no description of a system that is not running"),
  and the Node scripts it described are gone.
- MEMORY.md's ticket-history route was corrected on a reviewer finding. It
  omitted the namespace segment, so it was a second dangling path in the same
  list the ticket asked to clean.
- Interpreting "matching what this repository's own commit history actually
  shows" as: direct agent commits carry the trailer, and nothing in the log
  identifies a `/commit` invocation either way. A different reading (treat the
  two trailer-less commits as `/commit` output) would not change the rule.

## Edge Cases and Failures

- The concept's claim that architecture.md and services.md "describe a
  Turso/libSQL file database" did not match HEAD. Both bodies already said
  SQLite; only the frontmatter tag remained. The diff is tag-only for those
  two files plus the `.agents/tools/` correction.
- The concept attributes a "no network requests" guarantee to ADR-004 and
  `capabilities.md`. Neither says that. `services.md` and
  `.agents/brain/README.md` do. Track B's ADR work should target the docs
  that actually make the claim.

## Validation

Run from the repository root with `.agents/cli/target/release/gritt-agent`:

| Check | Result |
| --- | --- |
| `skill sync` | 0 files updated (stubs carry no body text) |
| `ticket sync --check` | ok, no drift |
| `ticket validate` | ok, 0 warnings |
| `memory index` | 172 files indexed |
| `memory search "bundled SQLite FTS5"` | returns the corrected providers.md chunk |
| `grep -riE 'turso\|libsql' .agents/brain .agents/memory .agents/skills` | no matches |
| `cargo fmt`, `clippy`, `cargo test` | not run; no Rust source changed |

Read-through of every `.agents/brain/*.md` for deleted-Node or not-enabled
descriptions: see Follow-up for the two items found outside this ticket's
scope.

Review. `review/ticket` self-check against `task.md` scope and acceptance:
scope items 1 to 4 met, items 5 and 6 not started by design. The harness
`code-review high` pass forked eight finder agents. Three had reported when
this report was written (task ids `accbba9c21afa07ea`, `a83ea4bc94c1d129e`,
`a29190dce5cba12b0`). No deduplicated final verdict arrived. Two findings on
this diff were adopted (shorter attribution rule without embedded evidence;
namespace segment in the MEMORY.md ticket route). The rest concern the
pre-existing concept, plan, and task text and are listed under Follow-up.
Treat the review as partial.

## Completion Gate

- Acceptance: partial. Criteria 1 and 2 (docs accurate, commit skill agrees
  with history) met. Criterion 3 (plan decisions locked) not met; the user has
  not answered them. Criteria 4 to 6 are conditional on Track B and did not
  apply. Criterion 7 (validate and sync clean) met.
- Scope: within the ticket, with the three extensions recorded under
  Assumptions. No Track B file touched.
- Validation: ticket tooling checks passed. Cargo verify set not run because
  no Rust source changed. Harness review partial, see Validation.
- Security and safety: documentation and skill text only. No file, network,
  credential, or dependency change.
- Regression risk: an agent reading the old MEMORY.md route to `operations/`
  would have hit a missing file; that path is gone. The commit skills now
  permit a trailer on direct agent commits, which matches existing practice.
- Follow-up: Track B remains blocked on the user's answers to `plan.md`. See
  Follow-up.
- Assumptions: see Assumptions above.

## Follow-up

- Track B. Answer the six items under "Decisions to lock" in `plan.md`,
  starting with local-only swap versus Turso Cloud sync. Nothing executes
  until then.
- Consider splitting the ticket. Reviewers noted Track A is shipped while the
  ticket stays `planning`, possibly forever if Track B is never approved. A
  slim `done` ticket for Track A and a `concept` ticket for Track B would
  make the index truthful. User's call; not done here.
- `.agents/brain/capabilities.md` lists "Store and retrieve lesson lifecycle
  artifacts", "Produce deterministic workspace activity reports", and a
  "Local Ollama model" as available. `grep -ri 'lesson|ollama'
  .agents/cli/src` finds nothing. These are leftovers from the Node
  implementation and belong in a doc-gap ticket.
- `concept.md` and `plan.md` misattribute the "no network requests" claim to
  ADR-004 and `capabilities.md`. Fix when `/tkt-plan` refreshes the plan.
- `plan.md` cites Turso crate APIs as "checked" without a URL, version, or
  docs.rs reference. `dev/cli` requires the check to be recorded before the
  dependency is added.
- `task.md` Scope item 5 has a malformed brace list
  (`{db,schema.sql,index,search,mcp}.rs`) and omits `chunk.rs` and `mod.rs`.
- Reviewers flagged `write`-skill tells in `plan.md` and `task.md` (em dashes
  at plan.md lines 80 and 95, bold lead-ins, "actually" cluster). Clean up on
  the next plan refresh.

## Updates

- [2026-09-04 local Turso store](updates/2026-09-04-local-turso-store.md)
