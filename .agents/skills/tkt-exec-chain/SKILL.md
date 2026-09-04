---
name: tkt-exec-chain
description: Runs a ticket as a PM, worker, and reviewer chain with one worker at a time. Use when a TKT-NNNN must ship as sequenced branch and PR work instead of a one-shot run, or on /tkt-exec-chain.
disable-model-invocation: true
---

# /tkt-exec-chain

Read [tkt](../tkt/SKILL.md) first, then [tkt/autonomy](../tkt/autonomy/SKILL.md). Review: [review/ticket](../review/ticket/SKILL.md) and [review/impact](../review/impact/SKILL.md). Load [`write`](../write/SKILL.md) on ticket artifacts.

This skill extends `tkt` for tickets that must run as a controlled PM -> worker -> reviewer sequence.

For a one-shot ticket run use [`tkt-exec`](../tkt-exec/SKILL.md) instead.

## Non-negotiable delivery contract

A chain is not complete when code exists or a PR is open. Every worker step
must run in a new worktree and feature branch, commit its changes, push the
branch, open a PR, pass review, and merge that PR before the next step starts.
The chain must continue through the final reviewer and merged result. Stopping
after implementation, leaving work on the current branch, opening an unmerged
PR, or asking the user whether to merge is a chain-execution failure.

The only valid reason to pause is a real external blocker such as missing
credentials, unavailable GitHub access, an explicit permission denial, or a
merge conflict that requires information unavailable to the agent. State the
exact blocker and recovery action. “This is safer,” “the user should merge,”
“CI is inconvenient,” or “the work is done” are not blockers.

## Goal and autonomy

Every role in the chain follows [tkt/autonomy](../tkt/autonomy/SKILL.md):

- The PM states the chain goal before dispatching the first worker, and completes it only after the final reviewer pass.
- The PM decides sequencing and decomposition from `task.md` and `plan.md`. It does not ask the user to re-plan work the ticket already specifies.
- A worker resolves ambiguity inside its subtask with best judgement, records the assumption in the PR and ticket artifacts, and hands off. It does not stop to ask the user or the PM for a preference.
- The reviewer returns `pass`, `needs-fix`, or `blocked` with reasons. `blocked` is for the stop conditions in `tkt/autonomy`, not for open questions the ticket already answers.
- Judgement calls surface once, in the report and the final handoff.

## Purpose

Use this skill when the work must be split across agents in sequence and branch
discipline matters more than speed.

This skill exists to prevent:

- reusing stale feature branches;
- stacking unrelated changes on the same branch;
- opening PRs from stale base state;
- merging worker output without immediate review;
- starting the next worker before the previous change is merged.

## Roles

### PM / Orchestrator

The PM owns sequencing, ticket decomposition, and handoff discipline.

PM responsibilities:

- gather the objective and convert it into ordered subtasks;
- decide whether the ticket stays as one chain or requires child tickets;
- choose the base branch for the chain;
- dispatch exactly one worker at a time;
- wait for reviewer verdict before advancing the chain;
- merge every worker PR immediately once the reviewer verdict is `pass`, without
  asking the user for confirmation first — this is standing autonomy for the
  chain, not a per-PR judgement call. Do not stop to ask whether to merge.
- route reviewer findings back into the next worker step;
- close the loop only after every worker PR is merged and the final reviewer
  pass confirms the integrated result;
- treat any worker without a merged PR as incomplete and route it back to
  execution instead of reporting partial completion.

If a merge attempt is blocked by a permission or classifier layer outside the
skill (for example a harness auto-mode safety classifier), that is a tooling
limit, not a signal to ask the user for approval-in-the-moment. Report the
block once, tell the user exactly what settings change or manual action
clears it, and continue any other chain-independent work while waiting. Do
not re-ask the merge question once the user has confirmed the policy.

The PM must not let later workers start from stale code.

Current repo default:

- use `main` as the base branch unless a later recorded process decision says
  otherwise.

### Worker

The worker performs one scoped subtask only.

Worker responsibilities:

- sync from the configured base branch before doing work;
- create a fresh worktree and feature branch for that subtask;
- keep changes inside the assigned scope;
- update ticket artifacts needed for review evidence;
- commit, push, and open a PR;
- merge its approved PR, or return the exact external blocker to the PM;
- report back to the PM with branch name, PR link, validation run, and
  unresolved risks;
- stop after handoff; do not start the next subtask.

### Reviewer

The reviewer runs after every worker handoff, not only at the end.

Reviewer responsibilities:

- run the deterministic chain validator first;
- load [review/ticket](../review/ticket/SKILL.md) against the worker ticket's `task.md` for scope drift and contract compliance, and [review/impact](../review/impact/SKILL.md) over the worker's diff for regressions;
- run ticket-specific verification and benchmark steps when required;
- report findings back to the PM as pass / needs-fix;
- after the final subtask, perform one last cross-step pass for conflicts,
  integration gaps, and sequence-level issues.

## Chain Rules

1. One active worker at a time.
2. Every worker starts from a freshly updated base branch in a new worktree.
3. Every worker uses a new feature branch. Reusing the current checkout or a
   prior worker branch is prohibited.
4. Every worker commits its scoped changes before opening a PR.
5. Every worker opens a separate PR against the configured base branch.
6. Every worker PR is reviewed before the PM advances the chain.
7. The PM merges every approved worker PR before the next worker starts.
8. The next worker starts from the updated base branch and a new worktree.
9. Do not wait for CI/CD if this chain's policy says CI is unreliable or out
   of quota; rely on explicit reviewer validation instead.
10. The PM must record reviewer findings and route fixes before moving on.
11. The final reviewer pass and merged integrated state are required for
    completion. A partial chain is not a successful chain.

## Required Inputs

Before using this skill, make these explicit in the task or PM handoff:

- ticket id (`TKT-NNNN`);
- base branch;
- branch naming pattern;
- merge policy;
- whether child tickets are allowed or all work stays under one ticket;
- validation required on every PR;
- benchmark steps, if any;
- final completion condition.

If any input is missing, the chain ticket was written too weakly. Return to
ticket creation or planning and resolve it there. Do not turn execution into a
requirements interview.

Current default: if no later process decision overrides it, use `main` as the
base branch.

## Worker Sequence

For each worker step:

1. Pull the latest remote state for the configured base branch.
2. Create a new worktree and feature branch for the subtask:
   `git worktree add ../<repo>-<step> -b <branch> <base>`.
3. Execute only the assigned scope inside that worktree.
4. Run the required validation from the ticket.
5. Commit the changes in the worker worktree.
6. Push the branch and open a PR against the configured base branch.
7. Hand off to the reviewer with the commit, branch, worktree, PR, and
   validation evidence.
8. Resolve every review finding in the same worker branch, commit and push the
   fix, and repeat review until `pass`.
9. Merge the approved PR. Confirm the merge commit or merged state.
10. Remove the worker worktree only after the merge is confirmed.
11. Report the merged result to the PM, which may then start the next step.

Never continue working on a previous worker branch after merge. The next worker
must restart from the updated base branch.

The worker handoff is not the end of the step. Before marking a step complete,
verify the PR state with `gh pr view <number> --json state,mergeCommit` and
require `state: MERGED`. An open, approved, closed-without-merge, or locally
committed branch is incomplete and must remain active.

## Reviewer Sequence

For each worker PR, run this tool first:

```bash
.agents/cli/target/release/gritt-agent ticket chain-check --ticket TKT-NNNN --base main
```

Add `--require-report` when the ticket policy requires a report before review.
Add `--require-benchmark` when benchmark evidence is mandatory for that step.

Use the tool output as the deterministic gate before semantic review.

Then run the semantic review: [review/ticket](../review/ticket/SKILL.md) against the worker ticket's `task.md` covers scope drift, unrelated files, and acceptance criteria; [review/impact](../review/impact/SKILL.md) over the worker's diff covers regressions. On top of both, check what neither one knows about the chain:

- branch was created from the correct base branch;
- a new worktree was used and removed only after merge;
- a commit exists on the worker branch;
- the PR is merged, not merely opened or approved;
- validation was run or honestly reported as not run;
- benchmarks were executed when the task required them;
- no obvious conflict was introduced for later chain steps.

Reviewer output to PM should be short and typed:

- `pass`
- `needs-fix`
- `blocked`

Include concrete reasons and the next action.

## Final Reviewer Pass

After the last worker PR is merged, run one final reviewer pass. Load [review/ticket](../review/ticket/SKILL.md) against the orchestrator's own `task.md` for completion readiness, and [review/impact](../review/impact/SKILL.md) across the full merged diff for integration conflicts. Add:

- missing follow-up validation;
- benchmark summary completeness.

This final pass does not replace the per-step reviews.

## Ticket Hygiene

Use the standard artifacts from [tkt/artifacts](../tkt/artifacts/SKILL.md).

At minimum, preserve:

- `task.md` as the execution contract;
- `report.md` with sequence summary, validation, and completion gate;
- `updates/YYYY-MM-DD-<slug>.md` for reviewer-driven fix rounds when useful.

Record chain-specific facts in the report:

- base branch used;
- ordered worker steps;
- PR links or identifiers;
- reviewer verdicts;
- benchmarks run;
- final unresolved risks.

Also record, for every worker, the worktree path, branch, commit, PR, merge
commit or merged state, reviewer verdict, and any fix round.

The completion table is a hard gate:

```text
new worktree -> new branch -> scoped changes -> validation -> commit
-> push -> PR -> review pass -> merge -> confirm merged state
```

No arrow may be skipped, and the next worker may not begin until the merge
confirmation is recorded.

## Boundaries

- This skill extends ticket execution; it does not replace the `tkt` parent.
- Do not use it for small one-shot chores.
- Do not let workers self-schedule the next step.
- Do not merge a PR and continue on the same branch for the next subtask.
- Do not treat CI status as the gate if the task policy explicitly says not to
  wait for CI.

## Output

Report each chain step, branch and PR state, reviewer result, and the next
permitted lifecycle transition.
