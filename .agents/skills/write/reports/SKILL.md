---
name: write-reports
description: Tightens a finding, review comment, or completion report into direct, evidence-first prose. Use when writing a review finding, a PR comment, or report.md.
---

# Reports

Read [write](../SKILL.md) first.

## Lead with the claim

State the defect, decision, or verdict first. Evidence and reasoning follow it, never precede it.

## Cite, do not restate

Point at `file:line`, a diff hunk, or a ticket's exact wording. Do not paraphrase back content the reader can already see.

## Size to severity

A no-op or low item is one line, table row only. A high or critical item gets exactly the sentences needed for cause, evidence, and fix. No scene-setting, no "as part of this review."

## Cut

- The setup sentence that only restates the trigger ("As requested, I reviewed...").
- A hedge on a checked fact ("it appears that", "seems to"). State it, or mark it PLAUSIBLE per [review](../../review/SKILL.md); nothing in between.
- A closing summary that repeats the table above it.

## Example

**Before:**
> As part of this review, I looked closely at the authentication flow. It's worth noting that there could potentially be an issue with how the token is being validated, which might possibly lead to a security concern in certain edge cases.

**After:**
> `auth/token.rs:42`: the expiry check compares against the wrong clock, so an expired token still validates. High: any request after a clock reset passes.
