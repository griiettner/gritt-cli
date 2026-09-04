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

Views: the streamed transcript with tool activity, a multiline prompt
editor, a status bar (model, profile or connector, session, phase, usage,
connection), an approval view showing tool, target, arguments, and reason, a
diff review before file writes, a command palette, and a session list.

Keys: Enter sends, Shift-Enter, Alt-Enter, or Ctrl-J inserts a newline,
Esc cancels a running turn or closes a view, Ctrl-P opens the palette,
Ctrl-S the session list, Ctrl-C cancels a running turn or quits when idle,
Ctrl-Q quits. In the approval view `y` approves, `n` or Esc denies, and
`d` shows the diff.

The mode honors terminal resize and `NO_COLOR`, uses the alternate screen,
and restores the terminal on exit and on panic. It needs a terminal on both
stdin and stdout; otherwise it exits with an error and print mode remains
available.

## Sessions and phases

Sessions are named, listable, resumable, and removable, whichever path
produced them. Each carries a phase. Planning is conversation only. Coding
offers tools. Switching phase sends the model a transition note on the
next turn, also after resuming a session that changed phase before exit.
Continuation state an adapter needs is stored behind the session interface.
