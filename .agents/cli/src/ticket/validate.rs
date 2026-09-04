//! `gritt-agent ticket validate`: checks ticket folders, frontmatter, chain
//! links, memory frontmatter, and the optional generated indexes.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;

use super::store::{
    chunk_name, is_chunk_dir_name, is_namespace_name, is_ticket_id, list_namespaces,
    qualified_ticket_id, ticket_number, SHARED_NAMESPACE,
};
use crate::frontmatter::{self, Metadata};
use crate::fsx;
use crate::repo::{memory_root, tasks_root};
use crate::Result;

const KNOWN_ARTIFACTS: [&str; 4] = ["concept", "plan", "task", "report"];
const REQUIRED_FIELDS: [&str; 6] = ["id", "title", "artifact", "status", "created", "updated"];
const ALLOWED_STATUSES: [&str; 7] = [
    "concept",
    "planning",
    "ready",
    "in_progress",
    "done",
    "blocked",
    "cancelled",
];
const ALLOWED_ARTIFACTS: [&str; 5] = ["concept", "plan", "task", "report", "update"];
const ALLOWED_CHAIN_ROLES: [&str; 3] = ["orchestrator", "worker", "reviewer"];
const SCAFFOLD_MARKER: &str = "TODO(tkt):";

fn index_path_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\.agents/tasks/[^\s,\]}]+").unwrap())
}

fn shard_path_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\.agents/tasks/(?:[A-Za-z0-9._-]+/)?TKT-\d{4}-\d{4}/index\.yaml").unwrap()
    })
}

fn shard_namespace_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\.agents/tasks/(?:([^/]+)/)?TKT-\d{4}-\d{4}/index\.yaml").unwrap()
    })
}

fn indexed_id_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*- id:\s+(TKT-\d{4})\s*$").unwrap())
}

fn indexed_namespace_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s+namespace:\s+(\S+)\s*$").unwrap())
}

#[derive(Debug, Default)]
pub struct Outcome {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl Outcome {
    fn error(&mut self, message: impl Into<String>) {
        self.errors.push(message.into());
    }

    fn warn(&mut self, message: impl Into<String>) {
        self.warnings.push(message.into());
    }
}

#[derive(Debug, Clone)]
struct ChainRecord {
    namespace: String,
    ticket_id: String,
    role: String,
    parent: Option<String>,
    children: Vec<String>,
}

/// Validates the repository and returns the exit code after printing the
/// outcome in the same shape as the replaced script.
pub fn run(repo: &Path) -> Result<i32> {
    let outcome = validate(repo)?;
    Ok(finish(&outcome))
}

pub fn validate(repo: &Path) -> Result<Outcome> {
    let root = tasks_root(repo);
    let mut outcome = Outcome::default();
    if !fsx::is_dir(&root) {
        outcome.error(format!("tasks root does not exist: {}", root.display()));
        return Ok(outcome);
    }
    let mut chains = BTreeMap::new();
    let ticket_ids = validate_ticket_folders(&root, &mut outcome, &mut chains)?;
    validate_chains(&chains, &mut outcome);
    validate_memory_frontmatter(&memory_root(repo), &mut outcome)?;
    validate_optional_index(&root, repo, &ticket_ids, &mut outcome)?;
    Ok(outcome)
}

fn validate_ticket_folders(
    root: &Path,
    outcome: &mut Outcome,
    chains: &mut BTreeMap<String, ChainRecord>,
) -> Result<BTreeSet<String>> {
    let mut ticket_ids = BTreeSet::new();
    for namespace in list_namespaces(root)? {
        if !fsx::is_dir(&namespace.root) {
            continue;
        }
        for chunk in fsx::list_dirs(&namespace.root)? {
            if namespace.id == SHARED_NAMESPACE && is_namespace_name(&chunk.name) {
                continue;
            }
            if !is_chunk_dir_name(&chunk.name) {
                outcome.error(format!(
                    "invalid chunk folder name: {}",
                    chunk.path.display()
                ));
                continue;
            }
            validate_chunk_folder(
                &chunk.path,
                &chunk.name,
                &namespace.id,
                &mut ticket_ids,
                outcome,
                chains,
            )?;
        }
    }
    Ok(ticket_ids)
}

fn validate_chunk_folder(
    chunk_path: &Path,
    chunk_dir_name: &str,
    namespace: &str,
    ticket_ids: &mut BTreeSet<String>,
    outcome: &mut Outcome,
    chains: &mut BTreeMap<String, ChainRecord>,
) -> Result<()> {
    for entry in fsx::list_entries(chunk_path)? {
        if entry.is_file {
            if entry.name != "index.yaml" {
                outcome.warn(format!(
                    "unexpected file in chunk folder: {}",
                    entry.path.display()
                ));
            }
            continue;
        }
        if !entry.is_dir {
            continue;
        }
        let ticket_id = entry.name.clone();
        if !is_ticket_id(&ticket_id) {
            outcome.error(format!(
                "invalid ticket folder name: {}",
                entry.path.display()
            ));
            continue;
        }
        let expected_chunk = chunk_name(ticket_number(&ticket_id));
        if expected_chunk != chunk_dir_name {
            outcome.error(format!(
                "{} is in {chunk_dir_name} but belongs in {expected_chunk}",
                qualified_ticket_id(namespace, &ticket_id)
            ));
        }
        ticket_ids.insert(qualified_ticket_id(namespace, &ticket_id));
        validate_ticket_folder(&entry.path, &ticket_id, namespace, outcome, chains)?;
    }
    Ok(())
}

fn validate_ticket_folder(
    ticket_path: &Path,
    ticket_id: &str,
    namespace: &str,
    outcome: &mut Outcome,
    chains: &mut BTreeMap<String, ChainRecord>,
) -> Result<()> {
    let mut artifacts = BTreeSet::new();
    for artifact in KNOWN_ARTIFACTS {
        let artifact_path = ticket_path.join(format!("{artifact}.md"));
        if fsx::exists(&artifact_path) {
            artifacts.insert(artifact);
            let metadata = validate_artifact(&artifact_path, ticket_id, artifact, outcome)?;
            if artifact == "task" {
                if let Some(role) = metadata.as_ref().and_then(|m| m.scalar("chain_role")) {
                    chains.insert(
                        qualified_ticket_id(namespace, ticket_id),
                        ChainRecord {
                            namespace: namespace.to_owned(),
                            ticket_id: ticket_id.to_owned(),
                            role: role.to_owned(),
                            parent: metadata
                                .as_ref()
                                .and_then(|m| m.scalar("chain_parent"))
                                .map(str::to_owned),
                            children: metadata
                                .as_ref()
                                .map(|m| m.list_or_empty("chain_children"))
                                .unwrap_or_default(),
                        },
                    );
                }
            }
        }
    }
    let updates = ticket_path.join("updates");
    if fsx::exists(&updates) {
        if !fsx::is_dir(&updates) {
            outcome.error(format!(
                "updates path is not a directory: {}",
                updates.display()
            ));
        } else {
            for entry in fsx::list_entries(&updates)? {
                if entry.is_file && entry.name.ends_with(".md") {
                    validate_artifact(&entry.path, ticket_id, "update", outcome)?;
                }
            }
        }
    }
    if artifacts.is_empty() {
        outcome.error(format!("{ticket_id} has no lifecycle artifacts"));
    } else if !artifacts.contains("task") {
        outcome.warn(format!(
            "{ticket_id} has no task.md; valid for early concepts, but not executable"
        ));
    }
    Ok(())
}

fn validate_artifact(
    target: &Path,
    ticket_id: &str,
    expected_artifact: &str,
    outcome: &mut Outcome,
) -> Result<Option<Metadata>> {
    let content = match fsx::read_text(target) {
        Ok(content) => content,
        Err(error) => {
            outcome.error(format!(
                "{}: cannot read file: {}",
                target.display(),
                error.message
            ));
            return Ok(None);
        }
    };
    let parsed = frontmatter::parse_document(&target.to_string_lossy(), &content);
    if !parsed.errors.is_empty() {
        for error in parsed.errors {
            outcome.error(error.render());
        }
        return Ok(None);
    }
    let metadata = parsed.metadata;
    let display = target.display();
    if metadata.is_empty() {
        outcome.error(format!("missing YAML frontmatter: {display}"));
        return Ok(None);
    }
    check_scaffold_markers(&content, target, outcome);
    for field in REQUIRED_FIELDS {
        if metadata.scalar(field).is_none() {
            outcome.error(format!("missing `{field}` in {display}"));
        }
    }
    if let Some(id) = metadata.scalar("id") {
        if id != ticket_id {
            outcome.error(format!(
                "id mismatch in {display}: expected {ticket_id}, got {id}"
            ));
        }
    }
    if let Some(role) = metadata.scalar("chain_role") {
        if !ALLOWED_CHAIN_ROLES.contains(&role) {
            outcome.error(format!("unsupported chain_role in {display}: {role}"));
        }
    }
    if let Some(artifact) = metadata.scalar("artifact") {
        if !ALLOWED_ARTIFACTS.contains(&artifact) {
            outcome.error(format!(
                "unsupported artifact value in {display}: {artifact}"
            ));
        }
        if artifact != expected_artifact {
            outcome.error(format!(
                "artifact mismatch in {display}: expected {expected_artifact}, got {artifact}"
            ));
        }
    }
    if let Some(status) = metadata.scalar("status") {
        if !ALLOWED_STATUSES.contains(&status) {
            outcome.warn(format!("suspicious status value in {display}: {status}"));
        }
    }
    Ok(Some(metadata))
}

fn check_scaffold_markers(content: &str, target: &Path, outcome: &mut Outcome) {
    let pending = content
        .lines()
        .filter(|line| line.contains(SCAFFOLD_MARKER))
        .count();
    if pending > 0 {
        outcome.error(format!(
            "{} still has {pending} unfilled scaffold line(s) marked `{SCAFFOLD_MARKER}`; replace them with real content",
            target.display()
        ));
    }
}

fn validate_chains(chains: &BTreeMap<String, ChainRecord>, outcome: &mut Outcome) {
    for entry in chains.values() {
        let qualified = qualified_ticket_id(&entry.namespace, &entry.ticket_id);
        if entry.role == "orchestrator" {
            if entry.children.is_empty() {
                outcome.error(format!(
                    "{qualified} is a chain orchestrator with no `chain_children`; a chain is an orchestrator plus one ticket per worker step"
                ));
                continue;
            }
            for child in &entry.children {
                let child_key = qualified_ticket_id(&entry.namespace, child);
                let Some(record) = chains.get(&child_key) else {
                    outcome.error(format!(
                        "{qualified} lists chain child {child}, but no such chain ticket exists in namespace {}",
                        entry.namespace
                    ));
                    continue;
                };
                if record.parent.as_deref() != Some(entry.ticket_id.as_str()) {
                    outcome.error(format!(
                        "{child_key} has chain_parent {} but is listed as a child of {}",
                        record.parent.as_deref().unwrap_or("none"),
                        entry.ticket_id
                    ));
                }
            }
            continue;
        }
        let Some(parent) = entry.parent.as_deref() else {
            outcome.error(format!(
                "{qualified} has chain_role {} but no `chain_parent`",
                entry.role
            ));
            continue;
        };
        let parent_key = qualified_ticket_id(&entry.namespace, parent);
        match chains.get(&parent_key) {
            None => outcome.error(format!(
                "{qualified} points at missing chain parent {parent}"
            )),
            Some(record) if !record.children.contains(&entry.ticket_id) => outcome.error(format!(
                "{parent_key} does not list {} in `chain_children`",
                entry.ticket_id
            )),
            Some(_) => {}
        }
    }
}

fn validate_memory_frontmatter(memory: &Path, outcome: &mut Outcome) -> Result<()> {
    if !fsx::is_dir(memory) {
        return Ok(());
    }
    for category in fsx::list_dirs(memory)? {
        for entry in fsx::list_entries(&category.path)? {
            if !entry.is_file || !entry.name.ends_with(".md") || entry.name == "index.md" {
                continue;
            }
            let parsed = frontmatter::load(&entry.path);
            for error in &parsed.errors {
                outcome.warn(error.render());
            }
            let display = entry.path.display();
            if parsed.metadata.is_empty() {
                outcome.warn(format!(
                    "memory file is missing YAML frontmatter: {display}"
                ));
                continue;
            }
            if parsed.metadata.scalar("id").is_none() {
                outcome.warn(format!("memory file is missing `id`: {display}"));
            }
            if parsed.metadata.scalar("title").is_none() {
                outcome.warn(format!("memory file is missing `title`: {display}"));
            }
        }
    }
    Ok(())
}

fn validate_optional_index(
    root: &Path,
    repo: &Path,
    ticket_ids: &BTreeSet<String>,
    outcome: &mut Outcome,
) -> Result<()> {
    let index_path = root.join("index.yaml");
    if !fsx::exists(&index_path) {
        outcome.warn(
            "index.yaml is missing; this is allowed because ticket folders are source of truth",
        );
        return Ok(());
    }
    let content = match fsx::read_text(&index_path) {
        Ok(content) => content,
        Err(error) => {
            outcome.warn(format!(
                "cannot read optional index {}: {}",
                index_path.display(),
                error.message
            ));
            return Ok(());
        }
    };
    let indexed_ids = indexed_ticket_keys(&content, SHARED_NAMESPACE);
    if shard_path_regex().is_match(&content) {
        return validate_sharded_index(repo, &content, ticket_ids, outcome);
    }
    let has_content = content
        .lines()
        .any(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'));
    if indexed_ids.is_empty() && has_content {
        outcome.warn("optional index.yaml has no recognizable ticket ids");
    } else if indexed_ids.is_empty() && !ticket_ids.is_empty() {
        outcome
            .warn("index.yaml is empty but ticket folders exist; rerun `gritt-agent ticket sync`");
    }
    for ticket_id in indexed_ids.difference(ticket_ids) {
        outcome.warn(format!(
            "optional index references missing ticket folder: {ticket_id}"
        ));
    }
    for ticket_id in ticket_ids.difference(&indexed_ids) {
        outcome.warn(format!(
            "optional index omits existing ticket folder: {ticket_id}"
        ));
    }
    for raw_path in index_path_regex().find_iter(&content).map(|m| m.as_str()) {
        if !index_path_is_valid(repo, raw_path) {
            outcome.warn(format!(
                "optional index references missing artifact path: {raw_path}"
            ));
        }
    }
    Ok(())
}

fn validate_sharded_index(
    repo: &Path,
    content: &str,
    ticket_ids: &BTreeSet<String>,
    outcome: &mut Outcome,
) -> Result<()> {
    let shard_paths: Vec<&str> = shard_path_regex()
        .find_iter(content)
        .map(|m| m.as_str())
        .collect();
    if shard_paths.is_empty() {
        outcome.warn("optional sharded index.yaml has no recognizable shard paths");
        return Ok(());
    }
    let mut indexed_ids = BTreeSet::new();
    for raw_path in shard_paths {
        let shard_path = resolve_index_path(repo, raw_path);
        if !fsx::exists(&shard_path) {
            outcome.warn(format!(
                "optional index references missing shard path: {raw_path}"
            ));
            continue;
        }
        let shard_content = match fsx::read_text(&shard_path) {
            Ok(text) => text,
            Err(error) => {
                outcome.warn(format!(
                    "cannot read optional shard {raw_path}: {}",
                    error.message
                ));
                continue;
            }
        };
        indexed_ids.extend(indexed_ticket_keys(
            &shard_content,
            &namespace_from_shard_path(raw_path),
        ));
        for artifact_path in index_path_regex()
            .find_iter(&shard_content)
            .map(|m| m.as_str())
        {
            if !index_path_is_valid(repo, artifact_path) {
                outcome.warn(format!(
                    "optional shard references missing artifact path: {artifact_path}"
                ));
            }
        }
    }
    for ticket_id in indexed_ids.difference(ticket_ids) {
        outcome.warn(format!(
            "optional shard index references missing ticket folder: {ticket_id}"
        ));
    }
    for ticket_id in ticket_ids.difference(&indexed_ids) {
        outcome.warn(format!(
            "optional shard indexes omit existing ticket folder: {ticket_id}"
        ));
    }
    Ok(())
}

fn index_path_is_valid(repo: &Path, raw_path: &str) -> bool {
    let normalized = raw_path.strip_suffix('/').unwrap_or(raw_path);
    let target = resolve_index_path(repo, normalized);
    if raw_path.ends_with('/') {
        fsx::is_dir(&target)
    } else {
        fsx::exists(&target)
    }
}

fn indexed_ticket_keys(content: &str, default_namespace: &str) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    let mut current_id: Option<String> = None;
    for line in content.lines() {
        if let Some(captures) = indexed_id_regex().captures(line) {
            if let Some(id) = current_id.take() {
                keys.insert(qualified_ticket_id(default_namespace, &id));
            }
            current_id = Some(captures[1].to_owned());
            continue;
        }
        if let Some(captures) = indexed_namespace_regex().captures(line) {
            if let Some(id) = current_id.take() {
                keys.insert(qualified_ticket_id(&captures[1], &id));
            }
        }
    }
    if let Some(id) = current_id {
        keys.insert(qualified_ticket_id(default_namespace, &id));
    }
    keys
}

fn namespace_from_shard_path(raw_path: &str) -> String {
    let maybe = shard_namespace_regex()
        .captures(raw_path)
        .and_then(|c| c.get(1).map(|m| m.as_str().to_owned()));
    match maybe {
        Some(namespace) if !is_chunk_dir_name(&namespace) => namespace,
        _ => SHARED_NAMESPACE.to_owned(),
    }
}

fn resolve_index_path(repo: &Path, raw_path: &str) -> PathBuf {
    let path = Path::new(raw_path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        repo.join(path)
    }
}

fn finish(outcome: &Outcome) -> i32 {
    for warning in &outcome.warnings {
        eprintln!("warning: {warning}");
    }
    let warning_plural = if outcome.warnings.len() == 1 { "" } else { "s" };
    if outcome.errors.is_empty() {
        println!(
            "tkt_validate ok ({} warning{warning_plural})",
            outcome.warnings.len()
        );
        return 0;
    }
    for error in &outcome.errors {
        eprintln!("error: {error}");
    }
    let error_plural = if outcome.errors.len() == 1 { "" } else { "s" };
    eprintln!(
        "tkt_validate failed ({} error{error_plural}, {} warning{warning_plural})",
        outcome.errors.len(),
        outcome.warnings.len()
    );
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexed_keys_use_namespace_lines() {
        let content = "tickets:\n  - id: TKT-0001\n    namespace: alice\n  - id: TKT-0002\n";
        let keys = indexed_ticket_keys(content, "_shared");
        assert!(keys.contains("alice/TKT-0001"));
        assert!(keys.contains("_shared/TKT-0002"));
    }

    #[test]
    fn shard_namespace_detection() {
        assert_eq!(
            namespace_from_shard_path(".agents/tasks/alice/TKT-0001-0025/index.yaml"),
            "alice"
        );
        assert_eq!(
            namespace_from_shard_path(".agents/tasks/TKT-0001-0025/index.yaml"),
            "_shared"
        );
    }
}
