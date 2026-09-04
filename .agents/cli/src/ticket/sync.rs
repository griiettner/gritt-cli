//! `gritt-agent ticket sync`: regenerates ticket shard indexes, the chunk
//! router, and memory category indexes from frontmatter.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::store::{
    chunk_name, is_chunk_dir_name, iter_ticket_dirs, list_namespaces, pad_ticket_number,
    ticket_number, SHARED_NAMESPACE, TASK_SHARD_SIZE,
};
use crate::frontmatter::{self, FrontmatterError, Metadata};
use crate::fsx::{self, relative_posix};
use crate::repo::{memory_root, tasks_root};
use crate::Result;

const ARTIFACTS: [&str; 4] = ["concept", "plan", "task", "report"];

#[derive(Debug, Clone)]
struct TicketEntry {
    id: String,
    namespace: String,
    title: String,
    status: String,
    owner: String,
    priority: String,
    created: String,
    updated: String,
    artifacts: Vec<(String, String)>,
    dependencies: Vec<String>,
    areas: Vec<String>,
    skills: Vec<String>,
}

#[derive(Debug, Clone)]
struct Shard {
    namespace: String,
    range: String,
    file: String,
    count: usize,
    updated: String,
    statuses: BTreeMap<String, usize>,
    areas: Vec<String>,
}

struct Context<'a> {
    repo: &'a Path,
    check: bool,
    errors: Vec<FrontmatterError>,
    drift: Vec<String>,
}

impl Context<'_> {
    fn update_generated(&mut self, target: &Path, desired: &str) -> Result<()> {
        let current = fsx::read_text_or(target, "")?;
        if current == desired {
            return Ok(());
        }
        self.drift.push(relative_posix(self.repo, target));
        if !self.check {
            fsx::write_text(target, desired)?;
        }
        Ok(())
    }

    fn read_frontmatter(&mut self, target: &Path) -> Metadata {
        let parsed = frontmatter::load(target);
        self.errors.extend(parsed.errors);
        parsed.metadata
    }
}

/// Runs the sync and returns the process exit code. Output goes to stdout
/// and stderr exactly as the replaced script printed it.
pub fn run(repo: &Path, check: bool) -> Result<i32> {
    let tasks = tasks_root(repo);
    if !fsx::is_dir(&tasks) {
        eprintln!("error: missing tasks root: {}", tasks.display());
        return Ok(1);
    }
    let mut context = Context {
        repo,
        check,
        errors: Vec::new(),
        drift: Vec::new(),
    };
    // Collect first so a frontmatter error never leaves half-written indexes.
    let entries = collect_ticket_entries(&mut context, &tasks)?;
    let memory_indexes = collect_memory_indexes(&mut context, &memory_root(repo))?;
    if !context.errors.is_empty() {
        for error in &context.errors {
            eprintln!("error: {}", error.render());
        }
        eprintln!(
            "tkt_sync failed ({} frontmatter error(s))",
            context.errors.len()
        );
        return Ok(1);
    }
    write_task_indexes(&mut context, &tasks, &entries)?;
    for (target, desired) in &memory_indexes {
        context.update_generated(target, desired)?;
    }
    if check {
        if !context.drift.is_empty() {
            for item in &context.drift {
                eprintln!("drift: {item}");
            }
            eprintln!(
                "tkt_sync: {} generated index file(s) out of sync",
                context.drift.len()
            );
            return Ok(1);
        }
        println!("tkt_sync ok (no drift)");
        return Ok(0);
    }
    println!("synced .agents task and memory indexes");
    Ok(0)
}

fn collect_ticket_entries(context: &mut Context<'_>, tasks: &Path) -> Result<Vec<TicketEntry>> {
    let mut entries = Vec::new();
    for ticket in iter_ticket_dirs(tasks)? {
        let artifacts = existing_artifacts(context.repo, &ticket.dir);
        let primary = first_existing(&ticket.dir, &["task", "concept", "plan", "report"]);
        let metadata = match primary {
            Some(path) => context.read_frontmatter(&path),
            None => Metadata::default(),
        };
        let report_metadata = context.read_frontmatter(&ticket.dir.join("report.md"));
        let created = metadata.scalar("created").unwrap_or("").to_owned();
        let newest = newest_updated(&ticket.dir)?;
        let updated = if !newest.is_empty() {
            newest
        } else {
            metadata
                .scalar("updated")
                .map(str::to_owned)
                .unwrap_or_else(|| created.clone())
        };
        entries.push(TicketEntry {
            id: ticket.ticket_id.clone(),
            namespace: ticket.namespace.clone(),
            title: metadata
                .scalar("title")
                .unwrap_or(&ticket.ticket_id)
                .to_owned(),
            status: report_metadata
                .scalar("status")
                .or_else(|| metadata.scalar("status"))
                .unwrap_or("planning")
                .to_owned(),
            owner: metadata
                .scalar("owner")
                .unwrap_or(&ticket.namespace)
                .to_owned(),
            priority: "normal".to_owned(),
            created,
            updated,
            artifacts,
            dependencies: metadata.list_or_empty("dependencies"),
            areas: metadata.list_or_empty("areas"),
            skills: metadata.list_or_empty("skills"),
        });
    }
    Ok(entries)
}

fn write_task_indexes(
    context: &mut Context<'_>,
    tasks: &Path,
    entries: &[TicketEntry],
) -> Result<()> {
    let mut shards = Vec::new();
    let mut known: BTreeSet<String> = list_namespaces(tasks)?
        .into_iter()
        .map(|namespace| namespace.id)
        .collect();
    for entry in entries {
        known.insert(entry.namespace.clone());
    }

    for namespace in &known {
        let namespace_entries: Vec<&TicketEntry> = entries
            .iter()
            .filter(|entry| &entry.namespace == namespace)
            .collect();
        let namespace_root = if namespace == SHARED_NAMESPACE {
            tasks.to_path_buf()
        } else {
            tasks.join(namespace)
        };
        let maximum = namespace_entries
            .iter()
            .map(|entry| ticket_number(&entry.id))
            .max()
            .unwrap_or(0);
        let mut expected_chunks = BTreeSet::new();
        let mut start = 1;
        while start <= maximum {
            let end = start + TASK_SHARD_SIZE - 1;
            let shard_entries: Vec<&TicketEntry> = namespace_entries
                .iter()
                .copied()
                .filter(|entry| {
                    let number = ticket_number(&entry.id);
                    start <= number && number <= end
                })
                .collect();
            if !shard_entries.is_empty() {
                let name = chunk_name(start);
                expected_chunks.insert(name.clone());
                let shard_path = namespace_root.join(&name).join("index.yaml");
                context.update_generated(&shard_path, &render_tasks_index(&shard_entries))?;
                shards.push(build_shard_metadata(
                    context.repo,
                    namespace,
                    start,
                    end,
                    &shard_path,
                    &shard_entries,
                ));
            }
            start += TASK_SHARD_SIZE;
        }

        if fsx::is_dir(&namespace_root) {
            for chunk in fsx::list_dirs(&namespace_root)? {
                if !is_chunk_dir_name(&chunk.name) || expected_chunks.contains(&chunk.name) {
                    continue;
                }
                let shard_path = chunk.path.join("index.yaml");
                if !fsx::exists(&shard_path) {
                    continue;
                }
                context.drift.push(format!(
                    "{} (stale)",
                    relative_posix(context.repo, &shard_path)
                ));
                if !context.check {
                    fsx::remove_file(&shard_path)?;
                }
            }
        }
    }

    context.update_generated(&tasks.join("index.yaml"), &render_task_router(&shards))
}

fn collect_memory_indexes(
    context: &mut Context<'_>,
    memory: &Path,
) -> Result<Vec<(PathBuf, String)>> {
    let mut indexes = Vec::new();
    if !fsx::is_dir(memory) {
        return Ok(indexes);
    }
    for category in fsx::list_dirs(memory)? {
        let mut memories = Vec::new();
        for entry in fsx::list_entries(&category.path)? {
            if !entry.is_file || !entry.name.ends_with(".md") || entry.name == "index.md" {
                continue;
            }
            let metadata = context.read_frontmatter(&entry.path);
            let stem = &entry.name[..entry.name.len() - 3];
            memories.push(MemoryEntry {
                id: metadata.scalar("id").unwrap_or(stem).to_owned(),
                title: metadata
                    .scalar("title")
                    .map(str::to_owned)
                    .unwrap_or_else(|| title_from_slug(stem)),
                file: entry.name.clone(),
                tags: metadata.list_or_empty("tags"),
                read_when: metadata.list_or_empty("read_when"),
            });
        }
        if !memories.is_empty() {
            indexes.push((
                category.path.join("index.yaml"),
                render_memory_index(&memories),
            ));
        }
    }
    Ok(indexes)
}

struct MemoryEntry {
    id: String,
    title: String,
    file: String,
    tags: Vec<String>,
    read_when: Vec<String>,
}

fn existing_artifacts(repo: &Path, ticket_dir: &Path) -> Vec<(String, String)> {
    let mut artifacts = Vec::new();
    for artifact in ARTIFACTS {
        let target = ticket_dir.join(format!("{artifact}.md"));
        if fsx::exists(&target) {
            artifacts.push((artifact.to_owned(), relative_posix(repo, &target)));
        }
    }
    let updates = ticket_dir.join("updates");
    if fsx::is_dir(&updates) {
        artifacts.push((
            "updates".to_owned(),
            format!("{}/", relative_posix(repo, &updates)),
        ));
    }
    artifacts
}

fn first_existing(ticket_dir: &Path, names: &[&str]) -> Option<PathBuf> {
    names
        .iter()
        .map(|name| ticket_dir.join(format!("{name}.md")))
        .find(|target| fsx::exists(target))
}

fn newest_updated(ticket_dir: &Path) -> Result<String> {
    let mut targets = Vec::new();
    for entry in fsx::list_entries(ticket_dir)? {
        if entry.is_file && entry.name.ends_with(".md") {
            targets.push(entry.path);
        }
    }
    let updates = ticket_dir.join("updates");
    if fsx::is_dir(&updates) {
        for entry in fsx::list_entries(&updates)? {
            if entry.is_file && entry.name.ends_with(".md") {
                targets.push(entry.path);
            }
        }
    }
    targets.sort();
    let mut values = Vec::new();
    for target in targets {
        let parsed = frontmatter::load(&target);
        if parsed.errors.is_empty() {
            if let Some(updated) = parsed.metadata.scalar("updated") {
                values.push(updated.to_owned());
            }
        }
    }
    values.sort();
    Ok(values.pop().unwrap_or_default())
}

fn render_tasks_index(entries: &[&TicketEntry]) -> String {
    let mut lines = vec!["tickets:".to_owned()];
    for entry in entries {
        lines.push(format!("  - id: {}", entry.id));
        lines.push(format!("    namespace: {}", entry.namespace));
        lines.push(format!("    title: {}", entry.title));
        lines.push(format!("    status: {}", entry.status));
        lines.push(format!("    owner: {}", entry.owner));
        lines.push(format!("    priority: {}", entry.priority));
        lines.push(format!("    created: {}", entry.created));
        lines.push(format!("    updated: {}", entry.updated));
        lines.push("    artifacts:".to_owned());
        for (key, value) in &entry.artifacts {
            lines.push(format!("      {key}: {value}"));
        }
        render_list(&mut lines, "dependencies", &entry.dependencies, "    ");
        render_list(&mut lines, "areas", &entry.areas, "    ");
        render_list(&mut lines, "skills", &entry.skills, "    ");
    }
    format!("{}\n", lines.join("\n"))
}

fn render_task_router(shards: &[Shard]) -> String {
    if shards.is_empty() {
        return "# Generated chunk router for .agents/tasks/.\n# No tickets yet. Create one with the tkt-new skill, then rerun:\n#   gritt-agent ticket sync\n".to_owned();
    }
    let mut lines = Vec::new();
    for shard in shards {
        lines.push(format!("- namespace: {}", shard.namespace));
        lines.push(format!("  range: {}", shard.range));
        lines.push(format!("  file: {}", shard.file));
        lines.push(format!("  count: {}", shard.count));
        lines.push(format!("  updated: {}", shard.updated));
        lines.push("  statuses:".to_owned());
        for (status, count) in &shard.statuses {
            lines.push(format!("    - {status}: {count}"));
        }
        render_list(&mut lines, "areas", &shard.areas, "  ");
    }
    format!("{}\n", lines.join("\n"))
}

fn render_memory_index(memories: &[MemoryEntry]) -> String {
    let mut lines = vec!["memories:".to_owned()];
    for memory in memories {
        lines.push(format!("  - id: {}", memory.id));
        lines.push(format!("    title: {}", memory.title));
        lines.push(format!("    file: {}", memory.file));
        render_list(&mut lines, "tags", &memory.tags, "    ");
        render_list(&mut lines, "read_when", &memory.read_when, "    ");
    }
    format!("{}\n", lines.join("\n"))
}

fn render_list(lines: &mut Vec<String>, key: &str, values: &[String], indent: &str) {
    if values.is_empty() {
        lines.push(format!("{indent}{key}: []"));
    } else {
        lines.push(format!("{indent}{key}:"));
        for value in values {
            lines.push(format!("{indent}  - {value}"));
        }
    }
}

fn build_shard_metadata(
    repo: &Path,
    namespace: &str,
    start: u32,
    end: u32,
    shard_path: &Path,
    entries: &[&TicketEntry],
) -> Shard {
    let mut statuses: BTreeMap<String, usize> = BTreeMap::new();
    let mut areas = BTreeSet::new();
    let mut updated = Vec::new();
    for entry in entries {
        *statuses.entry(entry.status.clone()).or_insert(0) += 1;
        for area in &entry.areas {
            areas.insert(area.clone());
        }
        if !entry.updated.is_empty() {
            updated.push(entry.updated.clone());
        }
    }
    updated.sort();
    Shard {
        namespace: namespace.to_owned(),
        range: format!(
            "TKT-{}-{}",
            pad_ticket_number(start),
            pad_ticket_number(end)
        ),
        file: relative_posix(repo, shard_path),
        count: entries.len(),
        updated: updated.pop().unwrap_or_default(),
        statuses,
        areas: areas.into_iter().collect(),
    }
}

/// Turns `adr-001-agent` into `Adr 001 Agent`.
pub fn title_from_slug(slug: &str) -> String {
    let mut result = String::with_capacity(slug.len());
    let mut previous_is_word = false;
    for ch in slug.chars() {
        let ch = if ch == '-' || ch == '_' { ' ' } else { ch };
        let is_word = ch.is_ascii_alphanumeric() || ch == '_';
        if is_word && !previous_is_word {
            result.extend(ch.to_uppercase());
        } else {
            result.push(ch);
        }
        previous_is_word = is_word;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_titles() {
        assert_eq!(title_from_slug("model-routing"), "Model Routing");
        assert_eq!(title_from_slug("adr_001-x"), "Adr 001 X");
    }

    #[test]
    fn empty_router_names_the_rust_command() {
        assert!(render_task_router(&[]).contains("gritt-agent ticket sync"));
    }
}
