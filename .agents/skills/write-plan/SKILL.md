---
name: write-plan
description: Writes feature plans and proposals under .agents/plans. Use when the user requests a product feature plan or proposal outside ticket execution.
disable-model-invocation: true
---

# Write feature plans

Create or update a repository-level feature plan under `.agents/plans/`. Use
this for cross-cutting product proposals that are not yet ticket execution
contracts. Do not use it for `task.md`, `plan.md` inside a ticket, or a
completion report.

## Workflow

1. Read [write](../write/SKILL.md) first and follow its prose workflow.
   Read `AGENTS.md` for repository context routing.
2. Query local memory before source search. If local memory is unavailable,
   use `.agents/memory/MEMORY.md` and the smallest relevant indexes.
3. Use [assets/feature-plan-template.md](assets/feature-plan-template.md) as
   the starting shape. Do not treat an existing plan as a template or source
   of truth. Read an existing plan only when the user explicitly asks to
   update, extend, or reconcile it.
4. Read the relevant ADRs, architecture memory, callers, public contracts, and
   tests. Do not load unrelated ticket history.
5. State the problem, desired outcome, current evidence, scope, exclusions,
   and durable decisions. Resolve terminology before proposing interfaces.
6. Draw the ownership and data-flow boundaries in prose or a small Mermaid
   diagram when relationships are easier to understand visually. Keep provider
   details behind adapters and keep native and connector paths on the shared
   event/session contracts.
7. Compare materially different implementation shapes when the feature crosses
   crate or subsystem boundaries. Record the chosen seam, rejected option, and
   reason. Do not turn unresolved implementation questions into placeholders;
   make a best-judgment assumption or identify a concrete decision owner.
8. Organize implementation phases in dependency order. Each phase must name
   affected files or crates, behavior, tests, and an exit condition.
9. Include acceptance criteria, verification commands, risks, security or
   compatibility constraints, and a completion condition.
10. Load [Markdown](../write/markdown/SKILL.md) and save the canonical plan as
    `.agents/plans/<name>.md`. Create or update `<name>.html` only when a paired
    presentation is requested or the plan being updated already has one.
    For that branch, load [HTML](../write/html/SKILL.md) for shared design
    assets and browser verification. Both files use the plan's outline.
11. Run the relevant validation for the plan and report the file, decisions
    locked, validation performed, and unresolved risks.

## Plan requirements

- A feature plan is a product and architecture proposal, not a ticket plan.
- Do not allocate a TKT, add ticket frontmatter, or place the canonical plan
  under `.agents/tasks/` unless the user explicitly asks for ticketing.
- Preserve accepted ADR decisions. If the proposal changes a durable rule,
  mark the ADR that must be added or amended before implementation.
- Keep secrets, credentials, and real prompt content out of examples and
  fixtures.
- Separate native provider control from external connector control. A
  connector's model, effort, permissions, and lifecycle remain its own unless
  a documented connector protocol exposes them.
- Prefer Rust and existing workspace crates. Do not introduce a new crate,
  frontend, protocol, or dependency without stating why the current seams are
  insufficient.
- Use plain technical prose. The `write` skill owns the prose pass and its
  completion criteria.

## Verification

- Confirm the plan is in `.agents/plans/` and has no ticket-only frontmatter.
- Check every referenced path exists or is explicitly marked as a proposed new
  path.
- Check that phases have dependency order and exit conditions.
- Distinguish accepted decisions, proposed decisions, assumptions, and open
  questions. A proposal is not an accepted architectural rule.
- When paired, run the shared Markdown/HTML parity and presentation checks.
- Run `./.agents/gritt-agent ticket validate` when the plan changes durable
  project terminology or references ticket and memory artifacts.
- If the skill itself changed, run `./.agents/gritt-agent skill audit --skill
  write-plan` and then synchronize generated skill adapters.

## Output

Report the canonical plan path, decisions locked, files or subsystems in
scope, validation performed, and unresolved follow-up.
