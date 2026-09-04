//! `gritt-agent ticket new-chain`: allocates consecutive ids for an
//! orchestrator, one worker per step, and a final reviewer, then scaffolds
//! every `task.md` with the chain fields `ticket validate` checks.

use std::fs;
use std::path::{Path, PathBuf};

use super::identity::{resolve_ticket_identity, ResolveOptions};
use super::new::sync_or_rollback;
use super::scaffold::{render_frontmatter, Frontmatter as TicketFrontmatter};
use super::store::{next_ticket_number, pad_ticket_number, ticket_dir};
use crate::fsx::{self, kebab_case, relative_path_posix, relative_posix};
use crate::repo::{local_date, tasks_root};
use crate::{CliError, Result};

pub const TODO: &str = "TODO(tkt):";
pub const DEFAULT_BASE_BRANCH: &str = "main";
pub const DEFAULT_BRANCH_PATTERN: &str = "tkt-{id}-{slug}";
pub const DEFAULT_MERGE_POLICY: &str = "Each worker opens a PR against main; reviewer runs after every PR; do not wait for CI/CD before merge when quota is unreliable.";
pub const DEFAULT_SKILLS: [&str; 2] = ["tkt", "tkt-exec-chain"];
pub const DEFAULT_AREAS: [&str; 2] = [".agents/tasks", ".agents/skills"];

#[derive(Debug, Clone)]
pub struct NewChainOptions {
    pub title: String,
    pub steps: Vec<String>,
    pub namespace: Option<String>,
    pub owner: Option<String>,
    pub base_branch: String,
    pub branch_pattern: String,
    pub merge_policy: String,
    pub reviewer_title: Option<String>,
    pub no_reviewer: bool,
    pub skills: Vec<String>,
    pub areas: Vec<String>,
    pub dependencies: Vec<String>,
    pub create_concept: bool,
    pub create_plan: bool,
    pub no_sync: bool,
    pub dry_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub number: usize,
    pub slug: String,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct ChainTicket {
    pub ticket_id: String,
    pub title: String,
    pub dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct Worker {
    pub ticket: ChainTicket,
    pub step: usize,
    pub slug: String,
    pub total: usize,
}

#[derive(Debug, Clone)]
pub struct Chain {
    pub orchestrator: ChainTicket,
    pub workers: Vec<Worker>,
    pub reviewer: Option<ChainTicket>,
}

/// Values shared by every rendered artifact of one chain.
pub struct Common<'a> {
    pub namespace: &'a str,
    pub owner: &'a str,
    pub created: &'a str,
    pub updated: &'a str,
    pub areas: &'a [String],
    pub skills: &'a [String],
    pub base_branch: &'a str,
    pub branch_pattern: &'a str,
    pub merge_policy: &'a str,
}

pub fn run(repo_root: &Path, options: &NewChainOptions) -> Result<i32> {
    let steps = parse_steps(&options.steps)?;
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
    let first = next_ticket_number(&tasks, namespace)?;
    let chain = build_chain(
        &tasks,
        namespace,
        first,
        &options.title,
        &steps,
        !options.no_reviewer,
        options.reviewer_title.as_deref(),
    );

    if options.dry_run {
        print_chain(repo_root, namespace, &chain, options);
        if !options.no_sync {
            println!("would run: gritt-agent ticket sync");
        }
        return Ok(0);
    }

    let today = local_date();
    let owner = options
        .owner
        .clone()
        .unwrap_or_else(|| namespace.to_owned());
    let common = Common {
        namespace,
        owner: &owner,
        created: &today,
        updated: &today,
        areas: &options.areas,
        skills: &options.skills,
        base_branch: &options.base_branch,
        branch_pattern: &options.branch_pattern,
        merge_policy: &options.merge_policy,
    };

    fs::create_dir_all(&chain.orchestrator.dir)?;
    fsx::write_text(
        &chain.orchestrator.dir.join("task.md"),
        &render_orchestrator_task(&common, &chain, &options.dependencies),
    )?;
    if options.create_concept {
        fsx::write_text(
            &chain.orchestrator.dir.join("concept.md"),
            &render_concept(&common, &chain),
        )?;
    }
    if options.create_plan {
        fsx::write_text(
            &chain.orchestrator.dir.join("plan.md"),
            &render_plan(&common, &chain),
        )?;
    }
    for (index, worker) in chain.workers.iter().enumerate() {
        fs::create_dir_all(&worker.ticket.dir)?;
        fsx::write_text(
            &worker.ticket.dir.join("task.md"),
            &render_worker_task(&common, &chain, index),
        )?;
    }
    if let Some(reviewer) = &chain.reviewer {
        fs::create_dir_all(&reviewer.dir)?;
        fsx::write_text(
            &reviewer.dir.join("task.md"),
            &render_reviewer_task(&common, &chain),
        )?;
    }

    let announce = || print_chain(repo_root, namespace, &chain, options);
    if options.no_sync {
        announce();
        return Ok(0);
    }
    let dirs: Vec<&Path> = std::iter::once(chain.orchestrator.dir.as_path())
        .chain(chain.workers.iter().map(|w| w.ticket.dir.as_path()))
        .chain(chain.reviewer.iter().map(|r| r.dir.as_path()))
        .collect();
    sync_or_rollback(
        repo_root,
        &dirs,
        "chain",
        &chain.orchestrator.ticket_id,
        announce,
    )
}

/// Parses `slug:title` values. A missing colon uses the whole text as the
/// title and derives the slug from it.
pub fn parse_steps(raw_steps: &[String]) -> Result<Vec<Step>> {
    if raw_steps.len() < 2 {
        return Err(CliError::usage(
            "a chain needs at least two --step values; use `gritt-agent ticket new` for a single one-shot ticket",
        ));
    }
    raw_steps
        .iter()
        .enumerate()
        .map(|(index, raw)| {
            let (slug, title) = match raw.split_once(':') {
                Some((slug, title)) => (slugify(slug), title.trim()),
                None => (slugify(raw), raw.trim()),
            };
            if title.is_empty() {
                return Err(CliError::usage(format!("--step {raw} has no title")));
            }
            Ok(Step {
                number: index + 1,
                slug,
                title: title.to_owned(),
            })
        })
        .collect()
}

pub fn build_chain(
    tasks: &Path,
    namespace: &str,
    first: u32,
    title: &str,
    steps: &[Step],
    with_reviewer: bool,
    reviewer_title: Option<&str>,
) -> Chain {
    let make = |offset: usize, title: String| {
        let ticket_id = format!("TKT-{}", pad_ticket_number(first + offset as u32));
        ChainTicket {
            dir: ticket_dir(tasks, namespace, &ticket_id),
            ticket_id,
            title,
        }
    };
    let orchestrator = make(0, title.to_owned());
    let workers = steps
        .iter()
        .enumerate()
        .map(|(index, step)| Worker {
            ticket: make(index + 1, step.title.clone()),
            step: step.number,
            slug: step.slug.clone(),
            total: steps.len(),
        })
        .collect();
    let reviewer = with_reviewer.then(|| {
        make(
            steps.len() + 1,
            reviewer_title
                .map(str::to_owned)
                .unwrap_or_else(|| format!("Review integrated {title} chain")),
        )
    });
    Chain {
        orchestrator,
        workers,
        reviewer,
    }
}

fn print_chain(repo_root: &Path, namespace: &str, chain: &Chain, options: &NewChainOptions) {
    let orchestrator = &chain.orchestrator;
    println!("{}", orchestrator.ticket_id);
    println!("namespace: {namespace}");
    println!("qualified: {namespace}/{}", orchestrator.ticket_id);
    println!("{}", relative_posix(repo_root, &orchestrator.dir));
    println!(
        "{}",
        relative_posix(repo_root, &orchestrator.dir.join("task.md"))
    );
    if options.create_concept {
        println!(
            "{}",
            relative_posix(repo_root, &orchestrator.dir.join("concept.md"))
        );
    }
    if options.create_plan {
        println!(
            "{}",
            relative_posix(repo_root, &orchestrator.dir.join("plan.md"))
        );
    }
    for worker in &chain.workers {
        println!(
            "worker {}/{}: {} {}",
            worker.step,
            worker.total,
            worker.ticket.ticket_id,
            relative_posix(repo_root, &worker.ticket.dir.join("task.md"))
        );
    }
    if let Some(reviewer) = &chain.reviewer {
        println!(
            "reviewer: {} {}",
            reviewer.ticket_id,
            relative_posix(repo_root, &reviewer.dir.join("task.md"))
        );
    }
    println!(
        "chain tickets: {}",
        1 + chain.workers.len() + usize::from(chain.reviewer.is_some())
    );
    println!(
        "{TODO} every scaffolded section must be replaced before execution; `gritt-agent ticket validate` fails while any remains"
    );
}

/// Kebab-case slug, `ticket` when nothing is left.
pub fn slugify(value: &str) -> String {
    let slug = kebab_case(value);
    if slug.is_empty() {
        "ticket".to_owned()
    } else {
        slug
    }
}

fn ticket_link(from_dir: &Path, target: &ChainTicket) -> String {
    format!(
        "[{} {}]({})",
        target.ticket_id,
        target.title,
        relative_path_posix(from_dir, &target.dir.join("task.md"))
    )
}

pub fn worker_branch(worker: &Worker) -> String {
    let number = worker.ticket.ticket_id.to_lowercase();
    let number = number.strip_prefix("tkt-").unwrap_or(&number);
    format!("tkt-{number}-{:02}-{}", worker.step, worker.slug)
}

struct Frontmatter<'a> {
    ticket_id: &'a str,
    title: &'a str,
    artifact: &'a str,
    status: &'a str,
    chain_role: Option<&'a str>,
    chain_parent: Option<&'a str>,
    chain_children: &'a [String],
    dependencies: &'a [String],
}

fn frontmatter(common: &Common<'_>, values: &Frontmatter<'_>) -> String {
    render_frontmatter(&TicketFrontmatter {
        ticket_id: values.ticket_id,
        namespace: common.namespace,
        title: values.title,
        artifact: values.artifact,
        status: values.status,
        owner: common.owner,
        created: common.created,
        updated: common.updated,
        chain_role: values.chain_role,
        chain_parent: values.chain_parent,
        chain_children: values.chain_children,
        dependencies: values.dependencies,
        areas: common.areas,
        skills: common.skills,
    })
}

pub fn render_orchestrator_task(
    common: &Common<'_>,
    chain: &Chain,
    dependencies: &[String],
) -> String {
    let orchestrator = &chain.orchestrator;
    let children: Vec<String> = chain
        .workers
        .iter()
        .map(|w| w.ticket.ticket_id.clone())
        .chain(chain.reviewer.iter().map(|r| r.ticket_id.clone()))
        .collect();
    let mut listing: Vec<String> = chain
        .workers
        .iter()
        .enumerate()
        .map(|(index, worker)| {
            format!(
                "{}. {}",
                index + 1,
                ticket_link(&orchestrator.dir, &worker.ticket)
            )
        })
        .collect();
    if let Some(reviewer) = &chain.reviewer {
        listing.push(format!(
            "{}. {} (final reviewer)",
            chain.workers.len() + 1,
            ticket_link(&orchestrator.dir, reviewer)
        ));
    }
    let first_child = children.first().map(String::as_str).unwrap_or("");
    let last_child = children.last().map(String::as_str).unwrap_or("");
    let mut text = frontmatter(
        common,
        &Frontmatter {
            ticket_id: &orchestrator.ticket_id,
            title: &orchestrator.title,
            artifact: "task",
            status: "planning",
            chain_role: Some("orchestrator"),
            chain_parent: None,
            chain_children: &children,
            dependencies,
        },
    );
    text.push_str(&format!(
        "# {} Task: {}\n\n## Goal\n\n{TODO} state the concrete outcome this chain delivers.\n\n## Chain Execution Contract\n\n- Execution mode: `tkt-exec-chain`\n- Base branch: `{}`\n- Branch naming pattern: `{}`\n- Worker branch pattern: `tkt-{{id}}-{{step}}-{{step-slug}}`\n- Merge policy: {}\n- Reviewer gate: reviewer runs after every worker PR\n- Child tickets: required and fixed as {first_child} through {last_child}\n- Validation required on every worker step: {TODO} name the checks\n- Benchmark requirements: {TODO} name them or state none\n- Final completion condition: {TODO} state it\n- Concurrency: exactly one active worker; no later step starts before the previous PR merges\n\n## Child Ticket Chain\n\n{}\n\nThe orchestrator activates exactly one worker ticket at a time. Every\nworker opens one PR and receives a reviewer verdict before merge. The next\nworker is activated only after that merge.\n\n## Inputs\n\n- {TODO} list the plans, ADRs, and package READMEs a worker must read.\n\n## Scope\n\n- {TODO} describe the work covered by the child chain.\n\n## Out of Scope\n\n- {TODO} describe what the chain must not change.\n\n## Acceptance Criteria\n\n- {TODO} give concrete, checkable criteria.\n\n## Verification\n\n- {TODO} name the checks every worker and reviewer pass must respect.\n- Run `gritt-agent ticket chain-check --ticket {} --base {}` before semantic review.\n",
        orchestrator.ticket_id,
        orchestrator.title,
        common.base_branch,
        common.branch_pattern,
        common.merge_policy,
        listing.join("\n"),
        orchestrator.ticket_id,
        common.base_branch,
    ));
    text
}

pub fn render_worker_task(common: &Common<'_>, chain: &Chain, index: usize) -> String {
    let worker = &chain.workers[index];
    let previous = index.checked_sub(1).map(|i| &chain.workers[i]);
    let dependencies: Vec<String> = previous
        .map(|p| vec![p.ticket.ticket_id.clone()])
        .unwrap_or_default();
    let start_line = match previous {
        Some(previous) => format!(
            "Start from a freshly updated `{}` only after {} merges and passes review.",
            common.base_branch, previous.ticket.ticket_id
        ),
        None => format!(
            "Start from a freshly updated `{}`. This is the first worker in the chain.",
            common.base_branch
        ),
    };
    let mut text = frontmatter(
        common,
        &Frontmatter {
            ticket_id: &worker.ticket.ticket_id,
            title: &worker.ticket.title,
            artifact: "task",
            status: if index == 0 { "ready" } else { "planning" },
            chain_role: Some("worker"),
            chain_parent: Some(&chain.orchestrator.ticket_id),
            chain_children: &[],
            dependencies: &dependencies,
        },
    );
    text.push_str(&format!(
        "# {} Task: {}\n\n## Chain Role\n\nWorker {} of {} in the {} chain.\n{start_line}\n\nBranch: `{}`\n\n## Goal\n\n{TODO} state what this single step delivers.\n\n## Scope\n\n- {TODO} keep this to the one step; anything else belongs to another worker.\n\n## Out of Scope\n\n- {TODO} name the neighbouring steps this worker must not touch.\n\n## Acceptance Criteria\n\n- {TODO} give concrete criteria the reviewer can check on the PR.\n\n## Verification\n\n- {TODO} name the commands and manual checks for this step.\n- Run `gritt-agent ticket chain-check --ticket {} --base {}` before semantic review.\n\n## Handoff\n\nReport branch name, PR link, validation output, and unresolved risks to the\nPM, then stop. Do not start the next step.\n",
        worker.ticket.ticket_id,
        worker.ticket.title,
        worker.step,
        worker.total,
        chain.orchestrator.ticket_id,
        worker_branch(worker),
        worker.ticket.ticket_id,
        common.base_branch,
    ));
    text
}

pub fn render_reviewer_task(common: &Common<'_>, chain: &Chain) -> String {
    let reviewer = chain
        .reviewer
        .as_ref()
        .expect("reviewer task rendered only when the chain has a reviewer");
    let dependencies: Vec<String> = chain
        .workers
        .iter()
        .map(|w| w.ticket.ticket_id.clone())
        .collect();
    let first_worker = &chain.workers[0].ticket.ticket_id;
    let last_worker = &chain.workers[chain.workers.len() - 1].ticket.ticket_id;
    let mut text = frontmatter(
        common,
        &Frontmatter {
            ticket_id: &reviewer.ticket_id,
            title: &reviewer.title,
            artifact: "task",
            status: "planning",
            chain_role: Some("reviewer"),
            chain_parent: Some(&chain.orchestrator.ticket_id),
            chain_children: &[],
            dependencies: &dependencies,
        },
    );
    text.push_str(&format!(
        "# {} Task: {}\n\n## Chain Role\n\nFinal reviewer ticket for the {} chain. Per-worker PR review stays\nmandatory throughout the chain. This ticket runs the integrated pass after\n{last_worker} and every earlier worker ticket have merged.\n\n## Goal\n\nIndependently determine whether the merged result satisfies the parent\ncontract without scope drift, integration gaps, regressions, or missing\nevidence.\n\n## Review Scope\n\n- Re-run deterministic ticket and chain validation.\n- Review the full diff across {first_worker} through {last_worker}.\n- Load `review/ticket` against {}'s task.md for completion readiness, and `review/impact` across the merged diff for integration conflicts.\n- {TODO} name the architecture and behavior checks specific to this chain.\n\n## Acceptance Criteria\n\n- Every parent and child acceptance criterion has evidence.\n- All worker PRs have recorded reviewer verdicts and required validation.\n- No unresolved high or medium finding blocks completion.\n- {} receives a completion report only after this reviewer returns `pass`.\n\n## Verification\n\n- Run `gritt-agent ticket validate`.\n- Run `gritt-agent ticket chain-check --ticket {} --base {}`.\n- Re-run the scoped command set recorded by the parent and worker tickets.\n- Produce a typed verdict: `pass`, `needs-fix`, or `blocked`, with findings\n  and next actions.\n",
        reviewer.ticket_id,
        reviewer.title,
        chain.orchestrator.ticket_id,
        chain.orchestrator.ticket_id,
        chain.orchestrator.ticket_id,
        reviewer.ticket_id,
        common.base_branch,
    ));
    text
}

pub fn render_concept(common: &Common<'_>, chain: &Chain) -> String {
    let orchestrator = &chain.orchestrator;
    let mut text = frontmatter(
        common,
        &Frontmatter {
            ticket_id: &orchestrator.ticket_id,
            title: &orchestrator.title,
            artifact: "concept",
            status: "concept",
            chain_role: Some("orchestrator"),
            chain_parent: None,
            chain_children: &[],
            dependencies: &[],
        },
    );
    text.push_str(&format!(
        "# {} Concept: {}\n\n## Problem\n\n{TODO} describe the user or product problem.\n\n## Intent\n\n{TODO} describe what the chain is meant to achieve.\n\n## Success Criteria\n\n- {TODO} define what success looks like before execution starts.\n",
        orchestrator.ticket_id, orchestrator.title
    ));
    text
}

pub fn render_plan(common: &Common<'_>, chain: &Chain) -> String {
    let orchestrator = &chain.orchestrator;
    let mut sequence: Vec<String> = chain
        .workers
        .iter()
        .map(|worker| {
            format!(
                "{}. {} on `{}`. {TODO} describe the step.",
                worker.step,
                worker.ticket.ticket_id,
                worker_branch(worker)
            )
        })
        .collect();
    if let Some(reviewer) = &chain.reviewer {
        sequence.push(format!(
            "{}. {} runs the final integrated review.",
            chain.workers.len() + 1,
            reviewer.ticket_id
        ));
    }
    let mut text = frontmatter(
        common,
        &Frontmatter {
            ticket_id: &orchestrator.ticket_id,
            title: &orchestrator.title,
            artifact: "plan",
            status: "planning",
            chain_role: Some("orchestrator"),
            chain_parent: None,
            chain_children: &[],
            dependencies: &[],
        },
    );
    text.push_str(&format!(
        "# {} Plan: {}\n\n## Sequence\n\n{}\n\nAfter each merge the reviewer runs the chain check, then a semantic pass.\n\n## Decisions To Lock Before Execution\n\n- {TODO} record any open process or implementation decision, or state none.\n",
        orchestrator.ticket_id,
        orchestrator.title,
        sequence.join("\n")
    ));
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontmatter::parse_document;

    fn steps() -> Vec<Step> {
        parse_steps(&["one:First step".to_owned(), "two:Second step".to_owned()]).unwrap()
    }

    #[test]
    fn steps_need_two_entries_and_titles() {
        assert_eq!(
            parse_steps(&["only:One".to_owned()]).unwrap_err().exit_code,
            2
        );
        assert!(parse_steps(&["a:".to_owned(), "b:B".to_owned()]).is_err());
        let parsed = parse_steps(&["Ship It Now".to_owned(), "x:Y".to_owned()]).unwrap();
        assert_eq!(parsed[0].slug, "ship-it-now");
        assert_eq!(parsed[0].title, "Ship It Now");
        assert_eq!(slugify("!!!"), "ticket");
    }

    #[test]
    fn chain_ids_are_consecutive_and_link_across_chunks() {
        let tasks = Path::new("/repo/.agents/tasks");
        let chain = build_chain(tasks, "alice", 24, "Big chain", &steps(), true, None);
        assert_eq!(chain.orchestrator.ticket_id, "TKT-0024");
        assert_eq!(chain.workers[1].ticket.ticket_id, "TKT-0026");
        let reviewer = chain.reviewer.as_ref().unwrap();
        assert_eq!(reviewer.ticket_id, "TKT-0027");
        assert_eq!(reviewer.title, "Review integrated Big chain chain");
        assert!(reviewer.dir.ends_with("TKT-0026-0050/TKT-0027"));
        assert_eq!(worker_branch(&chain.workers[0]), "tkt-0025-01-one");
        assert_eq!(
            ticket_link(&chain.orchestrator.dir, &chain.workers[1].ticket),
            "[TKT-0026 Second step](../../TKT-0026-0050/TKT-0026/task.md)"
        );
    }

    #[test]
    fn rendered_frontmatter_parses_and_carries_chain_fields() {
        let tasks = Path::new("/repo/.agents/tasks");
        let chain = build_chain(tasks, "alice", 3, "[Spike] chain", &steps(), true, None);
        let areas = vec![".agents/tasks".to_owned()];
        let skills = vec!["tkt".to_owned()];
        let common = Common {
            namespace: "alice",
            owner: "alice",
            created: "2026-09-03",
            updated: "2026-09-03",
            areas: &areas,
            skills: &skills,
            base_branch: "main",
            branch_pattern: DEFAULT_BRANCH_PATTERN,
            merge_policy: DEFAULT_MERGE_POLICY,
        };
        let orchestrator = render_orchestrator_task(&common, &chain, &[]);
        let parsed = parse_document("task.md", &orchestrator);
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        assert_eq!(parsed.metadata.scalar("title"), Some("[Spike] chain"));
        assert_eq!(parsed.metadata.scalar("chain_role"), Some("orchestrator"));
        assert_eq!(
            parsed.metadata.list("chain_children"),
            Some(
                &[
                    "TKT-0004".to_owned(),
                    "TKT-0005".to_owned(),
                    "TKT-0006".to_owned()
                ][..]
            )
        );
        assert!(
            orchestrator.contains("gritt-agent ticket chain-check --ticket TKT-0003 --base main")
        );

        let first = render_worker_task(&common, &chain, 0);
        assert!(!first.contains("dependencies:"));
        assert!(first.contains("status: ready"));
        let second = render_worker_task(&common, &chain, 1);
        assert!(second.contains("dependencies:\n  - TKT-0004\n"));
        assert!(second.contains("status: planning"));
        let reviewer = render_reviewer_task(&common, &chain);
        assert!(reviewer.contains("chain_parent: TKT-0003"));
        assert!(reviewer.contains("dependencies:\n  - TKT-0004\n  - TKT-0005\n"));
        for text in [&orchestrator, &first, &second, &reviewer] {
            assert!(text.contains(TODO));
        }
    }
}
