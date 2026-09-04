---
name: writing-for-agents
description: Writes concise, discoverable instructions for agents. Use when creating or editing skills, AGENTS.md, memory, tickets, or documents reached by an agent pointer.
disable-model-invocation: false
---

# Writing for agents

Treat every description and link as a context pointer. It must say what the
target contains and when the agent should read it.

## Rules

- Keep always-loaded identity and universal rules short.
- Put branch-specific instructions behind precise triggers.
- Use one trigger per branch. Remove synonym lists that do not add a branch.
- Keep ordered steps in the main file and long reference material behind local
  pointers.
- End every step with a checkable completion criterion.
- Keep each rule in one source of truth. Prefer the environment for commands and
  code patterns that are easy to discover.
- Use positive target behavior. Keep prohibitions only as hard safety guards.

## Review

Read the target file as a new agent. For every pointer, answer what it is, when
it fires, and whether the destination exists. Run `skill audit` for skills and
the applicable ticket or memory validation for other artifacts.

## Output

Return the edited artifact, context-load changes, pointers added or removed,
completion criteria, and validation results.
