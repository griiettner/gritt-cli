---
name: review-spec
description: Reviews a change against its originating ticket or specification. Use as the specification axis of a two-axis review.
---

# Specification review

Read the originating ticket, plan, acceptance criteria, and relevant user
request before reviewing the diff from a fixed point.

## Check

- every acceptance criterion is implemented
- scope and out-of-scope boundaries are honored
- behavior matches the intended user outcome
- failure and edge cases are covered
- validation proves the requested behavior
- no unrecorded assumptions change the result

Report missing or incorrect behavior with file and line evidence. Distinguish
partial implementation from a true defect and state when the axis passes.

## Output

Return acceptance evidence, findings, validation performed, and a specification
verdict.
