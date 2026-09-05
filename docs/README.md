# Gritt documentation

Gritt is a local terminal application that runs AI coding agents from one
place. Start with [Getting started](getting-started.md). The rest of these
pages each cover one part of the product.

| Page | Covers |
| --- | --- |
| [Getting started](getting-started.md) | Install one binary, configure a provider, plan, approve tools, run coding tasks, resume, inspect telemetry |
| [Configuration template](config.example.toml) | Every config section annotated, ready to copy to `config.toml` |
| [Providers and models](providers.md) | Provider profiles, protocols, model lists, capabilities, aliases, deprecated aliases |
| [Keys](keys.md) | Keychain first, environment second, what never happens to a key |
| [Tools and permissions](tools-and-permissions.md) | Native tools, the allow, ask, deny policy, workspace bounds, the shell authority exception |
| [Terminal modes](terminal-modes.md) | Print, REPL, and full-screen modes, sessions and phases |
| [Connectors](connectors.md) | Codex, Claude Code, Cursor, and OpenCode through one interface |
| [Local database](database.md) | Product storage, memory isolation, migrations, older sessions |
| [Telemetry and analytics](telemetry.md) | What is recorded, what never is, the content log retention rule |
| [Embeddings and reranking](embeddings.md) | The opt-in environment variables |
| [Privacy boundary](privacy.md) | What leaves the machine and what does not |
| [Reproducible builds](reproducible-builds.md) | Building, verifying checksums, release artifacts |
| [Upgrading](upgrading.md) | Additive migrations, inspecting versions, rolling forward |

The product follows the accepted decisions in `.agents/memory/decisions/`
(ADR-006 through ADR-011) and the plan in `.agents/plans/plan1.md`.
