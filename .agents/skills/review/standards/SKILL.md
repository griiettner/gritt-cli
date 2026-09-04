---
name: review-standards
description: Reviews a change against repository standards and design rules. Use as the standards axis of a two-axis review.
---

# Standards review

Read the repository router, applicable development skill, architecture memory,
and ADRs. Review the diff from a fixed point.

## Check

- crate and module boundaries
- provider-neutral and connector rules
- security, secret, and permission behavior
- error handling and cancellation
- tests at public seams
- formatting, lint, and repository conventions

Report only confirmed findings with file and line evidence. Rank severity by
impact, not effort. State when the axis has no findings.

## Output

Return findings, evidence, validation performed, and a standards verdict.
