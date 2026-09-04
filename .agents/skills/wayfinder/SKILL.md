---
name: wayfinder
description: Decomposes large work into decision-complete dependency tickets. Use when a plan spans multiple sessions, owners, branches, or architectural decisions.
disable-model-invocation: true
---

# Wayfinder

Read the repository plan, memory, ticket indexes, and existing chains before
creating work. Use the ticket CLI for allocation; never choose ids manually.

## Workflow

1. Define the destination, non-goals, and measurable completion state.
2. Identify decisions that block implementation and separate them from work
   that can proceed independently.
3. Draw dependency edges and order tickets as tracer bullets. Keep each ticket
   small enough for one review.
4. Assign ownership, scope, validation, and handoff artifacts. Prefer existing
   ticket chains when their worker/reviewer structure fits.
5. Create tickets only after the map is decision-complete, then sync and
   validate indexes.
6. Revisit the map when a decision changes. Do not silently rewrite downstream
   scope.

## Completion criteria

- Every ticket has a clear goal, scope, acceptance criteria, and dependency.
- No worker is blocked by an unrecorded decision.
- The next executable ticket is unambiguous.

## Output

Return the destination, ticket graph, dependency edges, critical path, next
ticket, and unresolved decisions.
