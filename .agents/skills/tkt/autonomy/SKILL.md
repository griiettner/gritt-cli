---
name: tkt-autonomy
description: Sets goal tracking and no-questions autonomy for ticket execution. Use when executing a ticket or chain step that already has a task or plan.
---

# Execution autonomy

Read [tkt](../SKILL.md) first. This governs every execution skill: `tkt-exec` and `tkt-exec-chain`.

The ticket is the contract. `task.md` and `plan.md` exist to close the open questions before execution starts, so execution does not reopen them.

For a chain, the delivery contract includes a new worktree, branch, commit, PR,
review, and merge for every worker step. The agent owns carrying that sequence
through to the final integrated result. It must use best judgment on choices
already within the ticket's scope and must not invent a requirements pause to
avoid the delivery steps.

## Track the goal

1. Before the first edit, state the goal as one outcome with the qualified ticket id, for example `griiettner/TKT-0007: catalog cache falls back to the last good copy`. Use the harness goal or task tool when one exists; otherwise write it at the top of your working notes.
2. Keep the goal active for the whole run. Re-read it when a step lands, and pull the next step from the ticket rather than from the last thing that broke.
3. Mark the goal complete only after verification actually ran and the completion gate is answered. A partial, skipped, or unverified result leaves the goal active.
4. Never mark a goal complete to end a turn.

## Communicate

Before the first tool call, say in one sentence what you are about to do. While working, give a brief update only when you find something important or change direction. The final message leads with the outcome: the first sentence answers what happened or what was found, with supporting detail after it for readers who want it. Never run a command only to "show" output; if the user needs to read it, put it in the message.

## Decide, do not ask

Do not ask the user anything that `task.md`, `plan.md`, the repo, an ADR, or the package `README.md` already answers. Read first, then decide.

Resolve ambiguity with best judgement, in this order:

1. The ticket's acceptance criteria and recorded scope.
2. The existing pattern in the package being changed.
3. The relevant ADR or memory file.
4. The smallest reversible option.

Record the call as an assumption and keep going. Do not stop for confirmation.

These questions are noise during execution. Pick, note the choice, move on:

- "would you like me to..."
- "should I also..."
- "which approach do you prefer..."
- anything already settled in the plan.

If execution reveals missing context that should have been answered during
ticket creation, record the best-judgment assumption and continue when safe.
Only a real external blocker may pause the chain, and it must include a
concrete recovery action.

## Keep changes to the task

If, while working or testing, you find a pre-existing bug, a performance concern, or behavior the task does not mention, do not fix, optimize, or extend it in this change unless the requested behavior cannot work without it. Report it as a follow-up in `report.md`. Where the task is ambiguous, implement the reading its wording and the surrounding code most directly support, state that assumption, and do not build for the other readings as well. Verify however you like; scratch scripts and quick checks need not be kept. Commit tests only where the task asks for them or this repository already keeps tests for this kind of change, sized like the neighboring test files, roughly one focused test per stated behavior. Do not turn scratch checks into permanent test files. This is about extras only: implement every behavior the task asks for, completely.

## Before ending the turn

Check your last paragraph. If it is a plan, an analysis, a question, a list of next steps, or a promise about work you have not done ("I will...", "let me know when..."), do that work now with tool calls. That includes retrying after errors and gathering missing information yourself. Do not stop because the context or session is long. End the turn only when the task is complete or you are blocked on input only the user can provide.

## When to stop

Stop and ask only when continuing is unsafe or impossible:

- a required credential, connection, or external approval is missing;
- the work would delete data, mutate production, or bypass auth;
- the ticket contradicts itself or an ADR, and both readings change the delivered outcome;
- `task.md` is missing, or the ticket is `cancelled`.

## Flag at the end

Report judgement calls once, at the end, in `report.md` and in the final message:

- the assumption and why it was made;
- what a different choice would have changed;
- anything worth a follow-up ticket.

Do not scatter these through the run as questions.
