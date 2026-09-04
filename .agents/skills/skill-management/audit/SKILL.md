---
name: skill-management-audit
description: Audits a skill against skill-management rules. Use when reviewing, tightening, or checking a skill before handoff.
---

# Skill audit

Read [skill-management](../SKILL.md) first. Load `write`. Report only unless the user asks to apply fixes.

## Input

A skill folder, a nested `SKILL.md`, or a family (`dev`, `tkt`, `write`, `write-docs`). If missing, ask once.

Canonical path only: `.agents/skills/<name>/`. Do not audit `.claude/skills/` stubs.

## Sequence

1. List files: parent `SKILL.md`, nested `*/SKILL.md`, `index.yaml`, `agents/openai.yaml`.
2. Score each file against the checks below.
3. Report. Do not edit until the user says apply.

## Checks

### Frontmatter

- `name:` matches the folder. Nested files use parent prefix, e.g. `dev-cli`.
- `description:` one line, `[Action]. Use when [trigger].` No filler ("helps", "allows", "designed to").

### Shape

- Procedure: steps, rules, verification. Cut examples, pep, duplicated policy.
- Markdown `##` sections. XML body tags (`<purpose>`, `<workflow>`) are non-conformant.
- Title-case headings are wrong. Sentence case.
- Nested files are not separately invocable. No `/parent/topic` paths.
- Long parents are routers: how to choose, default load sets, sub-skill table. Topics live in `<topic>/SKILL.md`.
- `index.yaml` exists when there are nested files. `SKILL.md` is canonical.

### Duplication

- Shared rules live on the family parent (`dev`, `tkt`, `write`), not copied into every workflow skill.
- Workflow skills name the parent as the first dependency and do not restate the parent rules.

### Policy

- `agents/openai.yaml` is display metadata and invocation policy only.
- `allow_implicit_invocation: false` unless the skill is safe to auto-apply.
- Writes that mutate git remotes or production stay false.

### Writing

- Skill body and description pass `write`.

## Severity

- Critical: wrong path, stub edited, invocable nested skill, missing parent-first load on a family workflow, implicit-invoke on a write skill.
- Warning: filler description, duplicated policy, missing `index.yaml` on a split skill, examples or pep left in.
- Advisory: title-case headings, extra length, `write` tells that do not change meaning.

## Report

```
## Skill audit: <name>

| File | Bucket | Severity | Issue |
| --- | --- | --- | --- |
| path | Conformant / Non-conformant | Critical / Warning / Advisory | one line |

Next: apply listed fixes, or stop.
```

Do not rewrite until the user confirms the list.
