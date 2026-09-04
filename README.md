# Gritt

Gritt is a local workspace for running AI agent sessions from one place. It
starts as a Rust terminal application and is designed to grow into a local
agent control plane with a desktop or web interface later.

The repository keeps the name `gritt-cli` while the first terminal version is
built.

## Status

Gritt is in its design stage. The agent workspace exists, but the Cargo
workspace and runtime have not been created. The architecture is still being
worked through and requires ADRs before its major choices become canonical.

[`.agents/plans/plan1.md`](.agents/plans/plan1.md) is the current working
proposal. It is input to those decisions, not the source of truth.

## How Gritt runs agents

Gritt supports two execution paths that feed the same interface and session
history.

### Native path

The user supplies a provider key. Gritt owns the agent loop, provider adapter,
tools, permissions, cancellation, and session continuation.

The first provider targets are:

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

The connector order for this machine is:

1. Native connector
2. Codex
3. Claude Code
4. Grok CLI

Connectors are optional. A missing or incompatible external CLI must not break
the native path.

## Proposed architecture

Both execution paths produce provider-neutral events for streamed text,
reasoning summaries, tool calls and results, approvals, usage, status changes,
errors, cancellation, and completion.

The main boundaries are:

- `gritt-core` defines events, sessions, tools, configuration, adapters, and connectors without I/O dependencies.
- `gritt-provider` owns HTTP clients, SSE parsing, model-list caching, protocol adapters, normalizers, and tool schemas.
- `gritt-harness` owns the terminal interface, permission engine, session store, built-in tools, and cancellation.
- `gritt-connector` supervises external agent processes and translates their events.
- `gritt` is the binary. It selects modes, loads configuration and keys, and composes the other crates.

The planned workspace is:

```text
crates/
  gritt-core/
  gritt-provider/
  gritt-harness/
  gritt-connector/
  gritt/
```

Dependency versions will be shared from the workspace `Cargo.toml`. Product
code is Rust and the installed binary has no runtime dependency. Node is used
only by repository maintenance scripts under `.agents/tools/`.

## Terminal modes

Features are built in this order:

1. Print mode accepts one prompt, streams output, and exits with a meaningful status.
2. REPL mode adds interactive history and session continuation.
3. Full-screen mode adds transcript navigation, approvals, diff review, status, commands, and task views.

Print mode is the fallback. Later interface work must not make it unreliable or
require a full-screen terminal.

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
outcomes. The first built-in tools cover workspace file access and approved
shell execution. Child processes are tracked so cancellation can stop them.

Config files may name the environment variable used for a provider key, but
they must never contain the key value. Keys come from the operating system
keychain or the named environment variable. Logs, fixtures, errors, and
transcripts must not expose them.

## Working roadmap

### Phase 0: workspace and contracts

- Establish the Cargo workspace and crate boundaries.
- Define provider-neutral event, session, tool, config, adapter, and connector contracts.
- Prove HTTPS and SSE handling against OpenRouter with recorded fixtures.
- Select a terminal UI crate after reviewing accessibility and platform support.
- Confirm cross-compilation, signing, and release conventions.

### Phase 1: native path

- Add OpenRouter and generic Chat Completions profiles.
- Fetch and cache model lists.
- Build print and REPL modes, sessions, permissions, file and shell tools, and cancellation.
- Stream transcript output and approval prompts in the terminal.

### Phase 2: provider coverage

- Add OpenAI Responses and Anthropic Messages.
- Generate tool schemas per adapter and enforce capability checks.
- Store keys in the system keychain and support secure key entry.

### Phase 3: terminal harness

- Add full-screen navigation, diff review, commands, task views, and child sessions.
- Add provider comparison, usage reporting, diagnostics, and release packaging.

### Phase 4: connectors

- Promote the native path to the connector contract.
- Add process supervision, health checks, approvals, timeouts, and cancellation.
- Connect Codex, Claude Code, and Grok CLI through structured interfaces where available.
- Keep capability differences visible instead of faking parity.

## Agent workspace

This repository includes a local workspace for planning, durable memory,
skills, and ticket history:

```text
.
├── AGENTS.md                 agent boot router
├── CLAUDE.md                 Claude Code entry point
├── .claude/skills/           generated Claude Code skill stubs
└── .agents/
    ├── MODELS.md             CLI and model delegation
    ├── plans/plan1.md        working product proposal
    ├── memory/               durable architecture and decisions
    ├── skills/               canonical reusable procedures
    ├── tasks/                ticket history and backlog
    ├── brain/                agent infrastructure and local RAG
    └── tools/agent-tools/    sync and validation scripts
```

`AGENTS.md` is intentionally short. Agents query `gritt-local-memory` first,
then read the canonical memory, decision, or ticket files returned by the
search. Accepted project knowledge lives under `.agents/memory/`, while plans
remain proposals until an ADR or ticket makes a decision explicit.

Canonical skills live under `.agents/skills/`. Claude Code discovers generated
stubs under `.claude/skills/`, while Codex uses the metadata stored beside each
canonical skill.

## Delegated model roles

Project work rotates across the installed CLIs according to
[`.agents/MODELS.md`](.agents/MODELS.md):

| Role | CLI and model |
| --- | --- |
| Orchestrator | Claude Code, Fable 5.1 medium |
| Implementation | Grok CLI, Grok 4.6 high |
| Reviewer | Codex, GPT 6 Astra. GPT 5.6 Sol medium is the fallback. |
| Reports and ticket writing | Codex, GPT 5.6 Luna |

Delegated workers start cold, so prompts must include the task, constraints,
relevant paths, expected result, and verification requirements.

## Development workflow

Before implementing product code:

1. Read `AGENTS.md`.
2. Query `gritt-local-memory` for relevant decisions and prior work.
3. Read the returned canonical files and the relevant ticket, if one exists.
4. Read `.agents/plans/plan1.md` when the task concerns the proposed product direction.
5. Load the smallest applicable skill before editing.

Once the Cargo workspace exists, the full verification set is:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release
```

Recorded provider fixtures belong in normal tests. Live requests require
`GRITT_LIVE_TESTS=1` and the selected profile's key. Live tests are never
required for a normal pass.

## Agent workspace maintenance

After changing a canonical skill:

```bash
node .agents/tools/agent-tools/sync-skills.mjs
```

After changing memory or ticket files:

```bash
node .agents/tools/agent-tools/tkt-sync.mjs
```

To validate without rewriting generated skill adapters:

```bash
node .agents/tools/agent-tools/sync-skills.mjs --check
node .agents/tools/agent-tools/tkt-validate.mjs .agents/tasks
```

Do not edit generated `.claude/skills/` stubs or generated memory and ticket
indexes by hand.

## Design constraints

- No provider SDKs in the product. Gritt owns its protocol implementations.
- No Git dependencies without an explicit license and maintenance decision.
- No secrets or prompt content in logs by default.
- No later phase starts before the current phase exit criteria are met.
- No connector-specific behavior leaks into provider-neutral contracts.
- No terminal scraping when a structured connector interface is available.

## License

MIT License. Copyright 2026 Paulo Griiettner.
