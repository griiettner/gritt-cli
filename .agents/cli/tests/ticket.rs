mod common;

use common::{empty_repo, expected, fixture, read, run, ticket_task, write};

#[test]
fn sync_reproduces_committed_indexes_and_reports_no_drift() {
    let repo = fixture();
    run(&repo.root, &["ticket", "sync"]).assert_status(0);
    assert_eq!(
        read(&repo.root, ".agents/tasks/index.yaml"),
        expected("tasks-index.yaml")
    );
    assert_eq!(
        read(&repo.root, ".agents/tasks/TKT-0001-0025/index.yaml"),
        expected("shared-shard.yaml")
    );
    assert_eq!(
        read(&repo.root, ".agents/tasks/alice/TKT-0001-0025/index.yaml"),
        expected("alice-shard.yaml")
    );
    assert_eq!(
        read(&repo.root, ".agents/memory/architecture/index.yaml"),
        expected("memory-architecture-index.yaml")
    );
    assert_eq!(
        read(&repo.root, ".agents/memory/decisions/index.yaml"),
        expected("memory-decisions-index.yaml")
    );
    let check = run(&repo.root, &["ticket", "sync", "--check"]);
    check.assert_status(0);
    assert_eq!(check.stdout, "tkt_sync ok (no drift)\n");
}

#[test]
fn sync_check_reports_drift_and_removes_stale_shards() {
    let repo = fixture();
    let check = run(&repo.root, &["ticket", "sync", "--check"]);
    check.assert_status(1);
    assert!(check.stderr.contains("drift: .agents/tasks/index.yaml"));
    assert!(check.stderr.contains("generated index file(s) out of sync"));
    assert!(!repo.root.join(".agents/tasks/index.yaml").exists());

    write(
        &repo.root,
        ".agents/tasks/alice/TKT-0026-0050/index.yaml",
        "tickets:\n",
    );
    let sync = run(&repo.root, &["ticket", "sync"]);
    sync.assert_status(0);
    assert!(!repo
        .root
        .join(".agents/tasks/alice/TKT-0026-0050/index.yaml")
        .exists());
}

#[test]
fn sync_fails_on_frontmatter_errors() {
    let repo = fixture();
    write(
        &repo.root,
        ".agents/tasks/alice/TKT-0001-0025/TKT-0003/task.md",
        "---\nid: TKT-0003\ntitle: Broken\n",
    );
    let sync = run(&repo.root, &["ticket", "sync"]);
    sync.assert_status(1);
    assert!(sync.stderr.contains("does not close"));
    assert!(sync
        .stderr
        .contains("tkt_sync failed (1 frontmatter error(s))"));
}

#[test]
fn validate_passes_on_the_fixture_and_matches_expected_output() {
    let repo = fixture();
    run(&repo.root, &["ticket", "sync"]).assert_status(0);
    let validate = run(&repo.root, &["ticket", "validate"]);
    validate.assert_status(0);
    assert_eq!(validate.stdout, expected("validate-ok.txt"));
    assert_eq!(validate.stderr, "");
}

#[test]
fn validate_reports_missing_router_and_omitted_tickets() {
    let repo = fixture();
    let validate = run(&repo.root, &["ticket", "validate"]);
    validate.assert_status(0);
    assert!(validate.stderr.contains("index.yaml is missing"));
    assert_eq!(validate.stdout, "tkt_validate ok (1 warning)\n");

    run(&repo.root, &["ticket", "sync"]).assert_status(0);
    write(
        &repo.root,
        ".agents/tasks/alice/TKT-0001-0025/TKT-0003/task.md",
        &ticket_task("TKT-0003", Some("alice"), ""),
    );
    let stale = run(&repo.root, &["ticket", "validate"]);
    stale.assert_status(0);
    assert!(stale
        .stderr
        .contains("optional shard indexes omit existing ticket folder: alice/TKT-0003"));
}

#[test]
fn validate_rejects_scaffold_markers_chunk_mismatch_and_childless_chains() {
    let repo = fixture();
    write(
        &repo.root,
        ".agents/tasks/alice/TKT-0001-0025/TKT-0003/task.md",
        &ticket_task("TKT-0003", Some("alice"), "chain_role: orchestrator\n"),
    );
    write(
        &repo.root,
        ".agents/tasks/alice/TKT-0001-0025/TKT-0030/task.md",
        &format!(
            "{}\nTODO(tkt): fill me in\n",
            ticket_task("TKT-0030", Some("alice"), "")
        ),
    );
    write(
        &repo.root,
        ".agents/tasks/alice/TKT-0001-0025/TKT-0004/task.md",
        "---\nid: TKT-0009\ntitle: Wrong id\nartifact: plan\nstatus: odd\n---\n",
    );
    let validate = run(&repo.root, &["ticket", "validate"]);
    validate.assert_status(1);
    let stderr = &validate.stderr;
    assert!(stderr.contains("alice/TKT-0003 is a chain orchestrator with no `chain_children`"));
    assert!(stderr.contains("alice/TKT-0030 is in TKT-0001-0025 but belongs in TKT-0026-0050"));
    assert!(stderr.contains("unfilled scaffold line(s) marked `TODO(tkt):`"));
    assert!(stderr.contains("id mismatch"));
    assert!(stderr.contains("artifact mismatch"));
    assert!(stderr.contains("missing `created`"));
    assert!(stderr.contains("warning: suspicious status value"));
    assert!(stderr.contains("tkt_validate failed ("));
}

#[test]
fn validate_checks_chain_links() {
    let repo = empty_repo();
    write(
        &repo.root,
        ".agents/tasks/bob/TKT-0001-0025/TKT-0001/task.md",
        &ticket_task(
            "TKT-0001",
            Some("bob"),
            "chain_role: orchestrator\nchain_children:\n  - TKT-0002\n  - TKT-0003\n",
        ),
    );
    write(
        &repo.root,
        ".agents/tasks/bob/TKT-0001-0025/TKT-0002/task.md",
        &ticket_task(
            "TKT-0002",
            Some("bob"),
            "chain_role: worker\nchain_parent: TKT-0001\n",
        ),
    );
    write(
        &repo.root,
        ".agents/tasks/bob/TKT-0001-0025/TKT-0003/task.md",
        &ticket_task("TKT-0003", Some("bob"), "chain_role: reviewer\n"),
    );
    let validate = run(&repo.root, &["ticket", "validate"]);
    validate.assert_status(1);
    assert!(validate
        .stderr
        .contains("bob/TKT-0003 has chain_role reviewer but no `chain_parent`"));
}

#[test]
fn new_allocates_per_namespace_and_syncs_indexes() {
    let repo = fixture();
    let first = run(
        &repo.root,
        &[
            "ticket",
            "new",
            "--title",
            "Alice follow-up",
            "--namespace",
            "alice",
        ],
    );
    first.assert_status(0);
    assert_eq!(
        first.stdout,
        "synced .agents task and memory indexes\nTKT-0003\nnamespace: alice\nqualified: alice/TKT-0003\n.agents/tasks/alice/TKT-0001-0025/TKT-0003\n.agents/tasks/alice/TKT-0001-0025/TKT-0003/task.md\n"
    );
    let task = read(
        &repo.root,
        ".agents/tasks/alice/TKT-0001-0025/TKT-0003/task.md",
    );
    assert!(task.starts_with("---\nid: TKT-0003\nnamespace: alice\ntitle: Alice follow-up\nartifact: task\nstatus: ready\nowner: alice\n"));
    assert!(task.contains("# TKT-0003 Task: Alice follow-up"));
    assert!(
        read(&repo.root, ".agents/tasks/alice/TKT-0001-0025/index.yaml").contains("id: TKT-0003")
    );
    assert!(read(&repo.root, ".agents/state/identity.local.yaml")
        .contains("github_login: alice\nsource: flag\n"));

    let bob = run(
        &repo.root,
        &[
            "ticket",
            "new",
            "--title",
            "Bob ticket",
            "--namespace",
            "bob",
            "--owner",
            "robert",
            "--create-concept",
            "--create-plan",
            "--no-sync",
        ],
    );
    bob.assert_status(0);
    assert!(bob.stdout.contains("qualified: bob/TKT-0001"));
    assert!(bob
        .stdout
        .contains(".agents/tasks/bob/TKT-0001-0025/TKT-0001/concept.md"));
    assert!(read(
        &repo.root,
        ".agents/tasks/bob/TKT-0001-0025/TKT-0001/plan.md"
    )
    .contains("owner: robert"));
    assert!(read(
        &repo.root,
        ".agents/tasks/bob/TKT-0001-0025/TKT-0001/concept.md"
    )
    .contains("status: concept"));

    // The stored identity is now bob, so a bare call allocates there.
    let stored = run(
        &repo.root,
        &["ticket", "new", "--title", "Bob again", "--no-sync"],
    );
    stored.assert_status(0);
    assert!(stored.stdout.contains("qualified: bob/TKT-0002"));

    run(&repo.root, &["ticket", "sync"]).assert_status(0);
    run(&repo.root, &["ticket", "validate"]).assert_status(0);
}

#[test]
fn new_honours_env_namespace_and_dry_run() {
    let repo = fixture();
    let output = common::command(&repo.root)
        .env("GRITT_TKT_NAMESPACE", "carol")
        .args(["ticket", "new", "--title", "Dry", "--dry-run"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("qualified: carol/TKT-0001"));
    assert!(stdout.contains("would run: gritt-agent ticket sync"));
    assert!(!repo.root.join(".agents/tasks/carol").exists());
}

#[test]
fn new_refuses_to_skip_a_missing_id() {
    let repo = fixture();
    write(
        &repo.root,
        ".agents/tasks/alice/TKT-0001-0025/TKT-0005/task.md",
        &ticket_task("TKT-0005", Some("alice"), ""),
    );
    let result = run(
        &repo.root,
        &[
            "ticket",
            "new",
            "--title",
            "Skip",
            "--namespace",
            "alice",
            "--no-sync",
        ],
    );
    result.assert_status(1);
    assert!(result
        .stderr
        .contains("missing ids in namespace alice: TKT-0003, TKT-0004"));
    assert!(!repo
        .root
        .join(".agents/tasks/alice/TKT-0001-0025/TKT-0006")
        .exists());
}

#[test]
fn new_rolls_back_when_sync_fails() {
    let repo = fixture();
    write(
        &repo.root,
        ".agents/tasks/broken/TKT-0001-0025/TKT-0001/task.md",
        "---\nid: TKT-0001\ntitle: never closes\n",
    );
    let result = run(
        &repo.root,
        &[
            "ticket",
            "new",
            "--title",
            "Rollback",
            "--namespace",
            "dave",
        ],
    );
    result.assert_status(1);
    assert!(result
        .stderr
        .contains("ticket creation rolled back because index sync failed for TKT-0001"));
    // A failed sync writes nothing, so no index advertises the rolled-back id.
    assert!(!repo.root.join(".agents/tasks/index.yaml").exists());
    assert!(!repo
        .root
        .join(".agents/tasks/dave/TKT-0001-0025/index.yaml")
        .exists());
    assert!(!repo
        .root
        .join(".agents/tasks/dave")
        .join("TKT-0001-0025")
        .join("TKT-0001")
        .exists());
}

#[test]
fn new_rejects_invalid_namespaces() {
    let repo = fixture();
    let result = run(
        &repo.root,
        &[
            "ticket",
            "new",
            "--title",
            "Bad",
            "--namespace",
            "_shared",
            "--no-sync",
        ],
    );
    result.assert_status(2);
    assert!(result.stderr.contains("invalid --namespace"));
}

#[test]
fn new_accepts_titles_that_look_like_yaml_structures() {
    let repo = fixture();
    let result = run(
        &repo.root,
        &[
            "ticket",
            "new",
            "--title",
            "[Spike] eval: thing",
            "--namespace",
            "alice",
        ],
    );
    result.assert_status(0);
    let task = read(
        &repo.root,
        ".agents/tasks/alice/TKT-0001-0025/TKT-0003/task.md",
    );
    assert!(task.contains("title: \"[Spike] eval: thing\""));
    assert!(
        read(&repo.root, ".agents/tasks/alice/TKT-0001-0025/index.yaml")
            .contains("title: [Spike] eval: thing")
    );
    run(&repo.root, &["ticket", "validate"]).assert_status(0);
}
