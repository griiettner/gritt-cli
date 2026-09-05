---
name: write-docs
description: Writes documentation of existing systems and processes. Use when the user requests documentation or updates to it.
disable-model-invocation: true
---

# Write docs

Read [write](../write/SKILL.md) first for the prose workflow.
This skill owns documentation of current behavior. Feature plans and proposals
belong to `write-plan`; do not apply a documentation outline to them.

## Workflow

1. Establish the topic, audience, and destination from the request and existing
   files. Default new documentation to `docs/<slug>/<slug>.md`, with a sibling
   HTML companion unless the user requests Markdown only. Preserve requested
   paths and formats.
2. Query local memory using the repository's routing instructions, then read
   the relevant code, contracts, and tests. Separate implemented behavior from
   limitations; do not present planned behavior as available.
3. Build an outline around what the reader needs to understand or do. Include
   setup, usage, examples, reference details, and troubleshooting only where
   relevant. Preserve the structure of existing documentation unless the
   requested change requires reorganizing it.
4. Load [Markdown](../write/markdown/SKILL.md) to author the canonical text.
   When an HTML companion is needed, load [HTML](../write/html/SKILL.md) to
   present the same content using the shared design assets.
5. Complete the prose pass from `write` on new drafts or changed passages.
   Verify commands and claims against their sources, and perform the shared
   presentation checks for the requested formats.

## Verification

- Documentation describes current behavior with evidence for commands and
  factual claims.
- The files exist at the requested paths and local links resolve.
- When paired, Markdown and HTML carry the same claims and sourced numbers.
- HTML passes the shared browser checks; report any unavailable checks.

## Output

Report document paths, behavior documented, validation performed, and any
remaining factual or presentation gaps.
