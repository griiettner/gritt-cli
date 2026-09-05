---
name: write-html
description: Presents Markdown content as an HTML companion using shared design assets. Use when a documentation or planning workflow requires HTML.
---

# HTML

Read [write](../SKILL.md) first for prose guidance. The calling workflow
supplies the outline, canonical Markdown, and output paths. Preserve its
document structure, whether documentation or a feature proposal.

The design system lives in [template/](template/). When something here is ambiguous, [template/example.html](template/example.html) is the reference render.

Start by opening [template/example.html](template/example.html) in a browser.
Every component appears there once with placeholder data; it is faster to look
at than to read about. Then work from
[template/README.md](template/README.md) and
[template/components.html](template/components.html).

Static HTML, opened in a browser. Do not start a React app, a Vite dev server,
or an Nx target unless the user asked for one.

## The stack

Two local assets and nothing else:

- `assets/doc-theme.css` — every token, layout rule and component style
- `assets/doc-charts.js` — chart runtime, theme toggle, tooltip

No CDN, no npm dependency, no build step, no network at open time.

**There is no Chart.js and no Mermaid.** Charts are divs and inline SVG built by
`doc-charts.js`. That is what makes them re-colour on the theme toggle and print
correctly. Do not add a charting library, and do not hand-roll a chart as raw
SVG when a `docCharts` function covers it.

## Create

1. Copy `template/shell.html` to the doc path and `template/assets/` next to it.
   Keep the relative `assets/` paths.
2. Set the title, the masthead status chip, and one nav link per section.
   Link to the canonical Markdown and add the reciprocal link there.
3. Copy the blocks each section needs out of `template/components.html`.
4. Put every chart mount call in the bottom `<script>`, after the markup.
5. Delete unused sections and every `SLOT` comment. No `SLOT` text ships.
6. Open the file in a browser, toggle the theme, and hover a chart before
   calling it done.

## Edit

If the `.html` path exists, edit it. Do not write a new document.

1. Read the current file. Keep its shell: doctype, asset links, masthead, nav,
   section ids, and the bottom script.
2. Change only the nodes the user named — a section, a table row, a chart's
   data array, a caption, a link.
3. Use a surgical replace on those spans. Do not re-emit the whole file, do not
   restyle or reorder untouched markup, and do not "clean up" the assets.
4. If the paired Markdown exists, update the matching claim there. Do not
   rebuild the HTML from the Markdown.

## Verification

- The companion and Markdown share claims, section coverage, and sourced
  numbers, with working reciprocal links.
- Local assets load without network access and no template placeholders remain.
- Open the page in a browser and check narrow layout, light and dark themes,
  navigation, and any chart interactions. Report checks that could not run.
- Apply the prose pass from `write` to new text or changed passages while
  preserving chart data, claims, and unaffected markup.

## Components

Twenty-one blocks, catalogued in [template/README.md](template/README.md) with
the markup in [template/components.html](template/components.html): `hero`,
`kpi-row`/`tile`, `chip`, `callout`, `figure.card`, `details.tableview`, `hbar`,
`stacked`, `donut`, `lineChart`, `ratio`, `meters`, `diagram`, `chain`,
`split`/`panel`, `cellrow`, `flow`, `phases`, `ol.blockers`, `table.prose`,
`pre.sql`.

Pick by what the content is doing:

| Need | Component |
| --- | --- |
| The one number the doc is about | `hero`, once, in the summary |
| Supporting numbers | `kpi-row`, two to four tiles |
| Categories with one value each | `hbar` — the default chart |
| Parts of a whole, 2–3 parts | `stacked` |
| Composition, many slices | `donut` |
| A value over time | `lineChart` |
| Quantities orders of magnitude apart | `ratio` |
| Progress where the shortfall is the point | `meters` |
| Ordered steps | `chain` |
| Before and after | `split` with two `panel`s |
| A read/write path | `flow` |
| A plan | `phases`, each row with a status chip |
| Open items | `ol.blockers` |

## Rules

- Colour comes from the tokens: `--series-1/2/3` for data, `--good`,
  `--warning`, `--serious`, `--critical` for state, `--text-*` and `--border`
  for chrome. Never add hex to a doc.
- `--series-*` is for data only, not for borders or rules.
- Every chart gets a `details.tableview` with its numbers, and a caption whose
  `.sub` gives the unit and the source.
- The `h4` states the finding, not the subject.
- Draw zero rather than omitting it. `hbar` ticks the baseline, `meters` keeps a
  sliver, and that is usually the point being made.
- `donut` and `lineChart` require `aria`; neither is readable from the DOM.
- Omit a chart when there are no sourced numbers. Do not invent data to fill a
  component.
- Dark mode must keep working. It is `data-theme="dark"` on `<html>`; anything
  hardcoded breaks it.

## ERD

The stylesheet ships `.erd-*` classes for a pan-and-zoom entity diagram, but no builder. Add one only when a doc needs it, and keep it driven by the doc's own model.
