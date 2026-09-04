---
name: review-impact
description: Reviews a diff's blast radius across the rest of the codebase. Use when asked whether a change breaks anything else, with no PR or ticket framing.
---

# Impact review

Read [review](../SKILL.md) first. Load [`write`](../../write/SKILL.md) when writing the report's paragraphs.

Answers one question: does this change break anything else that currently works? A line-by-line read of the diff's own code is still useful, but secondary to tracing what the change reaches.

## Resolve the diff

Use whatever the user named: the working tree diff (`git diff`), a branch comparison against its merge base (`git diff $(git merge-base <base> <head>) <head>`, never `<head>~1`, which shows only the last commit of a multi-commit change), or a specific commit range. Ask only when nothing in the request or the repo state identifies a target. Note whether the working tree is clean; an uncommitted local edit changes what "the diff" means.

## Trace the blast radius

For every changed export, function, endpoint, component, config key, or schema, answer in order:

1. Who consumes it? Search for the symbol, import, route, or call site across the whole repository, not just the changed files. Delegate a broad "find every caller of X" sweep to an `Explore` (or heavier `general-purpose`) subagent and keep the conclusions. A change with no live consumer is inert; say so and move on.
2. Does it run by default? A change behind a flag, an env var, or a config value the deployment leaves at its default does not change current behavior. A change with no gate, or a default that is already on, does. Read the actual default; do not assume it from the name.
3. Does it override something that was deliberately off? If the diff re-enables or replaces behavior the codebase had disabled, commented out, or gated for a reason, that is a real change even without a crash. Flag it and ask whether it is intended.
4. Can it crash or break the page or process? Check that imports resolve, required providers or context wrap every new consumer, and any migration or config the code now depends on is present. A missing provider or a broken re-export is the highest-priority finding.
5. Is it already handled elsewhere? A duplicate or conflicting change in another open or recently merged PR is worth surfacing (`gh pr list`, the ticket index) so the two do not collide.

## Classify and verify

Rate impact, not effort to fix:

- 🔴 Critical: boot failure, a broken import or provider, or a crash on the main path.
- 🟠 High: a real behavior change that runs by default, or an unnecessary override of working behavior. Needs an explicit decision before merge.
- 🟡 Medium: only triggers under a non-default flag, config, or input; an edge case worth noting.
- 🟢 Low: gated behind a default that stays off, fail-open, additive, or docs and tests only.

When torn between two levels, pick the lower one and say why.

Before reporting a finding, reread the actual current file content at the location it cites, not just the diff hunk. Mark a finding CONFIRMED only after that check passes; otherwise mark it PLAUSIBLE.

## Report

Lead with a one- or two-line verdict naming the one or two things that actually need a decision. If nothing runs on the default path, say so plainly and keep it short. Follow with a table: item, file, live by default, impact (🔴/🟠/🟡/🟢), action. Write a paragraph only for a 🔴 or 🟠 item, or a 🟡 one that needs a decision; a 🟢 item belongs in the table only. Close with a short crash and override check: do the imports and providers resolve, what stayed gated to a safe default, and any unnecessary override, stated plainly.

Rank findings most severe first. Do not fix a finding unless the user asked for a fix pass; report only.
