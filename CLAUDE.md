# Claude Entry Point

`AGENTS.md` is the canonical boot router for this repository. Read it first
and follow its routing rules.

Authority after `AGENTS.md`:

1. Package `README.md` files for layer conventions
2. `.agents/memory/` for durable project knowledge
3. `.agents/brain/` for local agent infrastructure
4. `.agents/skills/` for reusable procedures
5. `.agents/tasks/` for ticket history

Keep canonical knowledge in `.agents/`; `.claude/` contains generated
compatibility configuration and skill stubs.
