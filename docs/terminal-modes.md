# Terminal modes

Print mode is the fallback every feature degrades to. REPL mode adds an
interactive loop. The full-screen mode adds a transcript and views on
Ratatui 0.30.2 with its Crossterm 0.29 backend (ADR-009).

## Print mode

```bash
gritt run [flags] "<prompt>"
```

One prompt in, streamed text on stdout, status on stderr with `--verbose`.
The exit code reflects the result: 0 on completion, 1 on failure, 130 when
the turn was cancelled with Ctrl-C. If stdout breaks (a closed pipe) the
turn stops, running tools are cancelled, and the exit code is 1.

Flags: `--session NAME`, `--profile P`, `--model M`, `--effort LEVEL`,
`--plan` or `--code`, `--approve-all`, `--deny-all`, `--ask`, `--no-models`,
`--connector NAME`, `--verbose`. Global flags: `--workspace PATH`,
`--database PATH`.

A new native session starts on the first usable profile in the configured
fallback order, and on the profile, model, and effort of the last session
that completed when the flags leave them open; the configured defaults
fill whatever is still unnamed. Each
skipped profile and each remembered choice is a `note:` line on stderr,
followed by the profile and model the session runs on when the chain moved.
`--profile` pins the profile. See [Providers](providers.md#startup-failover).

`--mode planning|supervised|auto-approve|full-access` selects native tool
authority. It replaces the phase and approval flags for that invocation.
See [Tools and permissions](tools-and-permissions.md#approval-modes) for
the boundaries of each mode.

## REPL mode

```bash
gritt repl [flags]
```

Lines are prompts. Commands:

| Command | Does |
| --- | --- |
| `/plan`, `/code` | Switch phase; the model is told on the next turn |
| `/mode [NAME]` | Show or change Planning, Supervised, Auto Approve, or Full Access |
| `/sessions` | List sessions |
| `/resume NAME` | Switch to another session |
| `/history` | Show this session's events |
| `/help`, `/quit` or `/exit` | Help and exit |

Ctrl-C cancels the running turn; a second Ctrl-C with nothing running exits.

One reader owns stdin, so approval answers and commands never contend. A
cancelled approval gives up its read; a line typed within its short poll
window may still be consumed as the stale answer, which is a recorded
follow-up. The REPL reads plain lines; there is no arrow-key editing yet.

## Full-screen mode

Shift+Tab cycles through Planning, Supervised, Auto Approve, and Full Access.
`/mode` opens the execution-mode picker. `/mode full-access`, for
example, selects Full Access directly. The header and sidebar show the
effective mode. Finish or cancel a turn or pending approval before changing
it. `/plan` and `/code` remain shortcuts for Planning and Supervised.

```bash
gritt tui [flags]
```

Two layouts. **Home** is what an empty transcript shows: a centred
wordmark, a composer about 90 columns wide, and one status line with the
working directory, connection, model, effort, and phase. With no connection
it says `Use /connect to get started.` The wordmark is dropped on a short
terminal; the composer never is. **Conversation** replaces it after the
first message: a session header, the transcript, the composer, a one-line
footer, and the session information sidebar.

The sidebar is a 30-column column with a 2-column gutter at 110 columns or
more. Below that it collapses and `/sidebar` opens the same information as
a drawer; closing the drawer restores the focus and scroll position it
covered. It shows Session, Model, Usage, Cost, Changed files, and
Integrations. A value Gritt does not know reads `unavailable`, never `0`,
and an integration with no runtime is absent rather than shown as empty.
Cumulative token usage is not context occupancy and is never shown as one.
Cost is an estimate from the model list's prices, never a billed amount,
and it is withheld entirely when either token count or either price is
missing.

Tab moves focus to the sidebar where it is drawn. With the sidebar focused,
the arrow keys and PageUp/PageDown scroll it and Enter opens the changed
files as a searchable picker; choosing one opens a read-only diff. Sidebar
lines are truncated at the column width rather than wrapped, so a long path
or a long failure reason is cut.

### Connecting and provider setup

`/connect` lists configured provider profiles and installed agents in one
picker, along with a `Set up <name>…` row for each known provider that is not
configured yet and a `Custom endpoint…` row. Choosing one of those opens a
setup form: the profile name, the endpoint, the key variable, and the key
itself. `/models` offers the same entry point from the other direction: when
the selected profile has no key, the model list is headed by a
`Set up <profile>…` row that opens the same form, and setup returns to the
picker afterwards without losing your search or your prompt draft. In that form Tab
and the arrow keys move between fields, Ctrl-T cycles the protocol through
`chat_completions`, `responses`, and `messages`, Ctrl-D toggles whether the
profile is saved to the user config or the project config, Enter on the last
field saves, and Escape returns without writing. The profile is written to
the configuration first and the key to the operating system keychain second,
so a refused keychain still leaves a usable profile. The key value is never
written to the config file and never drawn.

Choosing an installed agent instead opens a confirmation that states what
Gritt does and does not control. An agent runs its own harness: Gritt
supervises it and relays its approvals, but its model, effort, and
permissions are the agent's. On a connector session `/connect`, `/models`,
and `/effort` are refused with a notice naming the agent, while `/plan`,
`/code`, `/new`, `/sessions`, `/details`, `/sidebar`, `/mcp`, and `/help`
still work, because those are Gritt's. The sidebar reads `Managed by agent`
in place of the model and effort rows, and the agent's own MCP servers are
reported as not reported rather than being confused with Gritt's.

Selecting a provider clears the model, because a model belongs to one
provider's catalog. An effort the newly selected model cannot take returns
to the model default, with a notice saying so.

### Sessions are pinned to a provider and model

A session that has stored history is pinned to the provider and model it was
opened with. Gritt cannot move a stored transcript and its continuation state
to a different model, so it does not pretend to: choosing another provider or
model on a pinned session opens a notice saying that changing this needs a
new session.

Effort is not pinned. `/effort` works on a live session and takes effect from
the next turn, because it changes what the next request asks for and not what
the stored transcript means.

The refusal happens before the choice is applied, so the draft still holds the
session's own provider and model afterwards. The order to change model is
therefore `/new` first, then `/models`: `/new` clears the transcript view, the
session identity, and the usage totals, keeps your composer draft, and leaves
the previous session in `/sessions`; the selection you make after it opens the
next session. Selecting first and running `/new` afterwards does not carry the
rejected choice across.

The same rule is enforced twice on purpose: once in the interface, for an
immediate answer, and once in the control plane, which refuses the draft
outright. Resuming a session restores the provider, model, effort, phase,
transcript, and continuation state it was left with.

### Commands

Typing `/` at the start of the composer opens filtered suggestions and
Ctrl-P opens the same registry as a searchable palette; both dispatch
through one table, so a command cannot exist in one and not the other.

| Command | Does |
| --- | --- |
| `/connect` | Choose an AI provider profile or an installed agent |
| `/models`, `/model` | Search the selected provider's models |
| `/effort` | Choose the reasoning effort for native turns |
| `/plan`, `/code` | Switch phase |
| `/sessions`, `/resume` | Search and resume sessions |
| `/new` | A fresh draft; the previous session is kept |
| `/details` | Expand or collapse tool output |
| `/sidebar` | Toggle the sidebar, or open its drawer |
| `/mcp` | Every configured MCP server and its state |
| `/help` | Commands, keys, and current limitations |
| `/quit` | Leave and restore the terminal |

Commands are handled locally and never become prompts. An unknown command
shows a local error and keeps what you typed. `//` escapes a literal
leading slash. Pasted multiline content is always text: it cannot run a
command.

### Keys

Enter sends, Shift-Enter, Alt-Enter, or Ctrl-J inserts a newline. Ctrl-M is
not bound on its own because terminals can encode it as Enter. Tab
completes a highlighted suggestion, or moves focus between the composer,
transcript, and sidebar. Escape closes the top overlay first and only then
cancels a running turn; an approval sits above every overlay, and a
cancelled approval cannot be answered by a late key. Ctrl-P opens the
palette, Ctrl-S the session list, Ctrl-G returns to the latest output,
Ctrl-Y copies the selection or draft into Gritt's own buffer (not the
system clipboard), Ctrl-A, Ctrl-W, and Ctrl-U select all, delete a word,
and delete to the line start. Ctrl-C cancels a running turn or quits when
idle; Ctrl-Q quits. In the approval view `y` approves, `n` or Esc denies,
and `d` shows the diff.

Shift-Enter and Alt-Enter work only where the terminal reports the modifier
distinctly; Ctrl-J always works and is the one the key hints name. Escape
also cancels background work such as a catalog load, an MCP action, or a
session that is still opening, before it reaches a running turn.

In a picker, typing filters the list, so `j` and `k` are ordinary filter
characters rather than movement. Arrow keys, Tab, PageUp/PageDown, and
Ctrl-N/Ctrl-P move the highlight, Enter chooses, and Escape closes.
Choosing a row that is unavailable does nothing except show that row's own
reason. In the sidebar drawer and the help and diff overlays, `j` and `k`
or the arrow keys scroll and Escape closes. The diff overlay does not wrap:
a long line is clipped at the panel edge, and there is no horizontal
scrolling.

Streaming follows the bottom only while you are already there. Scrolling up
holds the viewport and shows a new-output indicator; Ctrl-G is the explicit
way back. Escape sequences in tool or model output are drawn as text, never
executed.

An idle session draws nothing at all. Nothing in the interface is animated
from a clock, so the terminal receives no bytes until input arrives or the
harness reports something.

The mode honors terminal resize, `NO_COLOR`, and `GRITT_THEME=light|dark`,
uses the alternate screen with bracketed paste, and restores the terminal on
exit and on panic. On a narrow terminal it reduces margins and drops
secondary status before it takes space from the input or the transcript. It
needs a terminal on both stdin and stdout; otherwise it exits with an error
and print mode remains available.

### MCP servers

`/mcp` lists every entry in the workspace `.mcp.json` with its state and its
tool count, and choosing one offers the actions that apply to it. Reading the
file does not authorize running it: an entry stays in `awaiting approval`
until you approve that exact definition for that exact workspace. Approving
shows the redacted definition first, which is the command and its arguments
or the endpoint without its query, plus the names of the environment
variables and headers it declares but never their values. Approving records
the decision and then starts the server, the same decision `gritt mcp trust`
records from the command line.

Every entry is accounted for. A server on an unsupported transport, one that
answers with a protocol revision Gritt does not speak, or one that fails to
start keeps a visible reason rather than disappearing from the list. MCP
servers are started only on the native path; a connector session does not
open the runtime. [Tools and permissions](tools-and-permissions.md) covers
the trust record, the permission default, and the timeouts.

### Fixture mode

```bash
gritt tui --fixture home
gritt tui --fixture conversation
```

Opens a labelled design-review screen built from invented data. It creates
no session, sends no provider request, and starts no MCP server; the status
line reads `fixture` so a screenshot cannot be mistaken for live data. It
exists for reviewing layout, spacing, and keyboard navigation before the
control plane is connected.

## Sessions and phases

Sessions are named, listable, resumable, and removable, whichever path
produced them. Each carries a phase. Native Planning offers workspace file
reading; Coding offers tools under the selected execution mode. Switching
phase sends the model a transition note on the
next turn, also after resuming a session that changed phase before exit.
Continuation state an adapter needs is stored behind the session interface.
