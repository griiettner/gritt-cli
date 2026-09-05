# Contributing

This file covers development, verification, and the agent workspace that
lives beside the product code. User documentation is under `docs/`.

## Development workflow

Before implementing product code:

1. Read `AGENTS.md`.
2. Query `gritt-local-memory` for relevant decisions and prior work.
3. Read the returned canonical files and the relevant ticket, if one exists.
4. Read `.agents/plans/plan1.md` when the task concerns the proposed product direction.
5. Load the smallest applicable skill before editing.

The full verification set is:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release --locked
```

The dated nightly in `rust-toolchain.toml` enables Cargo's configured root
artifact output. An ordinary release build produces `./gritt` (`gritt.exe`
on Windows). For workspace checks, set `CARGO_BUILD_ARTIFACT_DIR` to
`target/verification-artifacts` to keep test executables and libraries out
of the project root, as CI does.

Recorded provider fixtures belong in normal tests. Live requests require
`GRITT_LIVE_TESTS=1` and the selected profile's key. Live tests are never
required for a normal pass.

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
    └── cli/                  gritt-agent maintenance CLI (Rust)
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

## Agent workspace maintenance

Use the bundled `.agents/gritt-agent` on a matching platform. Rebuild only after
changing the maintenance CLI source:

```bash
cargo build --release --locked --manifest-path .agents/cli/Cargo.toml --bin gritt-agent
```

The binary lands at `.agents/gritt-agent` and is also
the `gritt-local-memory` MCP server declared in `.mcp.json`.
Commit the executable with source changes so other users do not need to build.
The currently bundled executable targets macOS Apple Silicon; other platforms
need their matching prebuilt executable.

After changing a canonical skill:

```bash
.agents/gritt-agent skill sync
```

After changing memory or ticket files:

```bash
.agents/gritt-agent ticket sync
```

To validate without rewriting generated skill adapters:

```bash
.agents/gritt-agent skill sync --check
.agents/gritt-agent ticket validate
```

To refresh or query local memory from the terminal:

```bash
.agents/gritt-agent memory index
.agents/gritt-agent memory search "query terms"
```

Do not edit generated `.claude/skills/` stubs or generated memory and ticket
indexes by hand.
