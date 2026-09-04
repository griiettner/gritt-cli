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
        "TKT-0003\nnamespace: alice\nqualified: alice/TKT-0003\n.agents/tasks/alice/TKT-0001-0025/TKT-0003\n.agents/tasks/alice/TKT-0001-0025/TKT-0003/task.md\nsynced .agents task and memory indexes\n"
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
fn new_writes_areas_skills_and_dependencies() {
    let repo = fixture();
    let created = run(
        &repo.root,
        &[
            "ticket",
            "new",
            "--title",
            "Listed",
            "--namespace",
            "alice",
            "--areas",
            ".agents/cli",
            ".agents/skills",
            "--skills",
            "dev",
            "--dependencies",
            "TKT-0001",
            "--create-plan",
        ],
    );
    created.assert_status(0);
    let task = read(
        &repo.root,
        ".agents/tasks/alice/TKT-0001-0025/TKT-0003/task.md",
    );
    assert!(task.starts_with(
        "---\nid: TKT-0003\nnamespace: alice\ntitle: Listed\nartifact: task\nstatus: ready\nowner: alice\n"
    ));
    assert!(task.contains(
        "dependencies:\n  - TKT-0001\nareas:\n  - .agents/cli\n  - .agents/skills\nskills:\n  - dev\n---\n\n# TKT-0003 Task: Listed\n"
    ));
    let plan = read(
        &repo.root,
        ".agents/tasks/alice/TKT-0001-0025/TKT-0003/plan.md",
    );
    assert!(plan.contains("areas:\n  - .agents/cli\n  - .agents/skills\nskills:\n  - dev\n---\n"));
    assert!(!plan.contains("dependencies:"));
    let shard = read(&repo.root, ".agents/tasks/alice/TKT-0001-0025/index.yaml");
    assert!(shard.contains("    dependencies:\n      - TKT-0001\n"));
    assert!(shard.contains("      - .agents/cli\n"));
    run(&repo.root, &["ticket", "validate"]).assert_status(0);

    // Passing a list flag with no values clears it, as new-chain does.
    let cleared = run(
        &repo.root,
        &[
            "ticket",
            "new",
            "--title",
            "Cleared",
            "--areas",
            "--no-sync",
        ],
    );
    cleared.assert_status(0);
    assert!(!read(
        &repo.root,
        ".agents/tasks/alice/TKT-0001-0025/TKT-0004/task.md"
    )
    .contains("areas"));
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

fn expected_dated(name: &str) -> String {
    expected(name).replace("{{TODAY}}", &gritt_agent::repo::local_date())
}

fn env_run(root: &std::path::Path, namespace: &str, args: &[&str]) -> common::Run {
    let output = common::command(root)
        .env("GRITT_TKT_NAMESPACE", namespace)
        .args(args)
        .output()
        .unwrap();
    common::Run {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn git(root: &std::path::Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args([
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn identity_prints_and_stores_the_login() {
    let repo = fixture();
    let flag = run(&repo.root, &["ticket", "identity", "--namespace", "alice"]);
    flag.assert_status(0);
    assert_eq!(
        flag.stdout,
        "alice\nsource: flag\nstored: .agents/state/identity.local.yaml\n"
    );
    assert!(read(&repo.root, ".agents/state/identity.local.yaml")
        .starts_with("github_login: alice\nsource: flag\nresolved_at: "));

    let stored = run(&repo.root, &["ticket", "identity", "--no-persist"]);
    stored.assert_status(0);
    assert_eq!(stored.stdout, "alice\nsource: flag\n");

    let env = env_run(&repo.root, "carol", &["ticket", "identity", "--no-persist"]);
    env.assert_status(0);
    assert_eq!(env.stdout, "carol\nsource: env\n");
    assert!(read(&repo.root, ".agents/state/identity.local.yaml").contains("github_login: alice\n"));

    let bad = run(
        &repo.root,
        &["ticket", "identity", "--namespace", "_shared"],
    );
    bad.assert_status(2);
    assert!(bad.stderr.contains("invalid --namespace"));
}

#[test]
fn new_chain_scaffolds_orchestrator_workers_and_reviewer() {
    let repo = fixture();
    let args = [
        "ticket",
        "new-chain",
        "--title",
        "Sample chain",
        "--namespace",
        "alice",
        "--step",
        "one:First step",
        "--step",
        "two:Second step",
        "--create-concept",
        "--create-plan",
    ];
    let dry = run(&repo.root, &[&args[..], &["--dry-run"]].concat());
    dry.assert_status(0);
    assert_eq!(
        dry.stdout,
        "TKT-0003\nnamespace: alice\nqualified: alice/TKT-0003\n.agents/tasks/alice/TKT-0001-0025/TKT-0003\n.agents/tasks/alice/TKT-0001-0025/TKT-0003/task.md\n.agents/tasks/alice/TKT-0001-0025/TKT-0003/concept.md\n.agents/tasks/alice/TKT-0001-0025/TKT-0003/plan.md\nworker 1/2: TKT-0004 .agents/tasks/alice/TKT-0001-0025/TKT-0004/task.md\nworker 2/2: TKT-0005 .agents/tasks/alice/TKT-0001-0025/TKT-0005/task.md\nreviewer: TKT-0006 .agents/tasks/alice/TKT-0001-0025/TKT-0006/task.md\nchain tickets: 4\nTODO(tkt): every scaffolded section must be replaced before execution; `gritt-agent ticket validate` fails while any remains\nwould run: gritt-agent ticket sync\n"
    );
    assert!(!repo
        .root
        .join(".agents/tasks/alice/TKT-0001-0025/TKT-0003")
        .exists());

    let created = run(&repo.root, &args);
    created.assert_status(0);
    assert!(created.stdout.starts_with("TKT-0003\nnamespace: alice\n"));
    assert!(created
        .stdout
        .ends_with("chain tickets: 4\nTODO(tkt): every scaffolded section must be replaced before execution; `gritt-agent ticket validate` fails while any remains\nsynced .agents task and memory indexes\n"));
    let files = [
        ("TKT-0003/task.md", "chain-orchestrator-task.md"),
        ("TKT-0003/concept.md", "chain-orchestrator-concept.md"),
        ("TKT-0003/plan.md", "chain-orchestrator-plan.md"),
        ("TKT-0004/task.md", "chain-worker-1-task.md"),
        ("TKT-0005/task.md", "chain-worker-2-task.md"),
        ("TKT-0006/task.md", "chain-reviewer-task.md"),
    ];
    for (relative, name) in files {
        let path = format!(".agents/tasks/alice/TKT-0001-0025/{relative}");
        assert_eq!(read(&repo.root, &path), expected_dated(name), "{relative}");
    }
    assert!(
        read(&repo.root, ".agents/tasks/alice/TKT-0001-0025/index.yaml").contains("id: TKT-0006")
    );

    // The scaffold markers block validation until they are replaced; the
    // chain links themselves are valid.
    let scaffold = run(&repo.root, &["ticket", "validate"]);
    scaffold.assert_status(1);
    assert!(scaffold.stderr.contains("unfilled scaffold line(s)"));
    for (relative, _) in files {
        let path = format!(".agents/tasks/alice/TKT-0001-0025/{relative}");
        let filled = read(&repo.root, &path).replace("TODO(tkt):", "Filled:");
        write(&repo.root, &path, &filled);
    }
    let validate = run(&repo.root, &["ticket", "validate"]);
    validate.assert_status(0);
    assert_eq!(validate.stdout, "tkt_validate ok (0 warnings)\n");
}

#[test]
fn new_chain_guards_steps_and_honours_no_reviewer() {
    let repo = fixture();
    let none = run(
        &repo.root,
        &[
            "ticket",
            "new-chain",
            "--title",
            "Lonely",
            "--namespace",
            "alice",
            "--no-sync",
        ],
    );
    none.assert_status(2);
    assert!(none.stderr.contains("at least two --step values"));
    let one = run(
        &repo.root,
        &[
            "ticket",
            "new-chain",
            "--title",
            "Lonely",
            "--namespace",
            "alice",
            "--no-sync",
            "--step",
            "only:Only step",
        ],
    );
    one.assert_status(2);
    assert!(one.stderr.contains("at least two --step values"));
    assert!(!repo
        .root
        .join(".agents/tasks/alice/TKT-0001-0025/TKT-0003")
        .exists());

    let chain = run(
        &repo.root,
        &[
            "ticket",
            "new-chain",
            "--title",
            "Two steps",
            "--namespace",
            "alice",
            "--owner",
            "robert",
            "--step",
            "a:A",
            "--step",
            "b:B",
            "--no-reviewer",
            "--no-sync",
            "--dependencies",
            "TKT-0001",
            "TKT-0002",
            "--skills",
            "dev",
            "--areas",
            "crates/gritt",
        ],
    );
    chain.assert_status(0);
    assert!(chain.stdout.contains("chain tickets: 3\n"));
    assert!(!chain.stdout.contains("reviewer:"));
    let orchestrator = read(
        &repo.root,
        ".agents/tasks/alice/TKT-0001-0025/TKT-0003/task.md",
    );
    assert!(orchestrator.contains("owner: robert\n"));
    assert!(orchestrator.contains(
        "chain_children:\n  - TKT-0004\n  - TKT-0005\ndependencies:\n  - TKT-0001\n  - TKT-0002\nareas:\n  - crates/gritt\nskills:\n  - dev\n---\n"
    ));
    assert!(orchestrator
        .contains("1. [TKT-0004 A](../TKT-0004/task.md)\n2. [TKT-0005 B](../TKT-0005/task.md)\n"));
    assert!(!repo
        .root
        .join(".agents/tasks/alice/TKT-0001-0025/TKT-0006")
        .exists());
}

#[test]
fn new_chain_rolls_back_when_sync_fails() {
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
            "new-chain",
            "--title",
            "Rollback",
            "--namespace",
            "dave",
            "--step",
            "a:A",
            "--step",
            "b:B",
        ],
    );
    result.assert_status(1);
    assert!(result
        .stderr
        .contains("chain creation rolled back because index sync failed for TKT-0001"));
    for id in ["TKT-0001", "TKT-0002", "TKT-0003", "TKT-0004"] {
        assert!(!repo
            .root
            .join(".agents/tasks/dave/TKT-0001-0025")
            .join(id)
            .exists());
    }
}

#[test]
fn chain_check_reports_branch_state_and_ticket_artifacts() {
    let repo = fixture();
    git(&repo.root, &["init", "-q"]);
    git(&repo.root, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(&repo.root, &["add", "-A"]);
    git(&repo.root, &["commit", "-q", "-m", "init"]);
    git(&repo.root, &["checkout", "-q", "-b", "tkt-0002-work"]);
    write(&repo.root, "src.txt", "x\n");
    let report = read(
        &repo.root,
        ".agents/tasks/alice/TKT-0001-0025/TKT-0001/report.md",
    );
    write(
        &repo.root,
        ".agents/tasks/alice/TKT-0001-0025/TKT-0001/report.md",
        &format!("{report}\n## Validation\n\nbenchmark: 10ms\n\n## Completion Gate\n"),
    );
    git(&repo.root, &["add", "-A"]);
    git(&repo.root, &["commit", "-q", "-m", "work"]);

    let worker = env_run(
        &repo.root,
        "alice",
        &["ticket", "chain-check", "--ticket", "TKT-0002"],
    );
    worker.assert_status(0);
    assert!(worker
        .stdout
        .contains("NOTE: current branch: tkt-0002-work\n"));
    assert!(worker
        .stdout
        .contains("NOTE: changed files against `main`: 2\n"));
    assert!(worker.stdout.contains("WARN: missing report.md:"));
    assert!(worker.stdout.contains(
        "WARN: changed files include other ticket folders:\nWARN:   - .agents/tasks/alice/TKT-0001-0025/TKT-0001/report.md\n"
    ));
    assert!(worker
        .stdout
        .ends_with("tkt_chain_check ok (3 warning(s))\n"));
    assert_eq!(worker.stderr, "");

    let required = env_run(
        &repo.root,
        "alice",
        &[
            "ticket",
            "chain-check",
            "--ticket",
            "alice/TKT-0002",
            "--require-report",
        ],
    );
    required.assert_status(1);
    assert!(required.stderr.contains("ERROR: missing report.md:"));
    assert!(required
        .stderr
        .contains("tkt_chain_check failed (1 error(s), 2 warning(s))"));

    let benchmark = env_run(
        &repo.root,
        "alice",
        &[
            "ticket",
            "chain-check",
            "--ticket",
            "TKT-0001",
            "--require-benchmark",
        ],
    );
    benchmark.assert_status(0);
    assert!(benchmark
        .stdout
        .ends_with("tkt_chain_check ok (0 warning(s))\n"));

    git(&repo.root, &["checkout", "-q", "main"]);
    let base = env_run(
        &repo.root,
        "alice",
        &["ticket", "chain-check", "--ticket", "TKT-0001"],
    );
    base.assert_status(0);
    assert!(base
        .stdout
        .contains("WARN: current branch is the base branch `main`"));
    assert!(base
        .stdout
        .contains("WARN: no changed files detected against base branch"));
    assert!(base
        .stdout
        .contains("WARN: report.md missing section `## Completion Gate`"));
}

#[test]
fn chain_check_rejects_bad_ids_and_needs_a_git_repository() {
    let repo = fixture();
    let bad = run(&repo.root, &["ticket", "chain-check", "--ticket", "nope"]);
    bad.assert_status(2);
    assert!(bad.stderr.contains("invalid ticket id: nope"));

    let ambiguous = env_run(
        &repo.root,
        "zed",
        &["ticket", "chain-check", "--ticket", "TKT-0001"],
    );
    ambiguous.assert_status(2);
    assert!(ambiguous
        .stderr
        .contains("ambiguous ticket id TKT-0001; use one of: _shared/TKT-0001, alice/TKT-0001"));

    let missing = env_run(
        &repo.root,
        "zed",
        &["ticket", "chain-check", "--ticket", "alice/TKT-0009"],
    );
    missing.assert_status(2);
    assert!(missing
        .stderr
        .contains("ticket folder does not exist: alice/TKT-0009"));

    let no_git = env_run(
        &repo.root,
        "alice",
        &["ticket", "chain-check", "--ticket", "TKT-0002"],
    );
    no_git.assert_status(1);
    assert!(no_git
        .stderr
        .contains("ERROR: git rev-parse --show-toplevel failed"));
    assert!(no_git
        .stderr
        .contains("tkt_chain_check failed (1 error(s), 1 warning(s))"));
}
