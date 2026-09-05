# Getting started

## Install

Gritt ships as one native binary per platform with no runtime dependency.
Download the archive for your platform from a release, verify its checksum
as described in [Reproducible builds](reproducible-builds.md), and put the
binary on your `PATH`.

To build from source, install Rust through rustup. The repository selects
the dated nightly toolchain in `rust-toolchain.toml` automatically:

```bash
cargo build --release --locked
./gritt --version
```

Cargo places the actual executable at the source checkout root, using the
`build.artifact-dir` setting in `.cargo/config.toml`.
On Windows, run `.\gritt.exe --version`. To install in another project, take
the executable for that platform and a `config.toml`. The `target/` directory
is only a build cache and is not needed to run Gritt.

## Configure a provider

Gritt routes every request through a configured provider profile. It never
guesses the provider from a model name. Create a project config at
`config.toml` in your workspace, or a user config at
`~/.config/gritt/config.toml` (Linux), `~/Library/Application
Support/gritt/config.toml` (macOS), or `%APPDATA%\gritt\config.toml`
(Windows):

```toml
default_profile = "openrouter"
default_model = "openai/gpt-5-nano"

[profiles.openrouter]
name = "openrouter"
protocol = "chat_completions"
base_url = "https://openrouter.ai/api/v1"

[profiles.openrouter.key]
keychain_service_entry = "gritt/openrouter"
env_var_name = "OPENROUTER_API_KEY"
```

A complete, annotated template with every section is at
[config.example.toml](config.example.toml); copy it to `config.toml` and
delete what you do not need.

The config names the key. It never holds the key value, and a config that
contains one fails to load. See [Keys](keys.md) for storing a key in the
operating system keychain with `gritt key-set openrouter`, and
[Providers and models](providers.md) for OpenAI, Anthropic, and generic
OpenAI-compatible profiles.

Check the result:

```bash
gritt config
gritt doctor
```

`gritt doctor` reports config locations and precedence, key availability
per profile, the database path and migrations, model cache freshness,
installed connectors, and terminal capabilities. It never prints a key.

## Plan

Planning is a conversation. Tools are not offered to the model.

```bash
gritt run --plan --session refactor "How should we split the parser module?"
```

Every session is named. `--session` creates the session on first use and
resumes it afterwards. Without `--session` Gritt generates a name and
prints it with `--verbose`.

## Run coding tasks and approve tools

Coding is the tool-using phase. The model can read and write files inside
the workspace and run shell commands, each gated by the permission policy.

```bash
gritt run --code --session refactor "Split the parser as we planned"
```

When a tool needs approval, print mode asks on the terminal:

```text
file_write src/parser/mod.rs (ask): the user reviews a diff first
--- src/parser/mod.rs
+++ src/parser/mod.rs
...
approve? [y/N]
```

Without a terminal on stdin, print mode denies every request that would
ask, so scripts stay safe. Pass `--approve-all` to approve everything or
`--deny-all` to deny everything. [Tools and permissions](tools-and-permissions.md)
explains the policy and how to change it.

## Interactive modes

```bash
gritt repl --session refactor
gritt tui --session refactor
```

The REPL adds history and the `/plan`, `/code`, `/sessions`, `/resume`,
`/history`, and `/quit` commands. The full-screen mode adds a streamed
transcript with tool activity, a status bar, approval and diff views, a
command palette, and a session list. Both are described in
[Terminal modes](terminal-modes.md).

## Resume and manage sessions

```bash
gritt session list
gritt session show refactor
gritt session rename refactor parser-split
gritt session remove parser-split
```

A session records its workspace. Resuming it from another directory is
refused, so a session never runs tools against the wrong project.

## Cancel

Press Ctrl-C in print or REPL mode, or Esc in the full-screen mode. The
provider request is dropped, any child process the tool started is killed,
and the session records a cancelled turn. The next turn continues the same
session.

## Inspect telemetry

```bash
gritt telemetry
```

Telemetry and analytics are local and content-free: names, ids, durations,
token counts, tool names, and outcomes. Prompts, file content, shell output,
and keys are never recorded. See [Telemetry and analytics](telemetry.md).

## Use an installed agent instead

```bash
gritt connectors
gritt run --connector codex "Add a test for the alias resolver"
gritt run --connector claude --session review "Review the last commit"
```

Codex, Claude Code, Cursor, and OpenCode run through the same sessions,
transcript, and modes while keeping their own command and tool authority.
See [Connectors](connectors.md).
