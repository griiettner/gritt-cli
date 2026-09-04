---
name: review
description: Routes to the pr, impact, or ticket review sub-skill. Use when asked to review a PR, a diff, or a ticket, or on /review.
disable-model-invocation: true
---

# Review

Shared contract for reviewing code in this repository. **Read this file first**, then load the sub-skill, or skills, that match the target. Do not load all three by default.

Review finds and reports problems. It does not fix them unless the user explicitly asks for a follow-up fix pass.

## Sub-skills

Nested under `review/`. Not separately invocable. Load on demand:

| Sub-skill | Load when |
| --- | --- |
| [pr](pr/SKILL.md) | The target is a GitHub PR by number, URL, or "this pull request" |
| [impact](impact/SKILL.md) | The target is a diff, branch, or working-tree change with no PR or ticket framing |
| [ticket](ticket/SKILL.md) | The target is a `TKT-NNNN`'s implementation, including the self-review step inside `tkt-exec` |

A target can match more than one row. A PR that closes a ticket typically needs all three: `pr` for the defect audit of its own diff and posting mechanics, `impact` for what the change does to the rest of the codebase, and `ticket` for contract compliance. Load every row that applies; each answers a different question, so do not skip one assuming another covers it.

Routing metadata: [`index.yaml`](index.yaml).

## Verify before reporting

Treat every finding as a candidate until you have reread the actual current file content at the location it cites. Report a finding as CONFIRMED only after that check; otherwise mark it PLAUSIBLE and write "verify that ..." instead of asserting it. Do not report a finding from the diff hunk alone. Prefer what the current tree actually does over what the diff or its author seems to have intended.

## Stay in scope

A review is a defect audit, not a redesign exercise. Include a finding only when the change under review introduces, exposes, or materially worsens a concrete problem.

- Verify the problem against the change's intended behavior, existing callers, tests, and any explicit requirement, such as a ticket's acceptance criteria or a stated user ask. Unconventional behavior that is deliberate and works as requested is not a bug.
- Recommend the smallest safe correction that preserves the intended workflow. Do not turn a local fix into a new architecture, a migration, or a product-workflow change.
- Do not recommend removing a working feature merely because a different design would be cleaner in theory.
- Do not promote a separate ticket, memory note, or preferred architecture into a requirement for the change under review, unless it records a binding requirement for this change specifically.
- An existing off-diff defect is a finding only when this change newly activates or depends on it; otherwise omit it or note it as a non-blocking follow-up outside the findings list.
- If the only safe fix would materially change user-visible behavior, state the concrete risk and ask which behavior to preserve before recommending it.

Before reporting a finding, answer all four:

1. Introduced here: what in this change causes or exposes it?
2. Concrete impact: what reachable behavior fails, and for whom?
3. Evidence: which lines and which existing contract prove it?
4. Narrow fix: can it be corrected without redesigning the feature?

If any answer is missing, do not report it as an actionable finding. Use a lower severity, phrase it as a focused verification note, or omit it.

## Delegate and disclose

A review may be split across a subagent, a workflow, or a background or forked skill. When it is:

- Record whatever task or agent id the tool call returns.
- Before treating the review as finished, confirm every delegated piece actually reported back.
- If a delegated piece stalls, errors, or returns only partial findings, say so explicitly in the review's output and to the user before falling back to finishing it yourself. Silently substituting your own judgment for a review the user asked to run is a direction change, not a completion.

## How to use

1. Read this file.
2. Identify the target's shape (PR, ticket, or bare diff) and load the matching sub-skill or skills.
3. Resolve and review per that sub-skill's procedure.
4. Report findings ranked most severe first, with file:line pointers. Use the `ReportFindings` tool when the harness offers it; otherwise write the same ranked list as prose. Fold findings into a ticket's `report.md` instead when the sub-skill says to.

For a broad implementation review, run two independent axes before aggregating:
[standards](standards/SKILL.md) checks repository and design rules, while
[spec](spec/SKILL.md) checks the originating ticket or request. Keep their
findings separate until the final report so one perspective does not hide the
other.

## Output

Return ranked findings with file and line evidence, the verification performed,
and a clear verdict when no findings remain.

## Relationship to other review tools

This skill is the repository's own procedure for its three common review shapes. The harness may also offer a separate, more general review skill, for example a deep multi-agent or cloud review across an arbitrary diff. That one is complementary, not a replacement, and the delegation and disclosure rule above still applies when a `review` sub-skill uses it.
