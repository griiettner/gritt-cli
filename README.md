# Gritt

Gritt is a local terminal application for running AI coding agents from one
place. Bring a provider key and Gritt runs its own agent loop with tools,
permissions, and resumable sessions. Or point it at an installed agent such
as Codex or Claude Code and Gritt supervises that agent while it keeps its
own tools and authority. Both paths share one interface, one session store,
and one local database. Nothing leaves your machine except the requests you
configure.

It ships as one Rust binary for macOS, Windows, and Linux. User
documentation starts at [`docs/README.md`](docs/README.md), and
[`CONTRIBUTING.md`](CONTRIBUTING.md) covers development and the agent
workspace in this repository.

## Using the CLI

Build from source (Rust toolchain only) or download a release binary, then
configure a provider profile and a key:

```bash
cargo build --release --locked
cp docs/config.example.toml config.toml
export OPENROUTER_API_KEY=...        # or: echo -n "$KEY" | gritt key-set openrouter
./gritt doctor                         # config, keys, database, connectors; never prints a key
```

[`docs/config.example.toml`](docs/config.example.toml) is an annotated
template with every section: profiles, aliases, model-list policy, the
permission policy, connectors, interface, and logging. The config names a
key's keychain entry and environment variable; it never holds the value.

Plan, code, and resume in named sessions:

```bash
gritt run --plan --session refactor "How should we split the parser module?"
gritt run --code --session refactor "Split the parser as we planned"
gritt repl --session refactor
gritt tui --session refactor
gritt session list
```

Coding turns get workspace-bounded file read, file write, and shell tools,
each gated by the `allow`, `ask`, `deny` policy with a diff shown before
any write. Ctrl-C cancels a turn and kills any child process it started.

Run an installed agent through the same sessions and views instead:

```bash
gritt connectors
gritt run --connector codex "Add a test for the alias resolver"
gritt run --connector claude --session review "Review the last commit"
```

`gritt telemetry` prints the local, content-free records. The full guide is
[`docs/getting-started.md`](docs/getting-started.md).

## How Gritt runs agents

Gritt supports two execution paths that feed the same interface and session
history.

### Native path

The user supplies a provider key. Gritt owns the agent loop, provider adapter,
tools, permissions, cancellation, and session continuation.

Supported providers:

| Provider | Protocol | Key source |
| --- | --- | --- |
| OpenRouter | OpenAI-compatible Chat Completions | `OPENROUTER_API_KEY` |
| OpenAI | Responses and Chat Completions | `OPENAI_API_KEY` |
| Anthropic | Messages | `ANTHROPIC_API_KEY` |
| Custom endpoint | OpenAI-compatible Chat Completions | configured variable name |

Provider selection comes from a configured profile. Gritt never guesses the
provider from a model name.

### Connector path

Gritt launches an installed agent CLI and supervises its process while that
agent keeps ownership of its own loop and tools. Gritt translates the agent's
output into the shared event model, relays approvals, handles cancellation, and
stores the session beside native sessions.

The connectors, in the order they were built, are:

1. Native connector
2. Codex
3. Claude Code
4. Cursor and OpenCode

Connectors are optional. A missing or incompatible external CLI must not break
the native path.

## Architecture

Both execution paths produce provider-neutral events for streamed text,
reasoning summaries, tool calls and results, approvals, usage, status changes,
errors, cancellation, and completion.

The main boundaries are:

- `gritt-core` defines events, sessions, tools, configuration, adapters, and connectors without I/O dependencies.
- `gritt-provider` owns HTTP clients, SSE parsing, model-list caching, protocol adapters, normalizers, and tool schemas.
- `gritt-harness` owns the terminal interface, permission engine, session store, built-in tools, and cancellation.
- `gritt-connector` supervises external agent processes and translates their events.
- `gritt` is the binary. It selects modes, loads configuration and keys, and composes the other crates.

The workspace is:

```text
crates/
  gritt-core/
  gritt-provider/
  gritt-harness/
  gritt-connector/
  gritt/
```

Dependency versions are shared from the workspace `Cargo.toml`, and the
toolchain is pinned in `rust-toolchain.toml`. Product code is Rust and the
installed binary has no runtime dependency. Repository maintenance runs
through the separate `gritt-agent` crate at `.agents/cli/`. The repository
has no Node tooling.

### Control plane API

`gritt-harness` exposes a Rust seam that the CLI, REPL, and TUI already
share, and that ADR-011 names as the first non-terminal frontend's API:

- `control::ControlPlane` and `agent::AgentBuilder` own provider and
  profile resolution (with failover and last-used preferences), session
  lifecycle, execution mode, and effort selection.
- `driver::Driver` runs one turn to completion and reports `DriverInfo`;
  `agent::Ui` is the extension point a caller implements to receive the
  normalized `gritt_core::event::Event` stream and answer permission
  decisions (`allow`/`ask`/`deny`) without rendering anything.
- `setup::ProviderSetup` is the injected seam for config-file and keychain
  writes; a read-only or embedded client can use `setup::ReadOnlySetup`.

None of this module set depends on Ratatui, Crossterm, terminal
dimensions, or terminal escape sequences, and the terminal application
does not depend on a future frontend that reuses it.
`crates/gritt-harness/tests/control_plane_client.rs` is a non-terminal
Rust client fixture built directly against this API: it selects a
profile, model, mode, and effort, opens and resumes a session, answers a
permission decision, and consumes the same normalized events the
terminal clients do.

## Terminal modes

Three modes share one session store:

1. Print mode accepts one prompt, streams output, and exits with a meaningful status.
2. REPL mode adds interactive history and session continuation.
3. Full-screen mode adds transcript navigation, approvals, diff review, status, commands, and task views.

Print mode is the fallback. Every feature works there first, so scripts never
need a full-screen terminal.

## Provider layer

Each protocol has its own adapter and response normalizer:

- OpenAI-compatible Chat Completions covers OpenRouter and custom compatible endpoints.
- OpenAI Responses handles response items, streamed `response.*` events, and `previous_response_id` continuation.
- Anthropic Messages handles content blocks and the `message_*` and `content_block_*` event families.

Model lists are fetched by each adapter and cached with a timestamp. A failed
refresh may use the last cached list, marked stale. Capabilities such as tools,
vision, structured output, context length, and pricing are only exposed when
the provider reports them.

## Permissions and keys

Native tools pass through a policy engine with `allow`, `ask`, and `deny`
outcomes. The built-in tools cover workspace file access and approved shell
execution. Shell commands run with the user's authority; a command that
reaches outside the workspace always asks with a stronger prompt. Child
processes are tracked so cancellation can stop them.

Config files may name the environment variable used for a provider key, but
they must never contain the key value. Keys come from the operating system
keychain or the named environment variable. Logs, fixtures, errors, and
transcripts must not expose them.

## Design constraints

- No provider SDKs in the product. Gritt owns its protocol implementations.
- No Git dependencies without an explicit license and maintenance decision.
- No secrets or prompt content in logs by default.
- No connector-specific behavior leaks into provider-neutral contracts.
- No terminal scraping when a structured connector interface is available.

## License

MIT License. Copyright 2026 Paulo Griiettner.
