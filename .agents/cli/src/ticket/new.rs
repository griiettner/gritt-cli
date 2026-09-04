//! `gritt-agent ticket new`: allocates the next ticket id and scaffolds it.

use std::fs;
use std::path::{Path, PathBuf};

use super::identity::{resolve_ticket_identity, ResolveOptions};
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
    pub create_concept: bool,
    pub create_plan: bool,
    pub no_sync: bool,
    pub dry_run: bool,
}

struct Common<'a> {
    ticket_id: &'a str,
    namespace: &'a str,
    title: &'a str,
    owner: &'a str,
    created: &'a str,
    updated: &'a str,
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
            persist: true,
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
    };
    fsx::write_text(&dir.join("task.md"), &render_task(&common))?;
    if options.create_concept {
        fsx::write_text(&dir.join("concept.md"), &render_concept(&common))?;
    }
    if options.create_plan {
        fsx::write_text(&dir.join("plan.md"), &render_plan(&common))?;
    }
    if !options.no_sync {
        let status = match sync::run(repo_root, false) {
            Ok(status) => status,
            Err(error) => {
                eprintln!("error: {error}");
                1
            }
        };
        if status != 0 {
            fsx::remove_dir_all(&dir)?;
            eprintln!(
                "ticket creation rolled back because index sync failed for {ticket_id}; no ticket number was consumed"
            );
            return Ok(status);
        }
    }
    print_ticket(repo_root, namespace, &ticket_id, &dir, &created_files);
    Ok(0)
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

/// Quotes a scalar the frontmatter parser would otherwise reject as a
/// structured value, for example a title that starts with `[` or `{`.
fn yaml_scalar(value: &str) -> String {
    if value.starts_with(['[', '{']) {
        format!("\"{value}\"")
    } else {
        value.to_owned()
    }
}

fn frontmatter(values: &Common<'_>, artifact: &str, status: &str) -> String {
    format!(
        "---\nid: {}\nnamespace: {}\ntitle: {}\nartifact: {}\nstatus: {}\nowner: {}\ncreated: {}\nupdated: {}\n---\n\n",
        values.ticket_id,
        values.namespace,
        yaml_scalar(values.title),
        artifact,
        status,
        values.owner,
        values.created,
        values.updated
    )
}

fn render_task(values: &Common<'_>) -> String {
    let mut text = frontmatter(values, "task", "ready");
    text.push_str(&format!("# {} Task: {}\n", values.ticket_id, values.title));
    text.push_str(
        "\n## Goal\n\nDefine the concrete execution goal here.\n\n## Inputs\n\n- Add the required references here.\n\n## Scope\n\n- Define the exact work this ticket may change.\n\n## Out of Scope\n\n- Define what this ticket must not change.\n\n## Acceptance Criteria\n\n- Define concrete acceptance criteria.\n\n## Verification\n\n- Define the checks that prove the work is done.\n",
    );
    text
}

fn render_concept(values: &Common<'_>) -> String {
    let mut text = frontmatter(values, "concept", "concept");
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
    let mut text = frontmatter(values, "plan", "planning");
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
        };
        let parsed = parse_document("task.md", &render_task(&common));
        assert!(parsed.errors.is_empty());
        assert_eq!(parsed.metadata.scalar("title"), Some("[Spike] eval: thing"));
        assert_eq!(yaml_scalar("Plain title"), "Plain title");
    }
}
