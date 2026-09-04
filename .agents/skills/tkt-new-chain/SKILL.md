---
name: tkt-new-chain
description: Creates a full orchestrator, worker, and reviewer ticket chain. Use only when the ticket must follow tkt-exec-chain rules, or on /tkt-new-chain.
disable-model-invocation: true
---

# /tkt-new-chain

Read [tkt](../tkt/SKILL.md) first, then [`tkt-exec-chain`](../tkt-exec-chain/SKILL.md). Allocation: [tkt/store](../tkt/store/SKILL.md). Load [`write`](../write/SKILL.md) on the ticket prose.

## A chain is never one ticket

This skill creates **one orchestrator ticket, one worker ticket per step, and one final reviewer ticket**. That is the deliverable. A single `task.md` with chain-shaped headings is not a chain, and returning one is a failure of this skill.

Decide the worker steps before running anything. Read the source plan, decompose it into steps that each fit one branch and one PR, then pass every step to the tool. Never emit an orchestrator whose steps live only in prose.

Use [`tkt-new`](../tkt-new/SKILL.md) instead when the work is a small localized change, one agent can finish it in one pass, or branch and PR sequencing would be ceremony rather than risk control. Do not use `tkt-new-chain` and then quietly collapse it to a single ticket.

## Scaffold

Use the tool. Do not hand-write folders, and do not pick ids by reading `index.yaml`:

```bash
.agents/cli/target/release/gritt-agent ticket new-chain \
  --title "Chain title" \
  --step "contract:Freeze the request and response contract" \
  --step "skeleton:Add the service and route skeleton" \
  --step "resolvers:Register the first resolvers" \
  --create-concept --create-plan
```

The tool allocates consecutive `TKT-NNNN` ids in the current GitHub-login namespace: orchestrator first, then one worker per `--step`, then the reviewer. It refuses to allocate if that namespace already contains a gap, rather than creating a later id that obscures the missing ticket. It wires `chain_role`, `chain_parent`, `chain_children`, and per-worker `dependencies`, writes the child chain table into the orchestrator, and runs `gritt-agent ticket sync` unless `--no-sync` is passed.

Flags worth knowing:

- `--step SLUG:TITLE` is repeatable and at least two are required.
- `--no-reviewer` drops the final reviewer ticket. Only pass it when a recorded decision says the chain does not need an integrated pass.
- `--dry-run` prints the planned chain and allocated ids without writing.

## Fill every scaffolded section

Every generated `TODO(tkt):` line is unfinished work, not a template you may leave in place. `gritt-agent ticket validate` **errors** while any remains, so a chain that stops at the scaffold fails validation instead of looking done.

Replace them with real content in the same session that created the chain:

- orchestrator goal, inputs, scope, out of scope, acceptance criteria, verification, validation per step, benchmark requirements, final completion condition;
- each worker goal, scope, out of scope, acceptance criteria, verification;
- reviewer architecture and behavior checks;
- `plan.md` step descriptions and resolved decisions. A chain may not enter
  execution with open material decisions.

The chain must not be handed to `tkt-exec-chain` while any material context is
missing. Resolve the objective, scope, dependencies, branch and merge policy,
validation, review gates, and final completion condition during ticket creation
or planning. “The worker can decide later” is not a substitute for context.
If the ticket cannot answer a question, record the explicit assumption and its
owner before execution starts.

Add `concept.md` when the chain needs user-problem framing, and `plan.md` when the PM must sequence subtasks or set review gates. `report.md` comes later, once execution produces history.

## Chain fields the orchestrator must carry

The tool writes the structural ones. You still own the judgement ones:

- ticket id and title, and that the ticket uses `tkt-exec-chain`;
- base branch (`main`, unless a later recorded process decision changes it);
- branch naming pattern and per-worker branch names;
- merge policy;
- reviewer required after every worker PR;
- the child chain, listed as links, with one worker per step;
- validation required on every worker step;
- benchmark requirements, if any;
- final completion condition.

## Verify before reporting

```bash
.agents/cli/target/release/gritt-agent ticket validate
```

A clean run means every chain ticket exists, the parent and child links agree, and no scaffold marker is left. Do not report the chain as created until this passes.

Before reporting readiness, read every generated `task.md` and `plan.md` as an
executor. Confirm that each worker can create its worktree, commit, open a PR,
and merge without asking a requirements question that belongs in the ticket.

## Output

Report the orchestrator id and path, the ordered worker ids with their branch
names, the reviewer id, and that the ticket is chain-managed under
`tkt-exec-chain`. Do not report unresolved process decisions as a normal output.
