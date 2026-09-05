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

Flags: `--session NAME`, `--profile P`, `--model M`, `--plan` or `--code`,
`--approve-all`, `--deny-all`, `--ask`, `--no-models`, `--connector NAME`,
`--verbose`. Global flags: `--workspace PATH`, `--database PATH`.

## REPL mode

```bash
gritt repl [flags]
```

Lines are prompts. Commands:

| Command | Does |
| --- | --- |
| `/plan`, `/code` | Switch phase; the model is told on the next turn |
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

Streaming follows the bottom only while you are already there. Scrolling up
holds the viewport and shows a new-output indicator; Ctrl-G is the explicit
way back. Escape sequences in tool or model output are drawn as text, never
executed.

The mode honors terminal resize, `NO_COLOR`, and `GRITT_THEME=light|dark`,
uses the alternate screen with bracketed paste, and restores the terminal on
exit and on panic. On a narrow terminal it reduces margins and drops
secondary status before it takes space from the input or the transcript. It
needs a terminal on both stdin and stdout; otherwise it exits with an error
and print mode remains available.

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
produced them. Each carries a phase. Planning is conversation only. Coding
offers tools. Switching phase sends the model a transition note on the
next turn, also after resuming a session that changed phase before exit.
Continuation state an adapter needs is stored behind the session interface.
