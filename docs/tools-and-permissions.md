# Tools and permissions

## Native tools

The native path offers three tools during the coding phase:

| Tool | Does | Default outcome |
| --- | --- | --- |
| `file_read` | Reads a UTF-8 file inside the workspace | allow |
| `file_write` | Replaces a file inside the workspace after diff review | ask |
| `shell` | Runs a command line in the workspace root and returns its output | ask |

File paths are resolved against the workspace root. Absolute paths outside
it, `..` traversal, and symlinks that escape are refused before the policy
runs. A file write shows a unified diff against the current content and
applies only after approval. Shell commands run in their own process group
and are tracked so cancellation kills them.

## MCP server tools

Gritt reads `<workspace>/.mcp.json` directly and never writes it. Every entry
under `mcpServers` is treated the same way, whatever it is named: a stdio
entry with `command`, `args`, and optional `env`, or a `type: http` entry with
a `url` and optional `headers` for Streamable HTTP. Any other transport,
including legacy standalone SSE, is reported as unsupported rather than
skipped. `${VAR}` and `${VAR:-default}` resolve from the launch environment
without running a shell, and a field whose name looks like a credential must
hold a plain `${VAR}` reference; a literal is refused without echoing it
(ADR-008).

Reading the file does not authorize running it. A server stays in
`awaiting approval` until its exact definition is approved for that exact
workspace:

```bash
gritt mcp list              # every entry with its state and tools
gritt mcp trust <server>    # approve the definition as it stands now
gritt mcp trust <server> --deny
gritt mcp forget            # forget every decision for this workspace
```

Editing the entry changes its fingerprint, so the approval no longer applies
and Gritt asks again. The fingerprint covers the entry's content, not the file
around it: reformatting, reindenting, or reordering keys leaves it unchanged,
and so does rotating the value of a variable the entry names, because the
approval is over the `${VAR}` reference rather than what it resolves to. A child gets the workspace as its working directory,
its argument array verbatim, and a minimal environment plus the variables the
entry declares; provider keys configured for Gritt are never passed on.

Discovered tools reach the model as ordinary function tools named
`mcp__<server>__<tool>`, so two servers offering `search` both stay callable.
Every call passes the policy engine first, exactly like a native tool. The
default rule is:

```toml
[[policy.rules]]
tool = "mcp__*"
resource = "*"
outcome = "ask"
```

Narrow it per server or per tool by matching the resource, which is
`mcp:<server>/<tool>`:

```toml
[[policy.rules]]
tool = "mcp__docs__*"
resource = "mcp:docs/search"
outcome = "allow"
```

A server's own `readOnlyHint` and the rest of its annotations are display
information. They never grant permission: the specification says a client
must treat annotations from an unvetted server as untrusted.

Gritt speaks MCP revisions 2025-06-18 (offered), 2025-03-26, and 2024-11-05.
A server that answers with anything else is disconnected with a stated
reason. Planning turns carry no tools at all, MCP included.

### Timeouts, limits, and cancellation

One slow or hostile server must not hold the rest. Every server runs under
the same bounds:

| Bound | Default | Applies to |
| --- | --- | --- |
| Initialization | 30 s | Launch, `initialize`, and the first `tools/list` together |
| One call | 120 s | A single `tools/call` |
| Shutdown grace | 5 s | How long a stdio child may take to exit after its stdin closes, before termination escalates |
| Concurrent starts | 4 | How many servers initialize at once; the rest queue. There is no cap on how many entries a file may declare |
| Result size | 64 KiB | The largest tool result text handed back to the model |
| List pages | 100 | A guard against a server that never stops paginating |

A server that misses its initialization deadline is marked failed with that
reason and the others carry on; a failed server is not restarted
automatically and a call is not retried after a disconnect. A single line of
server output above 8 MiB is treated as a protocol violation.

These values are not yet settable in `config.toml`, and that is the one place
where the initialization default bites. A server that does substantial work
before answering `initialize`, such as building an index on its first run in a
fresh checkout, can exceed 30 seconds and be stopped partway. Retrying does
not necessarily recover: if the work is not resumable, the next start begins
it again and hits the same deadline, so the entry can fail indefinitely while
being perfectly healthy. Gritt's own server does this, and until the deadline
is configurable the way out is to let that server complete its first run
outside Gritt.

Cancelling a turn cancels its MCP calls. A call that has already been sent is
withdrawn with `notifications/cancelled` so the server can stop work, and a
call still waiting to be sent is never sent at all. Cleanup does not block
the interface. Stdio children are started in their own process group and are
terminated through the group, so a server that spawns its own helpers does
not leave them behind when Gritt quits or is interrupted.

## The policy engine

Every native tool execution passes the policy engine first, every time.
There is no bypass path. The engine returns `allow`, `ask`, or `deny` from
the first rule that matches the tool name and resource. Rules support `*`,
`**`, and `?` wildcards and a `workspace:` prefix for paths relative to the
workspace root.

The defaults (ADR-009):

- file reads inside the workspace: allow
- file writes: ask
- shell: ask
- network: ask
- destructive operations (`rm -rf`, `git push --force`, redirects that
  truncate, and similar): ask with a stronger prompt
- anything outside the workspace: deny

Override them in the config. Rules are evaluated in order, the first match
wins, and `policy.fallback` applies when nothing matches:

```toml
[[policy.rules]]
tool = "file_write"
resource = "workspace:docs/**"
outcome = "allow"
reason = "docs are always writable"

[[policy.rules]]
tool = "shell"
resource = "cargo test*"
outcome = "allow"
```

Every approval prompt shows the tool, the target, the relevant arguments,
and a one-line reason. The transcript records the decision. Without content
logging, approval events store the tool name, resource, and decision only.

## The shell authority exception

Shell commands run with your authority. They are started in the workspace
root, but the operating system does not confine them to it: `cat
/etc/hosts` reads `/etc/hosts`. This is a recorded exception to the
workspace bound (TKT-0011). Gritt guards it three ways:

- the approval prompt for shell states that the command may reach outside
  the workspace;
- a command whose tokens contain an absolute path outside the workspace,
  `..` traversal, `~`, or a shell variable expansion is classified as
  destructive and gets the stronger prompt, and an `allow` rule cannot
  downgrade it below `ask`;
- credential variables are removed from the child environment and tool
  output is key-redacted before it reaches the model or the store.

If you need confinement, run Gritt inside a container or sandbox of your
choice.

## Approval modes

| Mode | Behavior |
| --- | --- |
| `--ask` (default with a terminal) | Prompt on stdin |
| no terminal on stdin | Every `ask` is denied |
| `--approve-all` | Every `ask` is approved |
| `--deny-all` | Every `ask` is denied |

These flags apply to the native path. External connectors apply their own
approval policy; see [Connectors](connectors.md).
