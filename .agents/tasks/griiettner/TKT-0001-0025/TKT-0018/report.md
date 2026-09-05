---
id: TKT-0018
namespace: griiettner
title: Build the full-screen home, composer, commands, and picker UI
artifact: report
status: done
owner: griiettner
created: 2026-09-05
updated: 2026-09-05
chain_role: worker
chain_parent: TKT-0015
dependencies:
  - TKT-0017
areas:
  - crates/gritt-core
  - crates/gritt-provider
  - crates/gritt-harness
  - crates/gritt
  - docs
  - .agents/plans
skills:
  - tkt
  - tkt-exec-chain
  - dev-harness
  - dev-provider
  - codebase-design
  - tdd
  - write-plan
---

# TKT-0018 Report: Build the full-screen home, composer, commands, and picker UI

## Summary

Worker 3 of the TKT-0015 chain. The full-screen mode now has the two
layouts the feature plan describes, a semantic theme, one command registry
behind every entry point, one searchable picker behind every selection
dialog, a Unicode-aware composer, and a typed sidebar view model. All of it
runs on fixture state; TKT-0019 replaces the fixture builders with the
control plane without touching the reducers or the renderer.

Chain facts:

- Branch `tkt-0018-03-tui-foundation`, from `origin/main` at merge commit
  `77aaa22` (TKT-0016 contracts, TKT-0017 MCP runtime).
- The TUI stayed a client of the control plane. It gained no provider HTTP,
  no key handling, no model resolution rule, and no session storage.
- The one place a rule was reused rather than reimplemented is effort: the
  picker asks `gritt_provider::effort::effort_support` which level a model
  can take, so the adapter and the picker refuse the same cases for the
  same typed reason.

New modules under `crates/gritt-harness/src/tui/`:

| Module | Owns |
| --- | --- |
| `theme.rs` | Eight semantic tokens, dark and light palettes, the `NO_COLOR` palette |
| `composer.rs` | The multiline buffer: cursor, selection, word navigation, display width, paste |
| `command.rs` | The command registry and the parser that decides command from prompt |
| `picker.rs` | The one searchable list: rows, groups, filtering, highlight, list status |
| `sidebar.rs` | The typed sidebar view model, its placement rule, and its lines |
| `fixture.rs` | The two reviewable screens and the MCP snapshots behind `/mcp` |
| `app/tests.rs` | The reducer tests |

`app.rs` composes them and keeps the reducer; `render.rs` draws; `run.rs`
owns the terminal and gained the fixture loop.

## Key Decisions

**One registry, three entry points.** `/` suggestions, the Ctrl-P palette,
and the keyboard shortcuts all resolve through `command::COMMANDS`. A test
walks the enum and asserts every variant has exactly one row, so a command
cannot exist in the palette and not in `/`. `Command::Resume` was dropped
from the enum: `/resume` is an alias of `/sessions` in the same row, which
keeps `command::spec` total instead of leaving a variant with no row and a
panic behind it.

**Commands are parsed, not matched at the keystroke.** `command::parse`
classifies a submitted buffer as a prompt, a registry command, or an
unknown slash word. Multiline text returns `Prompt` before anything else is
considered, so a pasted script or diff cannot run a command whatever its
first character is. `//` returns `Prompt` with one slash removed.

**Overlay priority is structural, not a chain of `if`s.** An approval is
`app.pending` and takes keys first; overlays are a stack and the top one
takes them next; `/` suggestions are derived state that returns empty
whenever an approval or an overlay is open. Escape pops the stack and only
reaches a running turn when the stack is empty.

**The setup round trip is the overlay stack.** Opening provider setup from
the model picker pushes it *above* the picker rather than replacing it, so
Escape returns to a picker that still holds its search text and highlight.
No state has to be saved and restored for the round trip to work.

**Unknown sidebar values are `Option`, and `None` renders as
`unavailable`.** `UsageSection` keeps cumulative tokens and context
occupancy in separate fields, and `occupancy()` answers only when both the
current context size and the model limit are known, so a percentage cannot
be derived from cumulative usage. An integration with no runtime is `None`
and its section is not drawn; `Some(empty)` is what earns the word "none".

**The layout cache is keyed by width and a transcript revision.** Every
mutation bumps `revision`; `App::transcript_lines` reuses the previous wrap
when the width and revision match. A test passes a closure that panics if
it is called, which proves the second frame did not re-wrap.

**Escape sequences are neutralized at the boundary.** `app::sanitize` runs
when an entry is created and when a diff is drawn, replacing `ESC` with a
visible symbol and other control characters with a middle dot. A snapshot
test asserts no escape byte reaches the buffer.

**Copy is an in-application buffer.** Ctrl-Y fills `app.clipboard` from the
selection, the draft, or the transcript depending on focus. Adding an OS
clipboard crate was not in scope and `/help` says plainly that Ctrl-Y does
not write the system clipboard.

## Alternatives Considered

**A snapshot crate.** `insta` was considered and not added. The ticket
prefers no new dependency, and a plain golden file plus
`GRITT_UPDATE_SNAPSHOTS=1` gives the same review loop. No dependency was
added by this ticket at all, so no licence check was needed.

**A `unicode-width` dependency for display width.** Not needed:
`ratatui::text::Span::raw(s).width()` is the same measurement the renderer
uses, so the composer's column arithmetic and the drawn glyph cannot
disagree. `composer::display_width` is that one line.

**Grapheme segmentation** was needed, and `unicode-segmentation` 1.13.3
(MIT OR Apache-2.0, unicode-rs) was added in review round 2 after the
reviewer found that scalar-by-scalar wrapping split combining marks off
their characters. It is already a direct dependency of `ratatui-core`, so
the edge adds no code to the build; `Cargo.lock` grew by one line.
Ratatui's own `styled_graphemes` was tried first and rejected because it
filters control characters, which desyncs the byte offsets the composer
needs to place its cursor. Details in the update file.

**One golden file per theme.** Rejected: the text is identical across
palettes by design, so three files per screen would be three copies of the
same grid. Each golden holds the text once and then the distinct styles
each of the three palettes used, which is what actually differs and is
reviewable in a diff.

**Character wrapping in the transcript.** The first implementation broke
words mid-token, which looked wrong in the first golden. Replaced with word
wrapping that only splits a word longer than the line.

**Auto-opening setup when a profile has no key.** Rejected as presumptuous.
The model picker shows a `Set up <profile>` row instead, which the user
chooses deliberately.

## Assumptions

1. **`/resume` is an alias, not its own command.** The plan lists
   `/sessions` and `/resume` together and both search the same list.
   Modelling it as a second enum variant would have left a variant with no
   registry row. A different choice would have meant two rows in the
   palette for one list.
2. **A fixture prompt is answered locally.** Submitting in `--fixture` mode
   pushes a system line saying the prompt was not sent and that no session
   is open, rather than faking a streamed answer. Faking one would have
   made the walkthrough dishonest.
3. **The home status line shows the session name when there is one.** The
   plan lists directory, connection, model, effort, and phase for home. A
   named session started with `gritt tui --session NAME` opens on the home
   layout, and the existing PTY test asserts the name is drawn, so it is
   shown before the first turn too.
4. **`GRITT_THEME=light|dark` selects the palette.** The plan requires both
   palettes but names no selector. `NO_COLOR` still wins over it.
5. **`/sidebar` pushes the drawer on any width.** `sidebar::placement`
   ignores the drawer at 110 columns or more, so on a wide terminal the
   command reads as a column toggle and on a narrow one as a drawer,
   without the reducer needing to know the terminal width.
6. **Effort support is asked of `gritt-provider`, not recomputed.** The
   harness already depends on that crate. Reimplementing the rule in the
   TUI would have been a second place for it to drift.
7. **Ctrl-Y is Gritt's own buffer.** No OS clipboard crate was added; the
   limitation is stated in `/help` and in `docs/terminal-modes.md`.

## Edge Cases and Failures

- **Ctrl-M.** Never bound on its own; terminals can encode it as Enter and
  a separate binding would make Enter ambiguous. There is a test.
- **A late key after a cancelled approval.** The runtime clears `pending`
  and drops the responder together. A `y` arriving afterwards is an
  ordinary character in the composer, not an approval answer.
- **`j` and `k` in a picker.** They filter. Movement is the arrow keys,
  Ctrl-N and Ctrl-P, Tab, and the page keys.
- **Unavailable picker rows.** Shown and highlightable but not choosable;
  Enter on one puts its reason in the notice instead of doing nothing.
- **Two reasons at once.** Changing provider can clear both the model and
  an explicit effort. The notices are joined rather than one overwriting
  the other; this was a test failure first.
- **Wide glyphs in a `TestBackend` buffer.** The cell after a double-width
  glyph is a continuation whose symbol reads as a space. The snapshot
  extractor skips it, so a CJK model name reads as it does on screen.
- **`Color::Reset` is not a colour.** An unstyled cell carries
  `Some(Reset)`, so the `NO_COLOR` assertion treats `Reset` and `None`
  alike. Without that it failed on every screen.
- **Escape needs its own read window in a PTY.** A byte written immediately
  after `0x1b` is read as part of an escape sequence, not as a keypress.
  The walkthrough test sleeps after each Escape; this cost one debugging
  round.
- **Panel titles arrive in pieces.** Ratatui redraws only changed cells, so
  a title drawn over a previous panel's border reaches the PTY stream
  fragmented. The PTY assertions use body text, which lands on freshly
  cleared cells. This is why the two pre-existing needles were changed.

## Validation

Run from `/Users/griiettner/Projects/grittflow/gritt-cli-tkt-0018`:

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass, no warnings |
| `cargo test -p gritt-harness` | pass, 226 tests (128 lib, 98 integration) |
| `cargo test -p gritt --test tui_pty` | pass, 6 tests |
| `cargo test --workspace --no-fail-fast` | pass, 419 tests, 0 failed |

Counts are after two rounds of review fixes; the first gate on this branch
saw 395. See the update file for what each round changed.

Tests added: 27 reducer tests in `src/tui/app/tests.rs`, 20 unit tests
across `theme`, `composer`, `command`, `picker`, `sidebar`, and `fixture`,
9 snapshot tests over 38 committed goldens in
`crates/gritt-harness/tests/snapshots/`, and 2 PTY walkthrough tests.

The goldens cover twelve screens (home, conversation, command search, slash
suggestions, connection groups, models, a failed model catalog, effort,
an approval diff, `/mcp`, `/help`, the sidebar drawer) at 120x40, 80x24,
and 60x20, plus the conversation at 111 and 109 columns for the sidebar
boundary. Each golden holds the drawn text once and the distinct styles the
dark, light, and no-colour palettes used for it. Long model names, a CJK
model name, a deprecated model, and a missing catalog are in the fixture,
so they are in every relevant golden. Regenerate with
`GRITT_UPDATE_SNAPSHOTS=1 cargo test -p gritt-harness --test tui_snapshots`.

### Chain check

`.agents/gritt-agent ticket chain-check --ticket TKT-0018 --base main`:

```text
NOTE: current branch: tkt-0018-03-tui-foundation
NOTE: base branch `main` sha: 77aaa22cb1c5
NOTE: head sha: b9bfd14cfc25
NOTE: changed files against `main`: 57
WARN: benchmark expected but no benchmark evidence was found in report.md
tkt_chain_check ok (1 warning(s))
```

The benchmark warning is expected and not a gap in this step. The
responsiveness fixture and its recorded budgets are step 5 of the feature
plan, and TKT-0018 is explicitly scoped away from benchmark harnesses.
`.agents/gritt-agent ticket sync` then `ticket validate` reported
`tkt_validate ok (0 warnings)`.

### Walkthrough evidence and its limits

**No human ran this in a real terminal.** The walkthrough is machine-driven
and that is the whole of the evidence.

What was actually driven: the real `gritt` binary, spawned through
`portable-pty` in `crates/gritt/tests/tui_pty.rs`, at 120x40 and then
resized to 60x20, and separately at 111x30 and 109x30. In that session the
test typed `/connect` and read back both groups, `Managed by agent`, and
`not installed`; filtered to `openai` and selected it; read the model list
including the long identifier and `catalog fresh`; opened `/effort` and read
`Model default`; opened `/mcp` and read `gritt-local-memory` and
`awaiting approval`; opened `/help` and read `Limitations`; submitted
`/deploy` and read the local unknown-command error; resized and confirmed a
redraw; quit with Ctrl-Q and confirmed the alternate screen was left and
the exit status was zero. At 111 columns the sidebar column was present and
at 109 it was absent while the transcript and composer kept their space,
and `/sidebar` opened the drawer at the narrow width.

What a human should still check, because a PTY byte stream cannot:

1. Whether the home screen reads as spacious against the OpenCode
   reference: real spacing, the wordmark's weight, the composer's presence.
2. Colour in a real terminal: contrast of the dark and light palettes on an
   actual profile, and whether the accent reads as an accent.
3. Perceived latency while typing and scrolling. Nothing here was timed;
   the responsiveness budgets are step 5 of the plan.
4. The wordmark's block-drawing glyphs in fonts other than the reviewer's.
5. Bracketed paste from a real clipboard, including a paste that begins
   with `/`.
6. Shift-Enter, which only works where the terminal reports it distinctly.

## Completion Gate

- **Acceptance:** yes. Fixtures render the home, transcript, compact and
  expanded tool rows, command search, the three pickers, an approval diff,
  and `/mcp` at all three sizes. Reducer tests prove slash commands, paste,
  cancellation, overlay precedence, Unicode cursor movement, and scroll
  hold. The one criterion not met by a human is the real-terminal
  walkthrough, recorded above as machine-driven only.
- **Scope:** yes. No provider wire format, session persistence, MCP
  lifecycle, keychain or config write, or live control-plane open was
  touched. Slash commands do not send prompts. `docs/terminal-modes.md` was
  updated because `--fixture` is a new user-facing flag and the key map
  changed; the rest of plan step 6 was left alone.
- **Validation:** all five commands pass; counts above.
- **Security and safety:** no new file, network, or process access. The
  setup form's key field has no getter, only `secret_len()`, so the value
  cannot reach a transcript, a log, or an error; a test asserts the outcome
  line does not contain it. `sanitize` stops terminal escape sequences in
  model or tool output from being executed. The existing PTY test still
  asserts no key value is ever drawn.
- **Regression risk:** the two pre-existing PTY tests exercise real
  sessions and both still pass. `App::new` changed its second parameter
  from `bool` to `Theme` and `App::input`/`App::cursor` became
  `App::composer`; both are internal to the harness and the binary does not
  use them. The visible behaviour change is that a fresh session opens on
  the home layout rather than an empty bordered transcript.
- **Follow-up:** the list below.
- **Assumptions:** the seven above.

## Follow-up

0. **Two review rounds landed on this branch.** Ten findings in total, one
   high and nine medium. The high one and the round 2 findings are worth
   reading before extending this code: they were all cases where two parts
   of the interface disagreed about state (a highlight index against a
   filtered list, a renderer hiding an overlay the reducer still routed to,
   a scalar loop against a grapheme). See
   [the update file](updates/2026-09-05-review-fixes.md).
1. **TKT-0019 wiring.** `ModelCatalogView`, `AgentSummary`, and
   `SidebarModel` are populated by `fixture.rs` today. They exist to be
   filled from the control plane, the connector inventory, and session
   events. `SidebarModel::generation` and `accepts()` are there for
   rejecting late updates after a session switch, and nothing calls them
   yet.
2. **Picker scrolling** was partial at review time and is now fixed; see
   the 2026-09-05 update. The renderer computes its window in lines so a
   highlighted row stays visible at every size.
3. **No OS clipboard.** Ctrl-Y fills Gritt's own buffer. A real system
   clipboard needs a dependency and a platform review.
4. **Mouse support.** The plan mentions mouse scrolling and tool expansion
   as a supplement to the keyboard path. Not implemented; the keyboard path
   is complete.
5. **Changed files are not selectable.** The sidebar section renders and
   the plan wants selecting a file to open a read-only diff. That needs the
   harness workspace service from step 4 and belongs with it.
6. **Responsiveness is unmeasured.** Plan step 5 owns the budgets. The
   layout cache and event-driven redraw are in place to meet them, but
   nothing was timed here.
7. **`/new` clears presentation, not the session.** It empties the
   transcript view and the draft state, but the live driver and its
   continuation state stay open until TKT-0019 wires a real fresh session.
   Opening one here would be out of scope.
8. **The late-approval guarantee is tested at the reducer, not the
   runtime.** The test clears `pending` the way the loop does; proving the
   queued race would need the event loop under test, which needs a runtime
   harness this ticket does not build.
9. **Pre-existing, untouched:** the REPL's stale-approval read window is
   still recorded in `docs/terminal-modes.md` as a known follow-up.

## Updates

- [2026-09-05 review fixes](updates/2026-09-05-review-fixes.md)
