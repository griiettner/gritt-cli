# Agent Tools

Every repository maintenance command is a subcommand of the Rust CLI at
`.agents/cli/` (binary `gritt-agent`). Build it once per checkout:

```bash
cargo build --release --manifest-path .agents/cli/Cargo.toml
```

Then run:

```text
.agents/cli/target/release/gritt-agent memory index
.agents/cli/target/release/gritt-agent memory search "query terms"
.agents/cli/target/release/gritt-agent memory serve
.agents/cli/target/release/gritt-agent ticket new --title "Ticket title"
.agents/cli/target/release/gritt-agent ticket new-chain --title "Chain title" --step "one:First step" --step "two:Second step"
.agents/cli/target/release/gritt-agent ticket identity
.agents/cli/target/release/gritt-agent ticket chain-check --ticket TKT-0008 --base main
.agents/cli/target/release/gritt-agent ticket sync
.agents/cli/target/release/gritt-agent ticket validate
.agents/cli/target/release/gritt-agent skill new skill-name "Skill description"
.agents/cli/target/release/gritt-agent skill sync
.agents/cli/target/release/gritt-agent codex trust --check
.agents/cli/target/release/gritt-agent migrate cursor --source /path/to/repository --dry-run
```

The six Node commands that used to live here were ported into the crate and
removed together with their shared helper modules. One thing was dropped
without a replacement: `frontmatter-utils.mjs` could be run on a single file
to dump its parsed frontmatter as JSON, and no `gritt-agent` subcommand does
that. Run `ticket validate` to surface frontmatter errors instead.
`../cli/README.md` documents every command, its flags, and the verify set.

Run all generated metadata maintenance through the `/tkt-sync` skill.
