# Agent Tools

Repository automation is implemented with Node.js. The scripts under `agent-tools/` use only Node built-ins and run directly with `node`. No install, Python, or Nx is required. If an Nx workspace is added later, the same scripts can be exposed as `agent-tools:*` targets and the sync helper will prefer them.

## Common commands

```text
node .agents/tools/agent-tools/memory-index.mjs
node .agents/tools/agent-tools/memory-dashboard.mjs
node .agents/tools/agent-tools/sync-skills.mjs
node .agents/tools/agent-tools/tkt-sync.mjs
node .agents/tools/agent-tools/tkt-validate.mjs
node .agents/tools/agent-tools/tkt-identity.mjs
node .agents/tools/agent-tools/tkt-new.mjs --title "Ticket title"
```

Pass command-specific arguments after `--`:

```text
node .agents/tools/agent-tools/tkt-chain-check.mjs --ticket TKT-0008 --base main
node .agents/tools/agent-tools/tkt-new.mjs --title "Ticket title"
node .agents/tools/agent-tools/tkt-new-chain.mjs --title "Ticket title" --create-plan
node .agents/tools/agent-tools/create-skill.mjs skill-name "Skill description"
node .agents/tools/agent-tools/trust-codex-project.mjs --check
node .agents/tools/agent-tools/migrate-cursor-setup.mjs --source /path/to/repository --dry-run
```

Run all generated metadata maintenance through the `/tkt-sync` skill.

## Layout

- `agent-tools/` contains the cross-platform Node command implementations.
- `agent-memory/` contains the local Turso/libSQL memory implementation.

Targets that write files or run persistent services deliberately disable Nx
caching. The tools use Node APIs and Git subprocesses directly, without Bash,
PowerShell, or operating-system-specific command syntax.
