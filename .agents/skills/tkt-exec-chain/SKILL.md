---
name: tkt-exec-chain
description: Runs a ticket as a PM, worker, and reviewer chain with one worker at a time. Use when a TKT-NNNN must ship as sequenced branch and PR work instead of a one-shot run, or on /tkt-exec-chain.
disable-model-invocation: true
---

# /tkt-exec-chain

Read [tkt](../tkt/SKILL.md) first, then [tkt/autonomy](../tkt/autonomy/SKILL.md). Load [`write`](../write/SKILL.md) on ticket artifacts.

This skill extends `tkt` for tickets that must run as a controlled PM -> worker -> reviewer sequence.

For a one-shot ticket run use [`tkt-exec`](../tkt-exec/SKILL.md) instead.

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
- close the loop only after the final reviewer pass.

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
- create a fresh feature branch for that subtask;
- keep changes inside the assigned scope;
- update ticket artifacts needed for review evidence;
- commit, push, and open a PR;
- report back to the PM with branch name, PR link, validation run, and
  unresolved risks;
- stop after handoff; do not start the next subtask.

### Reviewer

The reviewer runs after every worker handoff, not only at the end.

Reviewer responsibilities:

- run the deterministic chain validator first;
- review the worker PR for scope drift, regressions, missing validation, and
  branch hygiene;
- run ticket-specific verification and benchmark steps when required;
- report findings back to the PM as pass / needs-fix;
- after the final subtask, perform one last cross-step pass for conflicts,
  integration gaps, and sequence-level issues.

## Chain Rules

1. One active worker at a time.
2. Every worker starts from a freshly updated base branch.
3. Every worker uses a new feature branch.
4. Every worker opens a separate PR.
5. Every worker PR is reviewed before the PM advances the chain.
6. The next worker does not start until the prior PR is merged.
7. Do not wait for CI/CD if this chain's policy says CI is unreliable or out
   of quota; rely on explicit reviewer validation instead.
8. The PM must record reviewer findings and route fixes before moving on.

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

Current default: if no later process decision overrides it, use `main` as the
base branch.

## Worker Sequence

For each worker step:

1. Check out the configured base branch.
2. Pull the latest remote state for that base branch.
3. Create a fresh feature branch for the subtask.
4. Execute only the assigned scope.
5. Run the required validation from the ticket.
6. Commit the changes.
7. Push the branch.
8. Open a PR against the configured base branch.
9. Hand off to reviewer.
10. Wait for PM instruction after review.

Never continue working on a previous worker branch after merge. The next worker
must restart from the updated base branch.

## Reviewer Sequence

For each worker PR, run this tool first:

```bash
node .agents/tools/agent-tools/tkt-chain-check.mjs --ticket TKT-NNNN --base main
```

Add `--require-report` when the ticket policy requires a report before review.
Add `--require-benchmark` when benchmark evidence is mandatory for that step.

Use the tool output as the deterministic gate before semantic review.

For each worker PR, the reviewer then checks:

- branch was created from the correct base branch;
- scope matches the assigned subtask;
- validation was run or honestly reported as not run;
- benchmarks were executed when the task required them;
- no unrelated files were changed without explanation;
- no obvious conflict was introduced for later chain steps.

Reviewer output to PM should be short and typed:

- `pass`
- `needs-fix`
- `blocked`

Include concrete reasons and the next action.

## Final Reviewer Pass

After the last worker PR is merged, run one final reviewer pass for:

- integration conflicts across merged steps;
- missing follow-up validation;
- benchmark summary completeness;
- ticket completion readiness.

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

## Boundaries

- This skill extends ticket execution; it does not replace the `tkt` parent.
- Do not use it for small one-shot chores.
- Do not let workers self-schedule the next step.
- Do not merge a PR and continue on the same branch for the next subtask.
- Do not treat CI status as the gate if the task policy explicitly says not to
  wait for CI.
