mod common;

use std::fs;

use common::{expected, fixture, run, write};

#[test]
fn index_search_and_citations_match_expected_output() {
    let repo = fixture();
    let index = run(&repo.root, &["memory", "index"]);
    index.assert_status(0);
    assert_eq!(
        index.stderr,
        format!(
            "Indexed 15 local knowledge files into {}\n",
            repo.root
                .join(".agents/brain/data/agent-memory.db")
                .display()
        )
    );
    assert!(repo
        .root
        .join(".agents/brain/data/agent-memory.db")
        .exists());

    let search = run(
        &repo.root,
        &["memory", "search", "catalog cache", "--limit", "5"],
    );
    search.assert_status(0);
    assert_eq!(search.stdout, expected("search-catalog-cache.txt"));

    let none = run(&repo.root, &["memory", "search", "zzznothing"]);
    none.assert_status(0);
    assert_eq!(none.stdout, "No local knowledge matched the query.\n");
}

#[test]
fn index_skips_excluded_directories_and_unsupported_files() {
    let repo = fixture();
    write(
        &repo.root,
        "node_modules/pkg/README.md",
        "# catalogunique inside node_modules\n",
    );
    write(
        &repo.root,
        "target/debug/.fingerprint/x/dep-lib.json",
        "{\"catalogunique\": true}\n",
    );
    write(&repo.root, "dist/out.md", "# catalogunique dist\n");
    write(
        &repo.root,
        ".agents/brain/data/scratch.md",
        "# catalogunique brain data\n",
    );
    write(
        &repo.root,
        "docs/deep/nested/page.mdx",
        "# Nested catalogunique page\n",
    );
    run(&repo.root, &["memory", "index"]).assert_status(0);
    let search = run(&repo.root, &["memory", "search", "catalogunique"]);
    search.assert_status(0);
    assert!(search
        .stdout
        .contains("Source: docs/deep/nested/page.mdx:1-2"));
    assert!(!search.stdout.contains("node_modules"));
    assert!(!search.stdout.contains("target/"));
    assert!(!search.stdout.contains("dist/"));
    assert!(!search.stdout.contains("brain/data"));
}

#[test]
fn reindex_updates_changed_documents_and_removes_deleted_ones() {
    let repo = fixture();
    run(&repo.root, &["memory", "index"]).assert_status(0);
    let before = run(&repo.root, &["memory", "search", "sessions"]);
    assert!(before.stdout.contains("Source: docs/guide.md:14-17"));

    write(
        &repo.root,
        "docs/guide.md",
        "# Guide\n\nReplaced body about pancakes.\n",
    );
    fs::remove_file(repo.root.join("docs/settings.yaml")).unwrap();
    let index = run(&repo.root, &["memory", "index"]);
    index.assert_status(0);
    assert!(index.stderr.contains("Indexed 14 local knowledge files"));

    let after = run(&repo.root, &["memory", "search", "sessions"]);
    assert!(!after.stdout.contains("docs/guide.md"));
    let pancakes = run(&repo.root, &["memory", "search", "pancakes"]);
    assert!(pancakes.stdout.contains("Source: docs/guide.md:1-4"));
    let policy = run(&repo.root, &["memory", "search", "tool_policy"]);
    assert_eq!(policy.stdout, "No local knowledge matched the query.\n");
}

#[cfg(unix)]
#[test]
fn index_does_not_follow_symlinked_directories() {
    let repo = fixture();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("secret.md"), "# outsidesecret\n").unwrap();
    std::os::unix::fs::symlink(outside.path(), repo.root.join("linked")).unwrap();
    std::os::unix::fs::symlink(&repo.root, repo.root.join("docs/loop")).unwrap();
    let index = run(&repo.root, &["memory", "index"]);
    index.assert_status(0);
    assert!(index.stderr.contains("Indexed 15 local knowledge files"));
    let search = run(&repo.root, &["memory", "search", "outsidesecret"]);
    assert_eq!(search.stdout, "No local knowledge matched the query.\n");
}

#[cfg(unix)]
#[test]
fn index_does_not_follow_symlinked_files() {
    let repo = fixture();
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("secret.md");
    fs::write(&secret, "# outsidefilesecret\n").unwrap();
    std::os::unix::fs::symlink(&secret, repo.root.join("outside.md")).unwrap();

    let index = run(&repo.root, &["memory", "index"]);
    index.assert_status(0);
    assert!(index.stderr.contains("Indexed 15 local knowledge files"));
    let search = run(&repo.root, &["memory", "search", "outsidefilesecret"]);
    assert_eq!(search.stdout, "No local knowledge matched the query.\n");
}
