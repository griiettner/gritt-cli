---
name: write-docs
description: Writes paired Markdown and self-contained HTML docs. Use when the user asks for documentation, a proposal, a comparison, findings, a design write-up, or an HTML report with charts.
disable-model-invocation: true
---

# Write docs

Paired Markdown plus HTML for one topic. The HTML look comes from the extracted design system in `html/template/`. Do not invent a palette, type scale, or layout chrome.

## Sequence

1. Confirm kind (documentation, proposal, comparison, or findings), topic, audience, and output paths. Ask once if missing. Default: one folder per doc, `docs/<slug>/<slug>.md` and `docs/<slug>/<slug>.html`.
2. If the `.html` already exists, load [html](html/SKILL.md) and patch only what the user asked. Load [markdown](markdown/SKILL.md) only if that claim also changed. Load [polish](polish/SKILL.md) on the changed passages only. Stop. Do not recreate the HTML.
3. If the files do not exist, gather facts from code, tickets, and `gritt-local-memory`. Do not invent metrics. Build one outline used by both files. Load [markdown](markdown/SKILL.md), then [html](html/SKILL.md), then [polish](polish/SKILL.md).
4. Check both files share the same claims, sections, and sourced numbers.

## Outline

Pick the kind first. Sentence-case headings. Skip a listed heading only when it would be empty. Add Scope, Audience, Tradeoffs, or Metrics only when they carry facts.

| Kind | Use when | Keep | Skip |
| --- | --- | --- | --- |
| Documentation | How something works today | Why, How, Solution | Goal, Conclusion, Risks, Open items, What's next |
| Proposal | Asking for a decision | Goal, Why, How, Solution, Risks, Open items, What's next, Conclusion | |
| Comparison | Options, v1 vs v2, before/after, with numbers | Why, Options, Metrics, Recommendation | Goal, How as a tutorial, Open items unless a choice is still blocked |
| Findings | Spike, investigation, RCA, what we learned | Why, Evidence, What we found, Risks, What's next | Solution as if already built, Conclusion filler |

Order:

- Documentation: Why, How, Solution, extras
- Proposal: Goal, Why, How, Solution, extras, Risks, Open items, What's next, Conclusion
- Comparison: Why, Options, Metrics, extras, Recommendation
- Findings: Why, Evidence, What we found, Risks, extras, What's next

If the user does not name a kind: existing code or process is documentation. A plan or ask-for-approval is a proposal. Side-by-side options or before/after numbers are comparison. A spike, RCA, or review of what we learned is findings.

Do not use this skill for ticket `task.md` / `plan.md` / `report.md` (`tkt`), durable ADRs (`memory-write`), or package README install steps.

## Rules

- Load [`write`](../write/SKILL.md) through [polish](polish/SKILL.md) after drafts exist. Technical register: plain and specific.
- One idea per sentence. Concrete names, paths, numbers.
- Comparisons belong in Markdown tables or HTML charts.
- No chatbot closers, title-case headings, or decorative emoji.
- HTML is a visual companion of the Markdown, not a second story.
- Never replace an existing `.html` with a newly generated file. Patch the open file.

## Sub-skills

Nested under `write-docs/`. Not separately invocable. Load **one** for the current step, then polish last:

| Sub-skill | Load when |
| --- | --- |
| [markdown](markdown/SKILL.md) | Writing or updating the `.md` file |
| [html](html/SKILL.md) | Writing or updating the `.html` file |
| [polish](polish/SKILL.md) | After both drafts exist |

Routing metadata: [`index.yaml`](index.yaml).

## Verification

- Both files exist at the agreed paths and link to each other.
- Required sections for that kind are present in both files.
- Every number has a source. Charts exist only when numbers exist.
- On create, polish ran on both files. On edit, only the requested HTML spans changed.
