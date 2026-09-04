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
