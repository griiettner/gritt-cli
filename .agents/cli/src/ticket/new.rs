//! `gritt-agent ticket new`: allocates the next ticket id and scaffolds it.

use std::fs;
use std::path::{Path, PathBuf};

use super::identity::{resolve_ticket_identity, ResolveOptions};
use super::scaffold::{render_frontmatter, Frontmatter};
use super::store::{next_ticket_number, pad_ticket_number, ticket_dir};
use super::sync;
use crate::fsx::{self, relative_posix};
use crate::repo::{local_date, tasks_root};
use crate::Result;

#[derive(Debug, Clone, Default)]
pub struct NewOptions {
    pub title: String,
    pub namespace: Option<String>,
    pub owner: Option<String>,
    pub areas: Vec<String>,
    pub skills: Vec<String>,
    pub dependencies: Vec<String>,
    pub create_concept: bool,
    pub create_plan: bool,
    pub no_sync: bool,
    pub dry_run: bool,
}

/// Values shared by every artifact of one ticket. `dependencies` is written
/// to `task.md` only; `areas` and `skills` go on every artifact, as
/// `ticket new-chain` does.
struct Common<'a> {
    ticket_id: &'a str,
    namespace: &'a str,
    title: &'a str,
    owner: &'a str,
    created: &'a str,
    updated: &'a str,
    areas: &'a [String],
    skills: &'a [String],
    dependencies: &'a [String],
}

/// Creates the ticket and runs the index sync. When the sync fails the new
/// folder is removed so no ticket number is consumed.
pub fn run(repo_root: &Path, options: &NewOptions) -> Result<i32> {
    let tasks = tasks_root(repo_root);
    if !fsx::is_dir(&tasks) {
        eprintln!("tasks root does not exist: {}", tasks.display());
        return Ok(1);
    }
    let identity = resolve_ticket_identity(
        repo_root,
        &ResolveOptions {
            namespace: options.namespace.clone(),
            refresh: false,
            persist: !options.dry_run,
        },
    )?;
    let namespace = identity.github_login.as_str();
    let number = next_ticket_number(&tasks, namespace)?;
    let ticket_id = format!("TKT-{}", pad_ticket_number(number));
    let owner = options
        .owner
        .clone()
        .unwrap_or_else(|| namespace.to_owned());
    let dir = ticket_dir(&tasks, namespace, &ticket_id);
    let mut created_files = vec![dir.join("task.md")];
    if options.create_concept {
        created_files.push(dir.join("concept.md"));
    }
    if options.create_plan {
        created_files.push(dir.join("plan.md"));
    }
    if options.dry_run {
        print_ticket(repo_root, namespace, &ticket_id, &dir, &created_files);
        if !options.no_sync {
            println!("would run: gritt-agent ticket sync");
        }
        return Ok(0);
    }

    fs::create_dir_all(&dir)?;
    let today = local_date();
    let common = Common {
        ticket_id: &ticket_id,
        namespace,
        title: &options.title,
        owner: &owner,
        created: &today,
        updated: &today,
        areas: &options.areas,
        skills: &options.skills,
        dependencies: &options.dependencies,
    };
    fsx::write_text(&dir.join("task.md"), &render_task(&common))?;
    if options.create_concept {
        fsx::write_text(&dir.join("concept.md"), &render_concept(&common))?;
    }
    if options.create_plan {
        fsx::write_text(&dir.join("plan.md"), &render_plan(&common))?;
    }
    let announce = || print_ticket(repo_root, namespace, &ticket_id, &dir, &created_files);
    if options.no_sync {
        announce();
        return Ok(0);
    }
    sync_or_rollback(repo_root, &[dir.as_path()], "ticket", &ticket_id, announce)
}

/// Runs the index sync after scaffolding. On success `announce` prints the
/// caller's own message, then the sync summary follows it. On failure every
/// directory in `dirs` is removed so no ticket number is consumed, and the
/// failing exit code is returned for the caller to propagate.
pub(super) fn sync_or_rollback(
    repo_root: &Path,
    dirs: &[&Path],
    label: &str,
    ticket_id: &str,
    announce: impl FnOnce(),
) -> Result<i32> {
    let status = match sync::sync(repo_root, false) {
        Ok(summary) if summary.exit_code() == 0 => {
            announce();
            summary.print();
            return Ok(0);
        }
        Ok(summary) => {
            summary.print();
            summary.exit_code()
        }
        Err(error) => {
            eprintln!("error: {error}");
            1
        }
    };
    remove_scaffold_dirs(repo_root, dirs)?;
    eprintln!(
        "{label} creation rolled back because index sync failed for {ticket_id}; no ticket number was consumed"
    );
    Ok(status)
}

/// Removes scaffolded ticket folders and any chunk or namespace folder left
/// empty by that, so a failed scaffold consumes no ticket number.
pub(super) fn remove_scaffold_dirs(repo_root: &Path, dirs: &[&Path]) -> Result<()> {
    let tasks = tasks_root(repo_root);
    for dir in dirs {
        fsx::remove_dir_all(dir)?;
        remove_empty_parents(dir, &tasks);
    }
    Ok(())
}

/// Removes the chunk and namespace folders a rolled-back scaffold created
/// when they are now empty, stopping at the tasks root.
fn remove_empty_parents(dir: &Path, stop: &Path) {
    let mut current = dir.parent();
    while let Some(parent) = current {
        if parent == stop || !parent.starts_with(stop) {
            break;
        }
        if fs::remove_dir(parent).is_err() {
            break;
        }
        current = parent.parent();
    }
}

fn print_ticket(repo_root: &Path, namespace: &str, ticket_id: &str, dir: &Path, files: &[PathBuf]) {
    println!("{ticket_id}");
    println!("namespace: {namespace}");
    println!("qualified: {namespace}/{ticket_id}");
    println!("{}", relative_posix(repo_root, dir));
    for file in files {
        println!("{}", relative_posix(repo_root, file));
    }
}

fn frontmatter(
    values: &Common<'_>,
    artifact: &str,
    status: &str,
    dependencies: &[String],
) -> String {
    render_frontmatter(&Frontmatter {
        ticket_id: values.ticket_id,
        namespace: values.namespace,
        title: values.title,
        artifact,
        status,
        owner: values.owner,
        created: values.created,
        updated: values.updated,
        chain_role: None,
        chain_parent: None,
        chain_children: &[],
        dependencies,
        areas: values.areas,
        skills: values.skills,
    })
}

fn render_task(values: &Common<'_>) -> String {
    let mut text = frontmatter(values, "task", "ready", values.dependencies);
    text.push_str(&format!("# {} Task: {}\n", values.ticket_id, values.title));
    text.push_str(
        "\n## Goal\n\nDefine the concrete execution goal here.\n\n## Inputs\n\n- Add the required references here.\n\n## Scope\n\n- Define the exact work this ticket may change.\n\n## Out of Scope\n\n- Define what this ticket must not change.\n\n## Acceptance Criteria\n\n- Define concrete acceptance criteria.\n\n## Verification\n\n- Define the checks that prove the work is done.\n",
    );
    text
}

fn render_concept(values: &Common<'_>) -> String {
    let mut text = frontmatter(values, "concept", "concept", &[]);
    text.push_str(&format!(
        "# {} Concept: {}\n",
        values.ticket_id, values.title
    ));
    text.push_str(
        "\n## Problem\n\nDescribe the user or product problem here.\n\n## Intent\n\nDescribe what the ticket is meant to achieve.\n\n## Success Criteria\n\n- Define what success looks like before execution starts.\n",
    );
    text
}

fn render_plan(values: &Common<'_>) -> String {
    let mut text = frontmatter(values, "plan", "planning", &[]);
    text.push_str(&format!("# {} Plan: {}\n", values.ticket_id, values.title));
    text.push_str(
        "\n## Sequence\n\n1. Lock remaining product or implementation decisions.\n2. Execute the scoped change.\n3. Verify against the ticket acceptance criteria.\n\n## Decisions To Lock Before Execution\n\n- Fill in any still-open process or implementation decisions here.\n",
    );
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontmatter::parse_document;

    #[test]
    fn structured_looking_titles_round_trip() {
        let common = Common {
            ticket_id: "TKT-0001",
            namespace: "alice",
            title: "[Spike] eval: thing",
            owner: "alice",
            created: "2026-09-03",
            updated: "2026-09-03",
            areas: &[],
            skills: &[],
            dependencies: &[],
        };
        let parsed = parse_document("task.md", &render_task(&common));
        assert!(parsed.errors.is_empty());
        assert_eq!(parsed.metadata.scalar("title"), Some("[Spike] eval: thing"));
    }

    #[test]
    fn lists_land_on_task_and_only_areas_and_skills_on_the_rest() {
        let areas = vec![".agents/cli".to_owned()];
        let skills = vec!["dev".to_owned()];
        let dependencies = vec!["TKT-0001".to_owned()];
        let common = Common {
            ticket_id: "TKT-0002",
            namespace: "alice",
            title: "Listed",
            owner: "alice",
            created: "2026-09-04",
            updated: "2026-09-04",
            areas: &areas,
            skills: &skills,
            dependencies: &dependencies,
        };
        let task = render_task(&common);
        assert!(task.contains(
            "updated: 2026-09-04\ndependencies:\n  - TKT-0001\nareas:\n  - .agents/cli\nskills:\n  - dev\n---\n\n# TKT-0002 Task: Listed\n"
        ));
        let plan = render_plan(&common);
        assert!(plan.contains("areas:\n  - .agents/cli\nskills:\n  - dev\n---\n"));
        assert!(!plan.contains("dependencies"));
        let parsed = parse_document("task.md", &task);
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        assert_eq!(
            parsed.metadata.list("dependencies"),
            Some(&dependencies[..])
        );
    }
}
