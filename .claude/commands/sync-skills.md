# /sync-skills

Regenerate the per-tool skill adapters from the canonical `.agents/skills/` tree.

Run:

```bash
node .agents/tools/agent-tools/sync-skills.mjs
```

This regenerates both adapters:

- Claude discovery stubs at `.claude/skills/<name>/SKILL.md`
- the `policy:` block of each skill's `.agents/skills/<name>/agents/openai.yaml` (Codex)

Use this command when:

- a canonical skill was added
- a canonical skill was removed
- a canonical skill frontmatter block changed
- Claude Code is not seeing the latest project skill metadata

To verify without writing (useful in CI or a pre-commit hook):

```bash
node .agents/tools/agent-tools/sync-skills.mjs --check
```

Do not edit `.claude/skills/` files manually. They are generated compatibility stubs.
Do not hand-edit the `policy:` block of an `openai.yaml`; it is derived from `SKILL.md`.
