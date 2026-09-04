# Claude Compatibility Layer

This directory exists for Claude-specific project discovery.

In this scaffold:

- canonical agent context lives in `AGENTS.md` and `.agents/`
- canonical skills live in `.agents/skills/`
- Claude Code compatibility is provided through generated stubs in `.claude/skills/`

Each generated Claude skill should:

- preserve the canonical skill frontmatter Claude uses for discovery
- point Claude to the real skill file in `.agents/skills/`
- avoid duplicating the full canonical skill body

Do not edit generated files under `.claude/skills/` manually.

The sync script is safe by default:

- generated stub files are refreshed
- stale generated stub directories are removed
- user-created Claude-only skill directories are preserved

Regenerate them with:

```bash
.agents/cli/target/release/gritt-agent skill sync
```

If you want a stricter mirror of canonical `.agents/skills/`, you can run:

```bash
.agents/cli/target/release/gritt-agent skill sync --prune
```

Or, from Claude Code, run:

```text
/sync-skills
```
