---
id: TKT-0018
namespace: griiettner
title: Resolve the PR #10 review findings
artifact: update
status: done
owner: griiettner
created: 2026-09-05
updated: 2026-09-05
chain_role: worker
chain_parent: TKT-0015
---

# TKT-0018 Update: Resolve the PR #10 review findings

## Trigger

The semantic review of PR #10 returned `needs-fix` with one high and six
medium findings, all confirmed against the branch. Findings are recorded in
`/tmp/review-tkt-0018-findings.md`. The reviewer's summary: "slash
completion can panic, session discovery regresses, and several renderer
behaviors do not satisfy the keyboard contract."

## Changes per finding

**1 (high): slash completion could index past the end of the list.**
`/`, Down, Tab completed `/models` and left the highlight on index 1, but
the completed query matched one row, so Enter indexed a one-row list at 1
and panicked out of the alternate screen. Three changes in
`src/tui/app.rs`: Tab now resets `suggestion_index` after it rewrites the
query; every composer edit that can change the filter (Backspace, Delete,
Ctrl-W, Ctrl-U, paste) resets it too; and reads go through the new
`App::highlighted_suggestion`, which uses `.get()` and falls back to the
first row, so no path can index out of range. `suggestion_key` also clamps
on entry and returns early on an empty list, which removes the `% count`
division as well. Two regressions:
`completing_a_suggestion_resets_the_highlight_so_enter_cannot_run_off_the_list`
drives the reviewer's exact sequence, and
`editing_the_slash_word_never_leaves_the_highlight_out_of_range` covers the
backspace direction.

**2 (medium): the first `/sessions` picker stayed empty.** The overlay is
built before `Action::RefreshSessions` returns, and the runtime only
assigned `app.sessions`, so the rows the user was looking at never filled
in. Added `Picker::replace_rows`, which swaps rows while keeping the query
and following the highlighted row by id (falling back to the current row,
then clamping), and `App::load_sessions`, which updates the list and
refreshes any open session picker. `run.rs` calls `load_sessions` instead
of assigning the field. Covered by
`sessions_loaded_after_the_picker_opened_appear_in_it`,
`a_late_session_load_keeps_the_query_and_the_highlight`,
`rows_arriving_late_keep_the_query_and_the_highlighted_row`, and a PTY
assertion: the session row carries a timestamp that appears nowhere else
on screen, so seeing it proves the rows reached the open list.

**3 (medium): picker selection moved into rows that were not drawn.**
`draw_picker` built every row from the top and let the paragraph clip, so
at 60x20 the `/connect` golden showed the "Installed agents" heading with
no agents under it. Rows are several lines tall and a group heading belongs
to the row beneath it, so the window is now computed in lines: the renderer
records each filtered row's line extent, then picks a start that keeps the
highlighted row's whole block visible. A window that would end on a
trailing heading trims it, so a heading is only drawn when a row follows it.
Slash suggestions got the same treatment, since twelve commands do not fit
the panel at 60x20. Covered by `a_picker_shows_the_highlighted_row_at_every_size`
(walks every row at all three sizes),
`the_connection_dialog_never_shows_a_group_heading_with_no_rows_under_it`,
and `slash_suggestions_keep_the_highlighted_command_on_screen`.

**4 (medium): scroll hold held an offset, not content.** The viewport was
computed as `total - scroll`, so every wrapped line that arrived pushed the
held view down by one. Held position is now measured from the top, where
appends cannot move it: `App::scroll` became `App::top`, and `touch()`
leaves it alone. The reducer needs the rendered line count and the pane
height to answer "am I at the bottom", so `App` gained a `Metrics` cell the
renderer fills each frame, and the viewport arithmetic moved into
`App::visible_transcript`, which the renderer and the tests both call. The
test now compares the visible lines before and after ten streamed lines
rather than asserting an offset stayed `10`, and a second test proves
scrolling back to the bottom resumes following.

**5 (medium): the constrained home composer hid what was being typed.**
The buffer was wrapped by `Paragraph` while the cursor was placed from
unwrapped coordinates and clamped, so a draft past the 88 interior columns
vanished under the box. The composer now wraps in `render.rs` through
`composer_rows`, which hard-wraps each logical line at the drawn width and
records where each row starts, so the cursor's visual row and column come
from exactly the rows that are drawn. The view scrolls to keep the cursor's
row on screen. Character wrapping, not word wrapping, is deliberate here:
an editor cursor has to land on a predictable cell. Covered by
`a_draft_longer_than_the_composer_scrolls_instead_of_disappearing`, which
checks the single-line and multiline cases and that the cursor sits on the
row holding the text.

**6 (medium): `/sidebar` contradicted the placement rule.** It always
pushed a drawer, and the renderer drew that drawer at any width, so on a
wide terminal the command replaced the column with a modal instead of
hiding it. `toggle_sidebar` now reads the measured terminal width: at 110
columns or more it flips `sidebar_enabled` and pushes nothing; below that it
opens the drawer as before. The renderer skips a drawer left open by a
resize once the terminal is wide enough for the column. The column also
hard-coded its scroll to zero, so Tab reached it and nothing moved;
`draw_sidebar` now uses `sidebar_scroll` for both forms, and Up, Down,
PageUp, and PageDown route to it when focus is on the sidebar. Covered by
`on_a_wide_terminal_sidebar_toggles_the_column_and_never_opens_a_drawer`,
`the_sidebar_column_scrolls_without_moving_the_transcript_or_the_draft`,
and `hiding_the_sidebar_on_a_wide_terminal_widens_the_transcript`.

**7 (medium): a refused phase change still changed the display.** `/plan`
and `/code` set `status.phase` optimistically while the runtime rejects the
change during a turn. The optimistic write is gone: the displayed phase now
moves only in `set_session`, which the runtime calls after the driver
applies it. A `changes_settings` guard also refuses `/connect`, `/models`,
`/effort`, `/plan`, `/code`, and `/new` while a turn or an approval is
active, with a notice naming which one is blocking, satisfying the plan's
"do not permit settings changes while a turn or approval is active".
Read-only commands (`/help`, `/mcp`, `/details`, `/sidebar`, `/sessions`,
`/quit`) still work. Covered by
`a_running_turn_refuses_settings_commands_and_shows_no_phase_change` and
`a_pending_approval_refuses_settings_commands_too`.

**Optional follow-up taken:** the narrow home status now drops the working
directory and the phase before anything else and always keeps the session
name and the `fixture` label, so a 60x20 screenshot cannot lose the label
that marks it as fixture data. Covered by
`the_fixture_label_survives_the_narrowest_home`.

**Optional follow-ups recorded, not taken:** strengthening the runtime
late-approval test beyond clearing `pending` needs the event loop under
test, which is a runtime harness this ticket does not have; and `/new`
still clears presentation while the live driver stays open. Both are in
`report.md` under Follow-up.

## Edge cases and failures found while fixing

- Guarding settings during a turn broke
  `escape_closes_the_top_overlay_first_and_then_cancels_the_turn`, which
  opened its overlays after setting `running`. The test now opens them
  while idle, which is what a user can actually do.
- `Metrics.terminal_width` was never written, so the first version of the
  wide-terminal sidebar test still saw a drawer. `render::draw` now records
  the width before anything is laid out.
- The new compact home status dropped the session name, which broke the
  pre-existing PTY test that waits for it at 80 columns. The compact
  variant keeps the session name; it drops the directory and phase instead.
- A picker filter test asserted two matches for query `a` over rows
  including `beta`, which also contains an `a`. The fixture rows were made
  unambiguous rather than the assertion loosened.

## Validation

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass, no warnings |
| `cargo test --workspace --no-fail-fast` | pass, 411 tests, 0 failed |
| `cargo test -p gritt --test tui_pty` | pass, 6 tests |

411 passing, up from 395: 16 tests added across the seven findings. Every
snapshot golden was regenerated; the visible differences are the picker
viewport, the trimmed trailing group heading, the composer's own wrapping,
and the narrow home status.

`.agents/gritt-agent ticket chain-check --ticket TKT-0018 --base main`:

```text
NOTE: current branch: tkt-0018-03-tui-foundation
NOTE: base branch `main` sha: 77aaa22cb1c5
NOTE: changed files against `main`: 61
tkt_chain_check ok (0 warning(s))
```

The benchmark warning the first gate reported is gone now that this update
states where the responsiveness work lives. Nothing was benchmarked here:
the responsiveness fixture and its budgets are step 5 of the feature plan
and outside this ticket. `ticket sync` then `ticket validate` reported
`tkt_validate ok (0 warnings)`.

## Round 2

The re-review confirmed fixes 1, 2, 3, 4, and 7 and returned three new
medium findings, all in the code round 1 introduced: the drawer and the
composer wrapping. Findings are in `/tmp/review-tkt-0018-r2-findings.md`.

**R2-1: an invisible drawer captured input after a resize.** Round 1 made
the renderer skip a drawer at 110 columns or more, but left the overlay on
the stack, and the reducer still routed keys into it. Opening `/sidebar` at
80 columns and resizing to 120 left the visible composer deaf until Escape.
Hiding something in the renderer while the reducer still believes in it was
the mistake; the two now agree on one rule. `App::reconcile_layout` drops
state the current size cannot support: it pops any drawer once the column
fits, turning it into the column the user asked for, and moves focus off a
sidebar that is not on screen. It runs from the new `App::on_resize`, which
both event loops call on a `Resize` event, and again at the top of
`on_key`, so a missed or coalesced resize cannot leave a key routed into
something invisible. Covered by
`a_drawer_left_open_by_a_resize_does_not_swallow_input`,
`a_stale_drawer_is_reconciled_before_the_next_key_is_routed`,
`a_drawer_becomes_the_column_when_the_terminal_grows` (through the
renderer), and a PTY assertion: the fixture conversation opens the drawer
at 109 columns, resizes to 120, and Ctrl-P still opens the palette, which a
stale drawer would have swallowed.

**R2-2: composer wrapping split combining marks from their characters.**
`composer_rows` iterated Unicode scalars and forced a zero-width mark to
one cell with `.max(1)`, so at 60x20 a combining acute after 57 characters
was pushed onto a row of its own and the one-row composer scrolled the base
text away. Wrapping, cursor movement, deletion, word navigation, transcript
long-word splitting, and sidebar truncation now all step over grapheme
clusters, and each cluster is charged its real display width rather than a
forced minimum of one. Ratatui's own `styled_graphemes` was tried first and
rejected: it filters control characters out of its output, so byte offsets
computed from it desync on a pasted tab, and the composer needs exact
offsets to place a cursor. Covered by
`a_combining_mark_travels_with_the_character_it_modifies` (cursor and
delete), `a_combining_mark_never_wraps_away_from_its_character` (the
reviewer's exact 60x20 case, asserting the drawn text, that no row holds a
lone mark, that the base text did not scroll away, and that the cursor is
charged one cell), and
`transcript_wrapping_keeps_combining_marks_with_their_characters`.

**R2-3: a hidden sidebar consumed the scroll keys.** Tab and BackTab
selected `Focus::Sidebar` unconditionally while PageUp and PageDown routed
to the sidebar offset whenever focus was there, so in a narrow conversation
Tab twice then PageUp moved nothing anyone could see. Focus now only stops
on panes that are drawn: Tab and BackTab skip the sidebar unless
`sidebar_column_visible()`, the scroll keys check the same predicate, and
`reconcile_layout` normalizes focus when the column goes away by resize or
by `/sidebar`. Covered by
`focus_skips_the_sidebar_when_its_column_is_not_on_screen` (including the
reviewer's Tab, Tab, PageUp sequence) and `hiding_the_column_moves_focus_off_it`
(both the toggle and the resize path).

### Dependency added

`unicode-segmentation` 1.13.3, MIT OR Apache-2.0, from the unicode-rs
group, added to the workspace and to `gritt-harness`. Checked before
adding, as the CLI skill requires: it is already a direct dependency of
`ratatui-core` and `ratatui-widgets`, so it is compiled into every Gritt
binary today and this edge adds no new code and no new transitive
dependency. `Cargo.lock` grew by one line, the edge itself. Source is 712
KiB, mostly Unicode tables and tests, with no build script. The
alternative, grouping scalars by zero display width, would have handled the
reported combining-mark case but not clusters generally, and the reviewer
asked for grapheme boundaries.

### Round 2 validation

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass, no warnings |
| `cargo test --workspace --no-fail-fast` | pass, 419 tests, 0 failed |
| `cargo test -p gritt --test tui_pty` | pass, 6 tests |

419 passing, up from 411: 8 tests added. No snapshot golden changed, which
is itself worth recording: the grapheme rewrite altered no fixture
rendering, because the fixtures hold CJK and long identifiers but no
combining marks, and those already wrapped correctly.

`.agents/gritt-agent ticket chain-check --ticket TKT-0018 --base main`:

```text
NOTE: current branch: tkt-0018-03-tui-foundation
NOTE: base branch `main` sha: 77aaa22cb1c5
NOTE: changed files against `main`: 64
tkt_chain_check ok (0 warning(s))
```

## Remaining follow-up

The `Picker::window` helper is now unused by the picker renderer, which
computes its window in lines instead of rows; it stays because the row-based
form is still the right shape for a flat list and is covered by its own
test. Grapheme clusters are handled through `unicode-segmentation`, so
emoji sequences and flags wrap as single units; terminal support for
drawing them still varies, which is a rendering limit rather than a Gritt
one. Everything else remaining is the follow-up list in `report.md`.
