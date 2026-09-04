---
name: tkt
description: Holds shared ticket rules and routes to TKT sub-skills. Use when starting any TKT-NNNN work.
disable-model-invocation: true
---

# TKT skills

Common contract for every ticket workflow. **Every `tkt-*` skill that touches ticket content reads this file first**, then loads **only** the sub-skills that step needs. Do not read all sub-skills.

Keep the ticket system useful, not ceremonial.

## Common rules

- Ticket ids are `TKT-NNNN`, unique inside a namespace rather than across the tree. Resolution and allocation: [store](store/SKILL.md).
- Always allocate through the allocator tool. It enforces a contiguous namespace
  sequence and fails loudly if an earlier id is missing; never skip a number or
  manually choose a later id to work around a gap.
- Ticket folder contents are canonical. Every `index.yaml` is generated routing metadata.
- Never create new ticket context outside a chunk folder.
- Use the smallest artifact set the work needs. Do not create placeholders. See [artifacts](artifacts/SKILL.md).
- Answer the completion gate before calling a ticket done. See [completion](completion/SKILL.md).
- Execution runs to completion on best judgement instead of stopping to ask. Goal tracking and the narrow stop conditions: [autonomy](autonomy/SKILL.md).
- Load `write` when writing `task.md`, `concept.md`, `plan.md`, `report.md`, or an update file.

## Sub-skills

Nested under `tkt/`. Not separately invocable. Load on demand:

| Sub-skill | Load when |
| --- | --- |
| [store](store/SKILL.md) | Resolving a ticket id, namespace, or chunk path, or allocating a new id |
| [artifacts](artifacts/SKILL.md) | Choosing ticket mode, writing frontmatter, deciding which files to create |
| [autonomy](autonomy/SKILL.md) | Executing a ticket: goal tracking, best judgement, when to stop |
| [completion](completion/SKILL.md) | Closing a ticket: completion gate, report format, update files |
| [backlog](backlog/SKILL.md) | Parking, activating, or rejecting deferred work in `backlog.yaml` |

Routing metadata: [`index.yaml`](index.yaml).

## Workflow skills

Invocable on their own. Each one **starts by reading this file**. Load **one**:

| Skill | Use when |
| --- | --- |
| [`tkt-new`](../tkt-new/SKILL.md) | Recording new work as a ticket |
| [`tkt-plan`](../tkt-plan/SKILL.md) | An existing ticket needs a decision-complete `plan.md` |
| [`tkt-exec`](../tkt-exec/SKILL.md) | Implementing a named ticket |
| [`tkt-update`](../tkt-update/SKILL.md) | Follow-up work on a ticket that already has a report |
| [`tkt-new-chain`](../tkt-new-chain/SKILL.md) | Creating a chain-managed ticket |
| [`tkt-exec-chain`](../tkt-exec-chain/SKILL.md) | Running sequenced PM, worker, and reviewer steps |
| [`tkt-sync`](../tkt-sync/SKILL.md) | Regenerating skill adapters and ticket indexes |

## How to use

1. Read this file. It is mandatory for every `tkt-*` skill that reads or writes ticket content.
2. Load the matching workflow skill.
3. Open a sub-skill only when that skill's step calls for it.

`tkt-sync` is a tooling launcher and does not need this file.

## Maintenance

After ticket or memory files change:

```bash
node .agents/tools/agent-tools/tkt-sync.mjs
node .agents/tools/agent-tools/tkt-validate.mjs
```
