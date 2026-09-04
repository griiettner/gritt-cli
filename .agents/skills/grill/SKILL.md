---
name: grill
description: Resolves ambiguous plans and designs through a focused interview. Use when requirements, boundaries, or implementation choices are unclear before work is ticketed or built.
disable-model-invocation: true
---

# Grill

Read the repository, relevant memory, existing tickets, and validation commands
before asking questions. Bring evidence and recommended options.

## Workflow

1. State the decision or outcome being clarified.
2. Map the branches: behavior, users, data, boundaries, failure cases, and
   validation. Ask one focused question at a time.
3. Challenge vague terms with concrete scenarios and existing code. Identify
   contradictions instead of silently choosing between them.
4. Record each answer as a decision, assumption, or unresolved question.
5. Stop when the next agent can execute without reopening a material choice.
6. Write the result to the applicable ticket plan, ADR, or memory file. Do not
   create an ADR for a reversible or unsurprising choice.

## Completion criteria

- Every material branch has an answer or an explicit owner.
- Scope, exclusions, interfaces, risks, and validation are named.
- The chosen artifact is canonical and does not duplicate another source.

## Output

Return the decision summary, recorded artifact, open questions, assumptions, and
the exact next executable action.
