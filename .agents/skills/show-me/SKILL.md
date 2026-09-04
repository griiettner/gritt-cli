---
name: show-me
description: Explains a repository topic with the smallest useful diagram or artifact. Use when a code shape, flow, state change, or comparison is easier to understand visually.
disable-model-invocation: false
---

# Show me

Skip the preamble. Pick one visual that answers the user's current question.
Keep the accompanying prose short and put it next to the visual.

## Choose a view

- Pseudocode for an algorithm or decision rule.
- A call tree for runtime control flow.
- A shallow file tree for ownership or layout.
- A component tree for UI structure and module boundaries.
- Mermaid for sequence, state, or data flow.
- A diff when the point is what changes.
- One focused HTML artifact when the state, layout, or comparison is too dense
  for text. Use the repository's existing document design system and real
  labels. Support desktop and mobile, then give the path to the artifact.

## Rules

- Include only the calls, files, states, props, and boundaries that matter.
- Label paths and ownership when they clarify the explanation.
- Use a full block when omitted context would hide order or responsibility.
- Do not create HTML for a relationship a ten-line text diagram explains.
- Do not invent metrics, components, or runtime behavior. Mark inferences.

## Output

Return the visual first, followed by at most a few sentences explaining the
point it makes and any uncertainty.
