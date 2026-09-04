---
name: domain-modeling
description: Sharpens project terminology against code and durable decisions. Use when concepts, names, or relationships are ambiguous or a shared glossary needs updating.
---

# Domain modeling

Use the repository's existing memory and ADR routing. Do not introduce a second
glossary store without an architectural decision.

## Workflow

1. Find the current definition in memory, tickets, ADRs, docs, and code.
2. Identify overloaded, conflicting, or vague terms. Propose one canonical term
   and state what it excludes.
3. Stress-test relationships with concrete edge cases and lifecycle scenarios.
4. Check the proposed language against public types, event names, paths, and
   user-visible text. Surface contradictions.
5. Record only durable resolved language in the closest memory or ADR file. Keep
   implementation details in code and ticket artifacts.

## Completion criteria

- Canonical terms and exclusions are explicit.
- At least one realistic edge case tests each important relationship.
- Code and durable documentation do not silently use conflicting meanings.

## Output

Return the terminology table, evidence paths, contradictions found, decisions
recorded, and unresolved language.
