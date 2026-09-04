# HTML template

The design system used by `template/example.html`,
extracted so a new doc renders the same way without rebuilding it.

| File | What it is |
| --- | --- |
| [`shell.html`](shell.html) | Page skeleton. Copy this to start a doc. |
| [`components.html`](components.html) | Block catalog, 21 components with real markup. Reference, not a page. |
| [`example.html`](example.html) | Every component rendered once with placeholder data. Open it first. |
| [`assets/doc-theme.css`](assets/doc-theme.css) | The whole stylesheet. Tokens, layout, every component. |
| [`assets/doc-charts.js`](assets/doc-charts.js) | Chart runtime, theme toggle, tooltip. |

## Assemble

1. Copy `shell.html` and the `assets/` directory to the doc's folder. Keep the
   relative `assets/` paths.
2. Set the title, the masthead status chip, and one nav link per section.
3. For each section, copy the blocks it needs out of `components.html`.
4. Put every chart's mount call in the bottom `<script>`, after the markup.
5. Delete unused sections and every `SLOT` comment. No `SLOT` text ships.

## What this is not

Self-contained static HTML: two local assets, no CDN, no build step, no
framework, no network at open time.

There is no Chart.js and no Mermaid. Charts are divs and inline SVG built by
`doc-charts.js`, which is why they re-colour on the theme toggle and print
correctly.

## Components

Numbered as in [`components.html`](components.html).

| # | Component | Use it for |
| --- | --- | --- |
| 1 | `hero` | The one number the doc is about. Max one, in the summary. |
| 2 | `kpi-row` / `tile` | Two to four supporting numbers. |
| 3 | `chip` | Status pill: `good`, `warning`, `serious`, `critical`, `neutral`. |
| 4 | `callout` | An aside that must not be missed. `.warn`, `.crit`. |
| 5 | `figure.card` | Container for every chart. |
| 6 | `details.tableview` | The numbers behind a chart. |
| 7 | `hbar` | Categories with one value each. The default chart. |
| 8 | `stacked` | One bar split into parts of a whole. Two or three segments. |
| 9 | `donut` | Composition across more slices than a stacked bar can label. |
| 10 | `lineChart` | A value over time. |
| 11 | `ratio` | Two quantities orders of magnitude apart. |
| 12 | `meters` | Progress against a target, where the shortfall is the point. |
| 13 | `diagram` | Container for the structural blocks below. |
| 14 | `chain` | Ordered steps, left to right. |
| 15 | `split` / `panel` | Before and after, side by side. |
| 16 | `cellrow` | A grid of states. |
| 17 | `flow` | Boxes joined by labelled arrows. |
| 18 | `phases` | A plan as stacked rows, each with a status chip. |
| 19 | `ol.blockers` | Numbered open items, each with its consequence. |
| 20 | `table.prose` | Plain tabular content. Wrap wide ones in `.tscroll`. |
| 21 | `pre.sql` | A code or SQL block. |

### Charts

`doc-charts.js` exposes `window.docCharts`. Every function takes the id of an
empty mount div; `label` and `note` accept HTML, so `<code>` works inside them.

```js
docCharts.hbar("chart-rows", {
  data: [{ label: "Japan", value: 160, note: "All English" }],
  max: 160, unit: "rows", ticks: [0, 80, 160]
});
```

```
hbar(mountId, { data:[{label,value,note?,color?}], max?, unit?, ticks? })
stacked(stackId, legendId, [{label,value,color}])
donut(mountId, { data:[{label,value,color,display?,note?}], valueLabel?,
                 centerValue, centerLabel, aria })
lineChart(mountId, { data:[{x,y}], max, yTicks,
                     xTicks:[{i,label,anchor?}], unit, aria })
ratio(mountId, { data:[{label,value,display?,note?}] })
meters(mountId, [{label,value,max,color,track}])
```

`stacked` needs two mounts, the bar and its legend. `donut` and `lineChart`
require `aria`, since neither is readable from the DOM. `donut`'s `display`
overrides the printed value, which is how `224395264` shows as `214 MB`.

## Rules

- **Colour comes from the tokens.** `--series-1/2/3` for data, `--good`,
  `--warning`, `--serious`, `--critical` for state, `--text-*` and `--border`
  for chrome. No new hex in a doc.
- **`--series-*` is reserved for data.** Do not use it for borders or rules.
- **Pair a meter's `color` with its `track`:** `--critical` with
  `--track-critical`, `--warning` with `--track-warning`, `--series-1` with
  `--track`.
- **Every chart gets a `tableview`.** It is how a reader checks a figure and
  what remains when the chart cannot render.
- **An `h4` states the finding**, not the subject. "Nearly half of the schema
  is duplication", not "Table counts". The `.sub` carries units and source.
- **Zero is drawn, not omitted.** `hbar` ticks the baseline and `meters` keeps
  a sliver, because "nothing here" is usually the finding.
- **Dark mode is not optional.** It is `data-theme="dark"` on `<html>`; the
  masthead button sets it. Anything hardcoded breaks it.
- **Both assets stay local.** No CDN, no npm import, no bundler.

## Print

The stylesheet carries a print block: the masthead unsticks, nav and the theme
button drop out, `tableview` details force open with their summary hidden, the
ERD viewport is hidden, and cards, diagrams and the hero avoid breaking across
pages.

## ERD

The `.erd-*` classes in `doc-theme.css` style a pan-and-zoom entity diagram. No builder ships in `doc-charts.js`. Add one per doc when needed.
