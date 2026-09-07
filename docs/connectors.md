# Connectors

A connector runs an installed agent through the same sessions, transcript,
telemetry, and modes as the native path. The native path is itself the
first connector, so the control plane never special-cases it. Order and
authority follow ADR-010.

```bash
gritt connectors
gritt run --connector codex "…"
gritt repl --connector claude
gritt tui --connector opencode
```

| Connector | `--connector` | Interface | State on this machine |
| --- | --- | --- | --- |
| Native | `native` | in process | always available |
| Codex | `codex` | `codex exec --json` | live-tested |
| Claude Code | `claude` | `claude -p --output-format stream-json --verbose` | live-tested |
| OpenCode | `opencode` | `opencode run --format json` | runs, auth unknown |
| Cursor | `cursor` | `cursor-agent -p --output-format stream-json` | fixtures only, CLI not installed |

## Authority

The external agent keeps its own command and tool authority. Gritt
launches it, supervises the process, normalizes its output into the shared
event model, records the session, and handles follow-up input, resume,
cancellation, health checks, and process failure. Gritt does not re-run the
agent's tools or second-guess its permissions.

## Approvals are the agent's own

The headless interfaces of all four external agents apply the agent's own
approval policy and do not expose approvals to a supervisor. Gritt shows
this instead of faking it: `gritt connectors` lists `own-approvals`, and
the native `--approve-all`, `--deny-all`, and `--ask` flags print a warning
and have no effect with an external connector. Pass the agent's own
permission flags through the config:

```toml
[connectors.extra_args]
codex = ["--full-auto"]
claude = ["--permission-mode", "acceptEdits"]
```

An `extra_args` entry that carries a credential (an option named like a
key, token, secret, or password, or any value matching a known secret) is
refused, because arguments appear in process lists and diagnostics.

## Planning is a request

A planning session on a connector prefixes a planning request to the
prompt. It asks the agent to discuss rather than act; it does not remove
the agent's tools. Use the native path when planning must be tool-free.

## Settings

```toml
[connectors]
# Executable overrides, by connector id.
executables = { codex = "/opt/codex/bin/codex" }
# Use a pseudo-terminal instead of pipes for these connectors.
pty = ["opencode"]
health_check_timeout_secs = 10
task_timeout_secs = 1800
```

Every field has a default; a partial section is valid.

## Supervision

Startup, idle, and task timeouts (the first two are built in, the task
timeout is configurable); health checks (version and auth probes
before a task starts); cancellation that kills the process tree; process
exit, non-zero exit, missing executable, outdated version, and malformed
output are surfaced as normalized error or status events. A terminal event
followed by a non-zero exit is reported as an error. After a terminal event
the process is drained until it exits, bounded by one deadline. Raw
connector output is kept in event diagnostics for troubleshooting,
key-redacted.

A missing or broken external agent fails only its own sessions and never
creates a session row. The native path is unaffected.

## Models

Print, REPL, and the full-screen UI share one control-plane operation that
asks an installed agent for the models it currently exposes, then passes an
explicit choice through that agent's documented `--model` flag.

| Connector | List | Select |
| --- | --- | --- |
| Codex | `codex debug models` | `codex exec --model <id>` |
| Claude Code | none documented (shown as unsupported) | `claude --model <id>` |
| Cursor | `cursor-agent --list-models` | `cursor-agent --model <id>` |
| OpenCode | `opencode models` | `opencode run --model <provider/id>` |

`--model` on `gritt run`, `gritt repl`, and `gritt tui` is that explicit
choice for a new connector session. A resumed session keeps the model
stored with it. When listing fails, Gritt uses the last cached catalog and
marks it stale rather than presenting it as current. A missing CLI,
unsupported listing command, failed command, or unreadable output is a
typed diagnostic and does not affect native sessions or other connectors.

## The agent's own MCP servers

An external agent keeps its own MCP servers, approvals, and logins
(ADR-010). Gritt does not manage them, but it does show them: opening a
connector session runs that agent's documented listing command once, in
the session workspace so project-scoped servers count, and reports every
server with a normalized status, through one control-plane operation
print, REPL, and the full-screen UI share.

| Connector | List | Status source |
| --- | --- | --- |
| Codex | `codex mcp list --json` | configuration only: `enabled`, `disabled`, or `needs auth` when Codex reports `not_logged_in` |
| Claude Code | `claude mcp list` | Claude's own live health check: `connected`, `failed`, `pending approval`, `needs auth`, `disabled` |
| Cursor | none machine-readable (shown as unsupported) | `cursor-agent mcp list` opens an interactive menu, which Gritt does not scrape |
| OpenCode | `opencode mcp list` | OpenCode's own check: `connected`, `failed`, `disabled`, `needs auth` |

The inventory is display only. Nothing here adds, removes, enables,
approves, or connects to a server; use the agent's own CLI for that.
Gritt's own MCP list (`.mcp.json`, `/mcp`, `gritt mcp`) is unaffected and
is always shown apart from the agent's, under `MCP owned by Gritt` in the
sidebar and as its own startup note in print and REPL mode, so one
agent's server is never mistaken for another's.

Only a server's name, transport, launch command or URL, status, and the
agent's own hint are kept, as display text. A credential-looking option
value in the command (`--api-key ...`), a URL's userinfo and query string,
and any value Gritt knows to be a secret are redacted before storage, and
environment values, headers, and argument vectors are never read from the
listing at all.

The read is bounded by the connector's health check timeout and is not
cached: a missing CLI, an unsupported listing, a failed command, a
timeout, or unreadable output is its own typed diagnostic, leaves the
session usable, and never affects native sessions or another connector.
The full-screen UI reads the inventory in the background after the
session opens and shows `checking` until it lands.

## Versions and updates

Gritt can tell when an installed agent CLI is behind the newest version its
installer publishes, and can run that installer's documented update command
after you approve it. Print, REPL, and the full-screen UI share one
control-plane operation for the check and one for the update.

Who installed the executable is read from evidence on disk, never guessed
from a path prefix: a Homebrew `Cellar` or `Caskroom` directory behind the
symlink, an npm package directory with its `package.json`, a pipx venv with
its `pipx_metadata.json`, a Cargo `.crates.toml` entry naming the binary,
or the vendor installer's own directory under your home directory. Two
owners with evidence are reported as ambiguous and no command is offered;
no evidence is reported as unknown with the same result.

| Owner | Newest version from | Update command |
| --- | --- | --- |
| Homebrew formula | `brew info --json=v2 <name>` | `brew upgrade <name>` |
| Homebrew cask | `brew info --json=v2 <name>` | `brew upgrade --cask <name>` |
| npm | `npm view <package> version` | `<npm path> install -g --prefix <verified prefix> <package>@latest` |
| Cargo | `cargo search <crate> --limit 1` | `cargo install <crate>` |
| pipx | not published | `pipx upgrade <package>` |
| Claude Code native installer | not published | `claude update` |
| Cursor CLI installer | not published | `cursor-agent update` |
| OpenCode install script | not published | `opencode upgrade` |

Every command is an executable plus a fixed argument vector. It is shown to
you exactly as it runs and never joined into a shell string. Nothing from a
prompt, a credential, or the agent's output enters it.

For npm, Gritt also checks `npm root --global` and verifies that the selected
executable belongs to that installation. Local packages and installations
owned by another npm executable offer no update. The approved command fixes
both the npm executable and its installation prefix.

- `gritt connectors --check` reports each installed agent's version, owner,
  newest published version, and the command Gritt would run. `--refresh`
  queries again instead of using a cached answer.
- `gritt connectors --update <name>` shows the command and asks before
  running it; `--yes` answers for you. A successful update is followed by a
  fresh check.
- In the REPL, `/version [refresh]` and `/update` do the same; `/update`
  asks `[y/N]` on the prompt.
- In the full-screen UI, the sidebar's model block shows the CLI version
  state for a connector session, `/version` checks again, and `/update`
  opens the same approval overlay a tool call uses.

The check is advisory and never delays a session. Opening a connector
session reads the installed version and owner from local evidence and the
newest version from the cache only; the full-screen UI then checks in the
background. A successful answer is cached for the model list refresh
interval and shown with its age. When a query fails the last answer is kept
and marked stale, and a stale answer is never presented as a current update
offer. Network, authentication, malformed, and unsupported-source failures
are reported by class; the text a package manager printed stays in the
process.

Version caches belong to an executable, installation source, and querying
manager. Changing those discards the old answer. Older cache entries without
that identity are refreshed before use.

An update runs through the same supervised process path as an agent task:
it is cancellable, stops after a bounded silence, and its process tree is
killed on cancellation or timeout. Failure reports contain the command and
exit status; package-manager stdout and stderr are discarded because they can
contain credentials Gritt does not know. Ctrl-C cancels an update in the CLI
or REPL and waits for cleanup. The REPL then accepts another command.

## PTY fallback

Machine-readable output is preferred. A connector listed under
`[connectors] pty` runs in a pseudo-terminal instead of pipes, for agents
whose structured mode needs one. Terminal scraping is not implemented; the
PTY path still parses the agent's structured output.

## Live tests

`GRITT_LIVE_CONNECTOR_TESTS=1 cargo test -p gritt-connector --test live`
runs a trivial task through the real Codex and Claude Code CLIs when they
are installed and authenticated. Fixture tests cover the same behavior
without them and are never skipped.
