---
id: TKT-0019
namespace: griiettner
title: Integrate conversation sidebar, sessions, MCP status, and responsive runtime
artifact: report
status: done
owner: griiettner
created: 2026-09-05
updated: 2026-09-05
chain_role: worker
chain_parent: TKT-0015
dependencies:
  - TKT-0018
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

# TKT-0019 Report: Integrate conversation sidebar, sessions, MCP status, and responsive runtime

## Summary

Worker 4 of the TKT-0015 chain. The full-screen mode is no longer a
prototype over fixture data. It opens without a driver, creates the
session from a draft when the first prompt is submitted, lists real
profiles and real installed agents, loads real catalogs, sets effort
through the driver, shows live MCP state from a subscription, reports
workspace changes from read-only Git, and opens a read-only diff for a
changed file. Everything that can wait runs off the terminal event path
with a visible loading line and Escape to cancel it.

Chain facts:

- Worktree: `/Users/griiettner/Projects/grittflow/gritt-cli-tkt-0019`
- Branch `tkt-0019-04-tui-integration`, from `origin/main` at merge
  commit `033e755` (TKT-0016 contracts, TKT-0017 MCP runtime, TKT-0018
  TUI foundation).
- Commits: `8d18b68` (harness seams), `0c8bfb4` (the TUI integration),
  `4a7dc94` (reducer and harness tests), `05afda9` (PTY tests),
  `e306dae` (the narrow walkthrough), `9e32a0c` (the timing seam), plus
  the ticket-artifact commit carrying this report.

The TUI stayed a client of the control plane. It gained no provider HTTP,
no key handling, no model resolution rule, and no session storage. The
three new capabilities it needed all landed as harness services with
typed contracts, not as logic in the renderer.

### What landed

| Where | What |
| --- | --- |
| `crates/gritt-harness/src/mcp/mod.rs` | `subscribe()`, a broadcast of the whole snapshot list published on every lifecycle change |
| `crates/gritt-harness/src/changes.rs` | New: `WorkspaceChanges`, the baseline, `git status` parsing, observed writes, read-only per-file diffs, and the `GitRunner` seam |
| `crates/gritt-harness/src/setup.rs` | `ProviderSetup::reload_config`, `SetupSubmission`, `apply_setup` |
| `crates/gritt-harness/src/control.rs` | `reload_config`, `reloaded`, `ControlPlane: Clone` |
| `crates/gritt-harness/src/agent.rs` | `AgentBuilder: Clone` |
| `crates/gritt-harness/src/tui/app.rs` | Live pickers, the new actions, the async tokens, cost and context, the setup form's real write, `/mcp` actions, the changed-file list |
| `crates/gritt-harness/src/tui/run.rs` | The loop without a driver: `Runtime`, `on_message`, `on_action`, the lazy open, the subscription, the scans |
| `crates/gritt-harness/src/tui/render.rs` | The read-only diff overlay, the loading line, the setup panel's protocol and destination, `short_path` |
| `crates/gritt/src/setup.rs` | `FileSetup::reload_config` |
| `crates/gritt/src/main.rs` | The lazy `gritt tui` path and the seeded draft |

## Key Decisions

**The lazy path is the default for an unnamed native run.** `gritt tui`
with no `--session` and no external connector opens on a draft seeded
from `--profile` and `--model` above the configured defaults, and the
first prompt is what calls `open_draft`. A named session and a connector
session stay eager: the user asked for that session, and a failure to
open it is their answer rather than a screen that quietly does nothing.
This also means `/connect` works in a workspace with no configuration at
all, which the eager path could not do because it failed before drawing.

**Every asynchronous result carries the token it started under.** Two
tokens, because two different things go stale. `App::selection` moves on
every provider selection and guards catalog loads;
`SidebarModel::generation` moves on every session change and guards
change scans, draft opens, and resumes. `apply_catalog`, `apply_changes`,
and the `Opened` message all check before they write, and all three
return whether they accepted, which is what the tests assert on. The
generation existed unused from TKT-0018; this is its caller.

**MCP state is delivered, not polled.** TKT-0017 left this open and
recorded it as the cheapest remaining improvement. `McpRuntime` now owns
a `tokio::sync::broadcast` sender and publishes the complete snapshot
list from `load`, `start`, `decide`, `stop`, `refresh`, and `shutdown`.
Publishing the whole list rather than a delta is what makes a lagged
receiver safe: it skips intermediate frames and converges on the current
truth, so the TUI's subscriber treats `Lagged` as "wait for the next
message" instead of replaying a history.

**Configuration reload is the binary's, applied by the plane.**
TKT-0016 recorded that `ProfileSaveOutcome::Saved` does not claim the
running configuration changed, and left the call here.
`ProviderSetup::reload_config` returns a freshly merged `Config` (ADR-006
keeps the layer merge in the binary), and `ControlPlane::reloaded`
rebuilds the plane around it while sharing the store, telemetry, catalog,
cache, workspace, and MCP runtime. Nothing already open is disturbed;
only the configuration-derived parts change. The TUI holds the plane
behind an `Arc` and swaps the handle, so a task that is already running
keeps the plane it started with.

**Changed files are a harness service with an injected `git`.**
`WorkspaceChanges` captures the baseline once, at open, and refuses a
second capture: calling it again would silently reclassify the session's
own work as pre-existing. Git is behind a `GitRunner` trait, which is why
the tests need no repository and why a machine without `git` degrades to
observed writes labelled partial rather than failing. Every invocation is
read-only; there is no path here that stages, checks out, or writes.

**Selecting a changed file reuses the picker.** The plan wants selecting
a file to open a read-only diff. Rather than add a selection cursor to
the sidebar - which would have taken the arrow keys that TKT-0018 gave to
sidebar scrolling, and broken its test - Enter on the focused sidebar
opens the changed files as the same searchable list every other selection
uses. It is searchable, needs no new binding, and does not touch the
composer.

**`/mcp` rows are all selectable now.** TKT-0018 marked a non-ready row
unavailable. That was backwards once the actions are real: approving and
restarting are exactly what a user opens `/mcp` for, and both apply to
entries that are not running. The availability moved to the action rows,
where an inapplicable action says why instead of being hidden.

**Context occupancy has one source and it is named.** The prompt tokens
of the most recent request are the tokens that were in the model's
context for it. The cumulative totals are not, and the sidebar keeps them
in separate fields, as TKT-0018's types already insisted. Without a
catalog there is no limit, so occupancy stays unavailable rather than
being derived from the cumulative total.

**Cost is an estimate from listed prices or nothing.** Both
`input_price_per_million` and `output_price_per_million` must be reported
for the active model, and both token counts must exist. Otherwise the
section is unavailable. It is always labelled an estimate with its scope.

## Alternatives Considered

- **Polling `snapshots()` on the tick.** TKT-0017 said the TUI would have
  to. It works and is three lines. It was rejected because an idle screen
  must not redraw continuously and because a poll interval is a latency
  floor on approval state, which the user is waiting on.
- **Mutating the plane in place for a reload.** `reload_config(&mut self)`
  exists and the binary can use it, but the loop shares the plane with
  spawned tasks, so the TUI uses `reloaded()` and replaces the `Arc`. A
  task that is mid-flight finishes against the plane it started with,
  which is simpler to reason about than a plane changing underneath it.
- **A selection cursor in the sidebar.** Rejected as above: it would have
  taken the arrow keys and changed a tested binding for one feature.
- **Deriving context occupancy from cumulative usage.** Explicitly
  forbidden by the plan and by TKT-0018's types. Not attempted.
- **Writing the PTY setup test through the real keychain.** Rejected: a
  test must not write to a developer's login keychain. The test leaves
  the key field blank, which is a real supported path - the profile is
  saved and the variable to export is named - and the keychain is
  covered by the fake service in `tests/tui_integration.rs`.
- **`toml_edit` for comment-preserving config writes.** Out of scope; the
  TKT-0016 follow-up still stands.

## Assumptions

1. **Lazy applies to any unnamed native run**, including one with
   `--profile`/`--model` flags. A different reading would have kept the
   eager path whenever flags were given; that would have made `/connect`
   unusable in an unconfigured workspace, which the acceptance criteria
   require.
2. **Context occupancy is the last request's prompt tokens.** If that is
   judged too indirect, the field can be set to `None` and only the
   cumulative totals shown; nothing else depends on it.
3. **Preset endpoints and protocols are copied from the shipped
   `config.toml` template** (`openrouter`, `openai`, `anthropic`,
   `local`). A provider not in that list is set up as a custom endpoint.
4. **A blank key variable is derived from the profile name**
   (`ptylocal` gives `PTYLOCAL_API_KEY`) rather than refused, and the
   custom form opens on the name field. Without this a custom endpoint
   had to type a variable to replace the placeholder `_API_KEY`.
5. **Approving a server in `/mcp` records the decision and then starts
   it**, mirroring what `gritt mcp trust` does. Approving alone would
   leave the entry at `starting` with nothing starting it.
6. **`/effort` on a live session persists through the driver
   immediately**; before a session exists it only updates the draft. The
   plan calls effort a session setting that can change between turns.
7. **The session `activity` line reuses the status events** the driver
   already emits, lower-cased, plus `running` while a turn is in flight.
   No new event was added for it.
8. **A connector session refuses `/connect`, `/models`, and `/effort`**
   with a notice naming the agent. `/plan`, `/code`, `/new`,
   `/sessions`, `/details`, `/sidebar`, `/mcp`, and `/help` still work:
   those are Gritt's, not the agent's.
9. **The home status shortens a long workspace path from its head.** A
   temp-directory path was pushing the connection, effort, and phase off
   the line. The last components are what identify a directory.

## Edge Cases and Failures

- **A refused draft costs nothing typed.** `undo_submission` removes the
  user entry the submission pushed and puts the prompt back in the
  composer, then the typed `DraftError` opens as a modal explanation.
  `describe_draft_error` turns each variant into a title and a body; the
  interface never parses an error string.
- **A driver that arrives after the user left.** `Opened` checks the
  sidebar generation. A driver for a superseded session is dropped rather
  than adopted, and its prompt is not sent anywhere.
- **A catalog for a provider the user left.** `apply_catalog` refuses on
  the selection token and on the drafted profile, and the test proves the
  current token still works afterwards, so a refusal breaks nothing.
- **A model that leaves the list.** `apply_catalog` calls
  `revalidate_effort`, so an explicit effort the newly arrived list does
  not support returns to the model default with an explanation.
- **A keychain that refuses.** The profile is already written, so the
  flow closes with the variable named. A profile with no key is usable as
  soon as that variable is exported; refusing the whole setup would have
  been worse.
- **A workspace with no repository.** `git status` fails or `git` is
  missing, so the list falls back to files Gritt itself wrote and is
  labelled `partial: observed writes only`.
- **A write that failed.** Only a successful `file_write` result promotes
  its path to an observation. A refused or errored write is not claimed
  as a change.
- **A rename in `git status`.** Reported at its new path, which is the
  one that exists on disk.
- **A diff larger than 64 KiB.** Truncated on a character boundary with a
  visible note, so a generated file cannot fill the viewport.
- **An MCP entry that cannot run.** `invalid` and
  `unsupported transport` entries stay listed with their reason, and
  their restart and stop actions say to approve or that nothing is
  running. The narrow PTY walkthrough drives exactly this case.
- **A lagged MCP subscriber.** Treated as "wait for the next message".
  Every message is the whole state, so nothing is replayed.
- **Escape with work in flight.** Aborts the background task, clears the
  loading line, and says `cancelled`. Escape still closes the top overlay
  first and still cancels a running turn when nothing is open.

## Validation

Run from `/Users/griiettner/Projects/grittflow/gritt-cli-tkt-0019`:

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass |
| `cargo test -p gritt-harness` | pass, 292 tests over 10 targets |
| `cargo test -p gritt --test tui_pty` | pass, 10 tests |
| `cargo test --workspace --no-fail-fast` | pass, 447 tests, 0 failed |
| `GRITT_LIVE_MCP_TESTS=1 cargo test -p gritt-harness --test mcp_live_smoke` | pass, 1 test, after the note below |

The live smoke check failed on its first run in this worktree with
`gritt: initialize did not answer within 30s`. That is TKT-0017's
recorded first-start latency, not a defect and not the database-lock
collision: `.agents/gritt-agent mcp serve` indexes 289 local knowledge
files on a fresh worktree, measured here at 41 s before it answers
`initialize`, against a 30 s default deadline. Once the index existed the
check passed in 1.08 s. Wiring `McpRuntimeSettings` to `config.toml`
remains TKT-0017's follow-up.

New test counts: 15 reducer tests in `src/tui/app/tests.rs`, 10 in the
new `tests/tui_integration.rs`, 4 PTY tests in
`crates/gritt/tests/tui_pty.rs`, 3 new snapshot screens at 5 sizes each,
and 4 unit tests in `src/changes.rs`.

`.agents/gritt-agent ticket chain-check --ticket TKT-0019 --base main`:

```text
NOTE: project root: /Users/griiettner/Projects/grittflow/gritt-cli-tkt-0019
NOTE: git root: /Users/griiettner/Projects/grittflow/gritt-cli-tkt-0019
NOTE: current branch: tkt-0019-04-tui-integration
NOTE: base branch `main` sha: 033e75523b63
NOTE: head sha: 9e32a0ca9911
NOTE: changed files against `main`: 35
NOTE:   (35 changed files; the full list is in the command output)
tkt_chain_check ok (0 warning(s))
```

### Walkthrough evidence and its limits

**No human ran this in a real terminal.** The walkthrough was driven
through the PTY harness, which spawns the real binary in a real
pseudo-terminal and reads the real escape stream, but writes bytes rather
than key events and reads a byte stream rather than a screen.

What the harness covered:

| Size | Covered |
| --- | --- |
| 120x40 | First run with no configuration: `/connect`, the "Add a provider" group, the custom endpoint form, Ctrl-D choosing the workspace destination, the save, and the profile becoming selectable in the same run |
| 120x40 | The lazy path: home with the drafted selection, the first prompt opening the session, the sidebar column, `/new` keeping the session, `/sessions` listing it, resume reloading its history |
| 120x40 | A connector session on a real installed agent (`codex` was present on this machine, so this ran rather than skipped): `/models` refused with the agent named |
| 80x24 | Ctrl-J inserting a newline the submitted prompt keeps, the sidebar as a drawer, `/mcp` accounting for an entry that cannot run, help scrolling |
| 120x40 to 60x20 | The fixture walkthrough from TKT-0018, still passing |
| 111 / 109 | The sidebar column boundary, in a real terminal and in `TestBackend` |

Platform-specific input limitations found:

1. **Ratatui redraws only changed cells**, so text typed a character at a
   time never appears as a contiguous string in the stream. Assertions
   about typed text have to wait for a full redraw of a cleared area.
   This is a harness limitation, not a product one.
2. **Shift-Enter cannot be tested here.** The harness writes bytes, and
   Shift-Enter is only distinguishable on terminals that report it
   distinctly (kitty keyboard protocol and similar). Ctrl-J is the
   newline the harness can prove, and it does.
3. **Ctrl-M is deliberately not bound** separately from Enter, so no test
   can distinguish them; that is the intended behavior.

What a human should still check in a real terminal:

- Shift-Enter for a newline on a terminal that reports it (kitty, WezTerm,
  Ghostty), and that it does nothing harmful on one that does not.
- Bracketed paste of a multiline block beginning with `/`, and that it
  neither runs a command nor opens the suggestion list.
- Mouse scrolling, which is not implemented; confirm it does not corrupt
  the viewport.
- The dark and light palettes against a real terminal theme, and
  `NO_COLOR` on a terminal with a light background.
- Ctrl-C during a streaming turn with an MCP server hung, and that no
  child process survives.
- A real provider key through the setup form writing to the real
  keychain. No automated test does this, on purpose.

## Completion Gate

- **Acceptance:** yes. A fresh TUI connects to configured providers or
  installed agents, creates and resumes sessions, selects model and
  effort, shows every MCP server state, invokes approved native MCP
  tools, preserves the composer draft and scroll through dialogs, keeps
  connector authority separate, and rejects late async work for a
  superseded session or selection.
- **Scope:** yes. No new MCP protocol feature, no provider request
  mapping change, no LSP or skill execution, no visual redesign, and no
  user documentation. No cost or context figure whose source is unknown.
  The measured benchmark is TKT-0020's; only its seams are here.
- **Validation:** all six commands pass. The live smoke check needed a
  warm memory index first; the reason is recorded above.
- **Security and safety:** the key travels in exactly one type
  (`SetupSubmission`), is taken from the form once and cleared from it,
  never enters an `Action`, a transcript, an event, or a `Debug` output,
  and reaches only the injected keychain writer. Config and keychain
  writes stay in the binary (ADR-006, ADR-008). `WorkspaceChanges` runs
  `git` read-only with the workspace as `-C` and a fixed argument list;
  no shell, no user string in a command position. The diff overlay is
  read-only and sanitizes control characters, so an escape sequence in a
  diff is drawn rather than executed. MCP approval still goes through the
  runtime's typed trust API; nothing here can launch a server without a
  recorded decision. No new dependency was added.
- **Regression risk:** the two real ones are the runtime loop and the
  MCP publish points. The loop was rewritten around a message and an
  action handler; every previous behavior it had (approval, cancel,
  phase, resume, session refresh, teardown, panic restoration) is still
  there and the existing PTY tests still pass unchanged. `publish()`
  takes the state lock after the mutating section has released it, so it
  cannot deadlock, and a send with no subscribers is not an error; all 48
  `mcp_runtime` tests pass. `ControlPlane` and `AgentBuilder` became
  `Clone`, which shares handles and copies only the configuration.
- **Follow-up:** documentation (plan step 6), the measured budgets
  (TKT-0020), and the items below.
- **Assumptions:** nine, listed above with what a different choice would
  have changed.

## Contract additions for ADR follow-up

1. **`McpRuntime::subscribe()`** - a broadcast of `Vec<McpServerSnapshot>`
   published on every lifecycle change. This is a new delivery seam on
   the MCP runtime and extends what TKT-0017 recorded against ADR-012.
2. **`ProviderSetup::reload_config() -> Option<Config>`** and
   **`ControlPlane::reload_config` / `reloaded`** - the configuration
   layer merge stays in the binary (ADR-006) and the plane rebuilds
   around its result. `Saved` still does not mean the running
   configuration changed; this is what makes it change.
3. **`crates/gritt-harness/src/changes.rs`** - a new harness service that
   observes the workspace, including read-only `git` invocations through
   an injected `GitRunner`. Gritt did not previously run `git`. Worth an
   ADR line: the invocation set is fixed, read-only, and never
   interpolates user text into a command position.
4. **`setup::SetupSubmission` and `setup::apply_setup`** - the order the
   profile and the key are written in is now a harness contract: the
   profile first, because a key with no profile is unusable and a
   refused keychain still leaves a usable profile.
5. **`AgentBuilder: Clone` and `ControlPlane: Clone`** - cloning shares
   every handle and copies only the configuration. A rebuilt plane must
   not open a second store, catalog, or MCP runtime.
6. **`run_tui(plane, Option<driver>, draft)`** - the signature changed:
   the plane is owned, the driver is optional for the lazy path, and the
   seed draft comes from the flags above the configured defaults.
7. **The changed-file types moved** from `tui::sidebar` to
   `harness::changes` and are re-exported, so the observer and the
   renderer cannot drift apart.
8. **`App::frames()`** - a frame counter for TKT-0020's deterministic
   timing harness. Nothing in the interface reads it.

## Follow-up

1. **Documentation is plan step 6 and was not done here**, as the ticket
   scope says. `docs/terminal-modes.md` needs the setup flow, the new key
   bindings (Ctrl-T, Ctrl-D in the setup form; Enter on the sidebar),
   the session pinning rule, and the connector limitation.
   `docs/providers.md` and `docs/getting-started.md` are also listed
   there.
2. **The measured responsiveness run is TKT-0020's.** The seams are
   here: `App::on_event` and `App::on_key` take synthetic input,
   `App::frames()` counts what they cost, and `render::draw` works
   against a `TestBackend`. Nothing was timed in this ticket.
3. **The change scan is not incremental.** Every scan runs a full
   `git status --porcelain --untracked-files=all`. On a very large
   repository that is the cost of the sidebar refreshing after a turn. It
   is off the event path, so it costs latency in the sidebar and not in
   the composer, but a watch-based source would be better.
4. **The diff overlay has no word wrap.** A long line is clipped at the
   panel. Horizontal scrolling or wrapping is a small addition.
5. **Cost and context stay unavailable for a connector session.** The
   agent owns its model, so Gritt has no catalog entry to price against.
   That is correct today; a connector that reports usage could change it.
6. **The effort picker before a catalog loads offers nothing explicit**
   on Chat Completions, because capabilities are unreported and
   `effort_support` refuses without them. This is the intended rule, but
   it means `/effort` on a cold start looks emptier than it will be a
   second later. Worth a line in the picker hint.
7. **`McpRuntimeSettings` is still not wired to `config.toml`**
   (TKT-0017). This ticket hit the 30 s initialization deadline again on
   a fresh worktree; see Validation.
8. **TKT-0018's open items are untouched:** no OS clipboard, no mouse
   support, and the grapheme-cursor cost of backward traversal over
   adjacent flag emoji.
9. **The `.cargo/config.toml` `artifact-dir = "."` trap** cost nothing
   this time but is still recorded by TKT-0016 and TKT-0017 as worth its
   own ticket.
