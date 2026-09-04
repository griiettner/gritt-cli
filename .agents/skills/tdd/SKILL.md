---
name: tdd
description: Drives behavior-first red-green-refactor implementation. Use when building a feature or fix test-first, discussing test seams, or writing integration tests.
---

# Tdd

Read the applicable development sub-skill and memory before choosing a seam.

## Workflow

1. Name the public interface and the one behavior slice under test.
2. Write one test from an independent expected result and run it red.
3. Implement only enough code to make that test green.
4. Refactor only after green, preserving the public behavior.
5. Repeat in vertical slices. Add integration coverage at boundaries that
   matter, not for private implementation details.

## Rules

- Test behavior through public interfaces, not private helpers or incidental
  storage.
- Expected values come from a specification, fixture, or worked example, not a
  restatement of the implementation.
- Do not weaken a production contract to make tests or mocks convenient.
- Keep test names readable as behavior statements.

## Completion criteria

- The first test was observed failing for the intended reason.
- The minimal implementation passed it before refactoring.
- Relevant unit, integration, and repository checks pass.

## Output

Return the seam, red test result, implementation slice, refactor summary, and
validation commands with results.
