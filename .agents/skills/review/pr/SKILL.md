---
name: review-pr
description: Audits a PR's diff for defects and posts a line-anchored review. Use when asked to review a PR by number or URL.
---

# PR review

Read [review](../SKILL.md) first. Load [`write`](../../write/SKILL.md) when composing finding comments and the review body.

Audits a PR's own diff for concrete defects and, optionally, posts them back to GitHub as a single line-anchored review. This is not the blast-radius check: load [impact](../impact/SKILL.md) separately when the PR's effect on the rest of the codebase also matters, and [ticket](../ticket/SKILL.md) when it closes a `TKT-NNNN`.

## Required input

A PR number or URL. If neither is given and there is no single open PR on the current branch, stop and ask which PR; do not guess or default to "the latest one."

## Pin the exact code under review

Accuracy depends on reviewing the real head against the real merge base, not the last commit of a multi-commit PR.

```bash
gh pr view <n> --json number,title,headRefName,baseRefName,headRefOid,url,body,isDraft,reviewDecision,mergeable
git fetch origin pull/<n>/head:pr-<n>
git fetch origin <baseRefName>
BASE=$(git merge-base pr-<n> origin/<baseRefName>)
gh pr diff <n> --name-only
gh pr checks <n>
```

- Diff against `$BASE`, never `pr-<n>~1`; `gh pr diff <n>` already computes against the merge base for you and includes the hunk headers needed later for line anchoring.
- Each `git fetch` overwrites `FETCH_HEAD`; always use the named ref `pr-<n>` instead.
- Triage the file list before reading the full diff. Skip generated or vendored files (minified bundles, lockfiles, committed build output) as cosmetic, and note which ones were skipped. A large PR's diff can be too big to read in one pass; the file list tells you where to spend the read budget.
- Read each remaining source file in full at the PR head, `git --no-pager show pr-<n>:<path>`, and use the new-file line numbers shown in the diff consistently throughout.

If the title, body, or branch names a `TKT-NNNN`, also load [ticket](../ticket/SKILL.md). Resolve its folder through [tkt/store](../../tkt/store/SKILL.md) rather than deriving the chunk path by hand.

## What to look for

At minimum:

- Correctness bugs introduced or left uncovered: off-by-one, wrong null handling, race conditions.
- Behavior changes to a function's contract that callers may not handle, for example now erroring where it previously returned a default.
- Type safety: an unchecked type escape (`any` in TypeScript, an unjustified `unwrap()` or cast in Rust), a non-null or force-unwrap assertion.
- Security and privacy: secrets, tokens, or response bodies leaking into logs, error state, or the UI.
- Performance: unnecessary retries on a terminal failure, a fixed delay advertised as backoff, synchronous work on a hot path, redundant state writes.
- Dead or redundant code that existing code already handles.
- Caching pitfalls: missing TTL or version, stale entries surviving a deploy, key collisions.
- Doc and comment drift: a header comment, docstring, or the PR description that no longer matches the code.
- Scope reconciliation: compare the PR description against the actual diff. Does the claimed file list or file count match? Flag an undescribed change, an extra file, an unrelated refactor, a behavior change not mentioned, as a finding; these ship unreviewed precisely because the description hides them.
- Repro steps in the description: do the flags or files they reference actually exist on this branch?

Add PR mechanics on top: a failing or pending required check, a draft PR, an unresolved merge conflict, or a stale base branch changes what "ready to review" means, and is a finding, not background noise.

Apply [review](../SKILL.md)'s stay-in-scope gate before reporting each one. Anchor every finding to the smallest contiguous line range that captures the problem; when two issues share a span, write two separate findings.

## Severity

Every finding carries exactly one severity, rated on impact, not effort to fix:

- 🔴 Critical: guaranteed data loss or corruption, a security breach or auth bypass on a deployed path, a secret or PII leak, or a hard crash on the main flow. Should block merge.
- 🟠 High: a behavior regression, a security or visibility change, or a correctness bug that ships silently, untested and undocumented, on a real path. Fix before merge.
- 🟡 Medium: a maintainability regression, contract or doc drift, scope or description drift, or a bug on an edge path. Worth addressing, not gating.
- 🟢 Low: style, log hygiene, an anti-simplification note, a verify-only note, or an informational "no issue" confirmation.

When uncertain between two levels, pick the lower one and say why. Do not post a nit, for example a naming or formatting preference, unless the user explicitly asked for stylistic feedback.

## Report

Print each finding in this exact shape, one block per finding, grouped by file in diff order, nothing wrapped in a table:

```text
Severity: {severity}
File: {file}
Lines: {lines}
Comment: {comment}
```

`{lines}` is a single line or an inclusive range in new-file numbering. For a PR-level finding with no anchor, use `File: PR-level` and `Lines: n/a`. Be specific: name the symbol, quote the line, suggest the concrete fix when there is one.

## Offer to post

After printing the findings, ask whether to post them as a single GitHub review, and recommend an event based on the highest severity found: any 🔴 or 🟠 suggests `request-changes`; only 🟡 or 🟢 suggests `comment`; nothing actionable makes `approve` reasonable. State the recommendation; the user's choice always wins. If they decline, stop; the printed findings are the deliverable.

If they accept, build one `POST /repos/{owner}/{repo}/pulls/{n}/reviews` payload:

- `commit_id`: the PR's `headRefOid`.
- `event`: `COMMENT`, `REQUEST_CHANGES`, or `APPROVE` per the user's choice.
- `body`: a short summary line, plus a `## PR-level notes` section for any `File: PR-level` finding, each prefixed with its severity.
- `comments`: one entry per anchored finding, each with `path` and either `line` for a single line, or `start_line`, `start_side`, `line`, and `side` for a range with both sides matching. Use `side: "RIGHT"` with the new-file line number for added or modified code; use `side: "LEFT"` with the old-file line number for a removed line, for example a deleted guard the PR regresses. `body` leads with the severity on its own line, then a blank line, then the comment.

GitHub rejects a comment whose line falls outside a diff hunk. Confirm each anchor sits inside one of the diff's `@@ -A,B +X,Y @@` ranges before sending: the `+X,Y` range for `RIGHT`, the `-A,B` range for `LEFT`. For a genuinely off-diff observation, fold it into the nearest in-diff comment prefixed `Related (off-diff):`, or drop the anchor and add it to the review body; never drop it silently.

Submit with `gh api -X POST repos/{owner}/{repo}/pulls/<n>/reviews --input <payload.json>`, then delete the temporary payload file. Print the review's `html_url` and a one-line count by severity, plus how many off-diff items were folded elsewhere.

## Rules

- Use new-file line numbers consistently; never the old-file numbers, except for a `LEFT`-anchored removal comment.
- Report what the code does, not what the author seems to have intended.
- When later fixing accepted findings, carry forward only the accepted, concrete defects and their narrow remediations; do not expand into adjacent refactors without explicit approval.
- Never modify code, never push, and never approve a PR unless the user explicitly chose that outcome when asked.
