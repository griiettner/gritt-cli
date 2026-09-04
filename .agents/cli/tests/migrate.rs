mod common;

use common::{empty_repo, fixtures_dir, read, run, write};

const MARKER: &str = "<!-- MIGRATED BY gritt-agent migrate cursor; DO NOT EDIT -->";

fn source() -> String {
    fixtures_dir()
        .join("cursor-source")
        .to_str()
        .unwrap()
        .to_owned()
}

#[test]
fn cursor_migration_plans_writes_skips_and_reports() {
    let repo = empty_repo();
    let source = source();
    let dry = run(
        &repo.root,
        &[
            "migrate",
            "cursor",
            "--source",
            &source,
            "--dry-run",
            "--no-sync",
        ],
    );
    dry.assert_status(0);
    assert_eq!(
        dry.stdout,
        "cursor/claude migration planned\nwrites: 9\nskipped: 0\nambiguous: 1\n"
    );
    assert!(!repo.root.join(".agents/skills").exists());
    assert!(!repo.root.join(".agents/migrations").exists());

    let real = run(
        &repo.root,
        &["migrate", "cursor", "--source", &source, "--no-sync"],
    );
    real.assert_status(0);
    assert_eq!(
        real.stdout,
        "cursor/claude migration migrated\nwrites: 9\nskipped: 0\nambiguous: 1\n"
    );
    let skill = read(&repo.root, ".agents/skills/example/SKILL.md");
    assert!(skill.starts_with(&format!(
        "---\nname: example\ndescription: \"Example imported skill.\"\ndisable-model-invocation: true\n---\n\n{MARKER}\n<!-- source: .cursor/skills/example.md -->\n<!-- source_system: cursor -->\n\n# Example\n\nUse the example flow.\n"
    )));
    assert!(
        read(&repo.root, ".agents/skills/example/agents/openai.yaml").starts_with(&format!(
            "# {MARKER}\n# source: .cursor/skills/example.md\n"
        ))
    );
    let command_skill = read(&repo.root, ".agents/skills/review/SKILL.md");
    assert!(command_skill.contains("description: \"Review the diff.\"\n"));
    assert!(
        command_skill.contains("<!-- source_system: claude -->\n\n# Review\n\nReview the diff.\n")
    );
    let agent = read(&repo.root, ".agents/agents/helper.md");
    assert!(agent.contains("id: helper\ntitle: \"Helper\"\ntype: agent\nstatus: active\n"));
    assert!(agent.contains("tags:\n  - imported\n  - cursor\n---\n"));
    let decision = read(&repo.root, ".agents/memory/decisions/decide.md");
    assert!(decision.contains("type: decisions\n"));
    assert!(decision
        .contains("- Classification confidence: `high`\n- Source: `.cursor/rules/decide.mdc`\n"));
    let plain = read(&repo.root, ".agents/memory/architecture/plain.md");
    assert!(plain.contains(
        "- Classification confidence: `low`\n- Review note: no strong category keywords found\n"
    ));
    assert!(!repo.root.join(".agents/skills/ignored").exists());

    let report = read(&repo.root, ".agents/migrations/cursor-migration-report.md");
    assert!(report.contains("- Planned writes: `7`\n- Skipped: `0`\n- Ambiguous: `1`\n"));
    assert!(report.contains(
        "- `skill` `.agents/skills/example/SKILL.md` from `.cursor/skills/example.md`\n"
    ));
    assert!(report.contains("## Ambiguous\n\n- `.agents/memory/architecture/plain.md` from `.claude/memory/plain.txt`: no strong category keywords found\n"));
    assert!(report.ends_with("## Maintenance Commands\n\n- Not run yet\n"));
    let manifest: serde_json::Value = serde_json::from_str(&read(
        &repo.root,
        ".agents/migrations/cursor-migration-manifest.json",
    ))
    .unwrap();
    assert_eq!(manifest["migrated"].as_array().unwrap().len(), 7);
    assert_eq!(
        manifest["ambiguous"][0]["destination"],
        ".agents/memory/architecture/plain.md"
    );
    assert_eq!(manifest["commands"].as_array().unwrap().len(), 0);
    assert_eq!(manifest["migration_marker"], MARKER);
    assert_eq!(manifest["migrated"][0]["kind"], "agent");

    // Migrator-owned files are rewritten on rerun; hand-written ones are
    // skipped until --force.
    let rerun = run(
        &repo.root,
        &["migrate", "cursor", "--source", &source, "--no-sync"],
    );
    rerun.assert_status(0);
    assert!(rerun.stdout.contains("writes: 9\nskipped: 0\n"));
    write(&repo.root, ".agents/agents/helper.md", "# Mine\n");
    let skipped = run(
        &repo.root,
        &["migrate", "cursor", "--source", &source, "--no-sync"],
    );
    skipped.assert_status(0);
    assert_eq!(
        skipped.stdout,
        "cursor/claude migration migrated\nwrites: 8\nskipped: 1\nambiguous: 1\n"
    );
    assert_eq!(read(&repo.root, ".agents/agents/helper.md"), "# Mine\n");
    assert!(
        read(&repo.root, ".agents/migrations/cursor-migration-report.md").contains(
            "destination exists and is not migrator-owned; rerun with --force to overwrite"
        )
    );
    let forced = run(
        &repo.root,
        &[
            "migrate",
            "cursor",
            "--source",
            &source,
            "--no-sync",
            "--force",
        ],
    );
    forced.assert_status(0);
    assert!(forced.stdout.contains("writes: 9\nskipped: 0\n"));
    assert!(read(&repo.root, ".agents/agents/helper.md").contains(MARKER));
}

#[test]
fn cursor_migration_reports_sources_that_collide_on_one_destination() {
    let repo = empty_repo();
    let source = tempfile::tempdir().unwrap();
    let source_root = source.path().canonicalize().unwrap();
    write(
        &source_root,
        ".cursor/commands/review.md",
        "Review from Cursor.\n",
    );
    write(
        &source_root,
        ".claude/commands/review.md",
        "Review from Claude.\n",
    );
    let migrated = run(
        &repo.root,
        &[
            "migrate",
            "cursor",
            "--source",
            source_root.to_str().unwrap(),
            "--no-sync",
        ],
    );
    migrated.assert_status(0);
    // The two skill files (SKILL.md and openai.yaml) plus the two reports
    // are written once; the second source is skipped for both of its files.
    assert_eq!(
        migrated.stdout,
        "cursor/claude migration migrated\nwrites: 4\nskipped: 2\nambiguous: 0\n"
    );
    let skill = read(&repo.root, ".agents/skills/review/SKILL.md");
    assert!(skill.contains("<!-- source: .cursor/commands/review.md -->"));
    assert!(skill.contains("Review from Cursor."));
    assert!(!skill.contains("Review from Claude."));
    let report = read(&repo.root, ".agents/migrations/cursor-migration-report.md");
    assert!(report.contains("- Skipped: `2`\n"));
    assert!(report.contains(
        "from `.claude/commands/review.md`: conflicts with `.cursor/commands/review.md`, which already maps to this destination in this run; rename one source to migrate both\n"
    ));
    let manifest: serde_json::Value = serde_json::from_str(&read(
        &repo.root,
        ".agents/migrations/cursor-migration-manifest.json",
    ))
    .unwrap();
    let skipped = manifest["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 2);
    assert_eq!(skipped[0]["source"], ".claude/commands/review.md");
    assert!(skipped[0]["destination"]
        .as_str()
        .unwrap()
        .ends_with("SKILL.md"));
    assert!(skipped[0]["reason"]
        .as_str()
        .unwrap()
        .starts_with("conflicts with `.cursor/commands/review.md`"));
}

#[test]
fn cursor_migration_without_skills_skips_skill_sync() {
    let repo = empty_repo();
    let source = tempfile::tempdir().unwrap();
    let source_root = source.path().canonicalize().unwrap();
    write(
        &source_root,
        ".cursor/rules/decide.mdc",
        "We decided this after a decision.\n",
    );
    let migrated = run(
        &repo.root,
        &[
            "migrate",
            "cursor",
            "--source",
            source_root.to_str().unwrap(),
        ],
    );
    migrated.assert_status(0);
    assert_eq!(
        migrated.stdout,
        "cursor/claude migration migrated\nwrites: 3\nskipped: 0\nambiguous: 0\n"
    );
    let manifest: serde_json::Value = serde_json::from_str(&read(
        &repo.root,
        ".agents/migrations/cursor-migration-manifest.json",
    ))
    .unwrap();
    let commands = manifest["commands"].as_array().unwrap();
    assert_eq!(commands.len(), 2);
    assert_eq!(
        commands[0]["argv"],
        serde_json::json!(["gritt-agent", "ticket", "sync"])
    );
}

#[cfg(unix)]
#[test]
fn cursor_migration_reads_a_file_once_through_overlapping_source_roots() {
    let repo = empty_repo();
    let source = tempfile::tempdir().unwrap();
    let source_root = source.path().canonicalize().unwrap();
    write(&source_root, ".cursor/agents/helper.md", "# Helper\n");
    std::os::unix::fs::symlink("agents", source_root.join(".cursor/agent")).unwrap();
    let migrated = run(
        &repo.root,
        &[
            "migrate",
            "cursor",
            "--source",
            source_root.to_str().unwrap(),
            "--no-sync",
        ],
    );
    migrated.assert_status(0);
    assert_eq!(
        migrated.stdout,
        "cursor/claude migration migrated\nwrites: 3\nskipped: 0\nambiguous: 0\n"
    );
}

#[test]
fn cursor_migration_runs_maintenance_and_rejects_bad_sources() {
    let repo = empty_repo();
    let source = source();
    let synced = run(&repo.root, &["migrate", "cursor", "--source", &source]);
    synced.assert_status(0);
    // Maintenance output is captured into the manifest, not echoed.
    assert_eq!(
        synced.stdout,
        "cursor/claude migration migrated\nwrites: 9\nskipped: 0\nambiguous: 1\n"
    );
    let manifest: serde_json::Value = serde_json::from_str(&read(
        &repo.root,
        ".agents/migrations/cursor-migration-manifest.json",
    ))
    .unwrap();
    let commands = manifest["commands"].as_array().unwrap();
    assert_eq!(commands.len(), 3);
    assert_eq!(
        commands[0]["argv"],
        serde_json::json!(["gritt-agent", "skill", "sync"])
    );
    assert_eq!(commands[0]["returncode"], 0);
    assert!(commands[0]["stdout"]
        .as_str()
        .unwrap()
        .starts_with("synced skill adapters ("));
    assert_eq!(commands[2]["stdout"], "tkt_validate ok (0 warnings)");
    assert_eq!(commands[2]["stderr"], "");
    let report = read(&repo.root, ".agents/migrations/cursor-migration-report.md");
    assert!(report.contains("- Planned writes: `9`\n"));
    assert!(report.ends_with(
        "## Maintenance Commands\n\n- `gritt-agent skill sync` -> `0`\n- `gritt-agent ticket sync` -> `0`\n- `gritt-agent ticket validate` -> `0`\n"
    ));
    assert!(repo.root.join(".claude/skills/example/SKILL.md").exists());
    assert!(repo
        .root
        .join(".agents/memory/decisions/index.yaml")
        .exists());
    // The sync keeps the migration marker and the migrated policy.
    let openai = read(&repo.root, ".agents/skills/review/agents/openai.yaml");
    assert!(openai.starts_with(&format!(
        "# {MARKER}\n# source: .claude/commands/review.md\n"
    )));
    assert!(openai.ends_with("policy:\n  allow_implicit_invocation: false\n"));
    let rerun = run(&repo.root, &["migrate", "cursor", "--source", &source]);
    rerun.assert_status(0);
    assert!(rerun.stdout.contains("writes: 9\nskipped: 0\n"));
    run(&repo.root, &["skill", "sync", "--check"]).assert_status(0);

    let same = run(
        &repo.root,
        &["migrate", "cursor", "--source", repo.root.to_str().unwrap()],
    );
    same.assert_status(1);
    assert!(same
        .stderr
        .contains("source and target repo must be different paths"));
    let missing = run(
        &repo.root,
        &[
            "migrate",
            "cursor",
            "--source",
            "/definitely/missing/source",
        ],
    );
    missing.assert_status(1);
    assert!(missing.stderr.contains("source repo does not exist"));
}
