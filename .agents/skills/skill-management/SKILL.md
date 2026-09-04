---
name: skill-management
description: Manages project skills. Use when creating, editing, splitting, renaming, or auditing a skill.
---

# Skill management

Canonical skills live at `.agents/skills/<skill-name>/SKILL.md`. Do not edit `.claude/skills/` stubs.

## Sub-skills

Nested under `skill-management/`. Not separately invocable. Load on demand:

| Sub-skill | Load when |
| --- | --- |
| [audit](audit/SKILL.md) | Reviewing or tightening a skill against these rules |

Routing metadata: [`index.yaml`](index.yaml).

## Frontmatter

One line. Action, then trigger. No filler ("helps", "allows", "designed to"). State each once: do not restate the trigger inside the action, and do not list near-synonyms where one word covers it.

```yaml
---
name: kebab-name
description: [Action]. Use when [exact trigger].
---
```

Example: `Formats git commit messages. Use when the user asks to create or edit a commit message.`

## Create

1. Pick a lowercase kebab-case name.
2. Write the description in the form above.
3. Scaffold:

```bash
node .agents/tools/agent-tools/create-skill.mjs <skill-name> "<description>"
```

4. Replace the starter body with the real procedure. Keep it short: steps, rules, verification.
5. Sync.

## Edit

1. Open the canonical `SKILL.md` (and nested sub-skills if needed).
2. Change frontmatter and body in place with targeted edits. Keep the description in the form above. Rewrite the whole file only when most of it is changing.
3. Sync.

Do not rewrite a skill by creating a second folder with a new name unless the user asked to rename it.

## Skill vs sub-skill

Two different things. Do not convert one into the other without being asked.

| | Skill | Sub-skill |
| --- | --- | --- |
| Path | `.agents/skills/<name>/SKILL.md` | `.agents/skills/<skill>/<topic>/SKILL.md` |
| Registered by `skill sync` | Yes — Claude stub + Codex `openai.yaml` | No |
| Invocable | Yes, `/<name>` | No — its parent loads it |
| `name:` | matches the folder | parent prefix, e.g. `dev-cli` |

There is no path invocation. `/ado/defaults` does not work; sub-skills are reached only by the parent router.

## Split a long skill

If the body is too long to load every time:

1. Keep the parent `SKILL.md` as a router: how to choose, default load sets, table of sub-skills.
2. Put each topic in `<topic>/SKILL.md` with its own frontmatter and a `<skill>-<topic>` name.
3. Add `index.yaml` as a routing aid. The `SKILL.md` files are canonical.
4. Load 1–3 sub-skills per task. Do not load all of them by default.

```yaml
subskills:
  - id: skill-topic
    title: Topic
    file: topic/SKILL.md
    tags:
      - topic
    read_when:
      - the exact trigger for this file
```

Promote a sub-skill to a top-level skill only when the user asks for it to be invocable on its own.

## Rename or remove

- Rename: move the folder, set `name` in frontmatter to match, then sync.
- Remove: delete `.agents/skills/<skill-name>/`, then sync.

## Sync

After any skill change:

```bash
.agents/cli/target/release/gritt-agent skill sync
```

## Rules

- Procedure only. Cut examples, pep talks, and duplicated policy.
- Markdown `##` sections only. Do not wrap the body in XML tags (`<purpose>`, `<workflow>`).
- - Load `write` when writing or editing a skill body or description.
- `agents/openai.yaml` is display metadata and invocation policy only.
- Set `allow_implicit_invocation: false` unless the skill is safe to auto-apply.
- After writing or editing a skill, load [audit](audit/SKILL.md) if the user asked to review it, or before calling the work done.
