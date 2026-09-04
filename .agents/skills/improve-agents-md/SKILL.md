---
name: improve-agents-md
description: Refactors AGENTS.md into a short router with targeted relevance rules. Use when improving repository instructions, reducing context load, or fixing instruction-discovery problems.
disable-model-invocation: true
---

# Improve agents md

Treat `AGENTS.md` as the boot router, not the repository encyclopedia. Read the
current file, its referenced indexes, and the project structure before editing.

## Workflow

1. Keep one sentence of project identity, a short map, universal safety rules,
   and the ordered route to memory, tickets, skills, and validation.
2. Move domain guidance into the smallest relevant skill or memory file. Give
   each route a specific `read_when` condition.
3. Keep commands that agents need to discover, but remove rules that a
   formatter, linter, compiler, or existing code pattern already enforces.
4. Split broad rules into narrow conditional sections. Do not use a condition
   that applies to nearly every task for a domain-specific rule.
5. Replace copied examples with links to canonical files. Do not duplicate a
   rule in `AGENTS.md`, a skill, and memory.
6. Run `gritt-agent skill audit` for changed skills and the repository's normal
   validation commands. Re-read the resulting router as a first-time agent.

## Output

Report what stayed in the router, what moved, the new discovery path, and the
validation commands run. Call out any rule that remains broad by design.
