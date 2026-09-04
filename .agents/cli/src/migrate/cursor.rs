//! `gritt-agent migrate cursor`: imports `.cursor/` and `.claude/` skills,
//! agents, and rules from another repository into `.agents/`.
//!
//! Discovery, classification, and rendering are pure functions over an
//! in-memory document so they can be tested without a source tree. The run
//! function only plans, writes, and hands off to the maintenance commands.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::json;

use crate::frontmatter::{split_fence, split_lines, Split};
use crate::fsx::{self, compare_names, kebab_case, relative_posix};
use crate::repo::{expand_home, local_date, skills_root};
use crate::skill::sync::{display_name, quote_yaml};
use crate::Result;

pub const MIGRATION_MARKER: &str = "<!-- MIGRATED BY gritt-agent migrate cursor; DO NOT EDIT -->";
const LEGACY_MARKERS: [&str; 3] = [
    "<!-- MIGRATED BY nx run agent-tools:migrate-cursor-setup -->",
    "<!-- MIGRATED BY migrate-cursor-setup.mjs -->",
    "<!-- MIGRATED BY migrate_cursor_setup.py -->",
];
const SKILL_SOURCE_DIRS: [&str; 5] = [
    ".cursor/commands",
    ".cursor/skills",
    ".cursor/prompts",
    ".claude/commands",
    ".claude/skills",
];
const AGENT_SOURCE_DIRS: [&str; 4] = [
    ".cursor/agents",
    ".cursor/agent",
    ".claude/agents",
    ".claude/agent",
];
const MEMORY_SOURCE_DIRS: [&str; 8] = [
    ".cursor/rules",
    ".cursor/memory",
    ".cursor/memories",
    ".cursor/context",
    ".claude/rules",
    ".claude/memory",
    ".claude/memories",
    ".claude/context",
];
const SUPPORTED_EXTENSIONS: [&str; 3] = ["md", "mdc", "txt"];

fn sentence_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(.+?[.!?])(?:\s|$)").unwrap())
}

fn paragraph_split_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\n\s*\n").unwrap())
}

#[derive(Debug, Clone)]
pub struct CursorOptions {
    pub source: PathBuf,
    pub dry_run: bool,
    pub force: bool,
    pub no_sync: bool,
}

/// One source document with its frontmatter split off.
#[derive(Debug, Clone, Default)]
pub struct SourceDoc {
    pub rel_path: String,
    pub stem: String,
    pub frontmatter: Vec<(String, String)>,
    pub body: String,
    pub title: String,
    pub description: String,
}

impl SourceDoc {
    fn field(&self, key: &str) -> Option<&str> {
        self.frontmatter
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct Write {
    pub destination: PathBuf,
    pub content: String,
    pub kind: &'static str,
    pub source: String,
    pub confidence: &'static str,
    pub ambiguous_reason: String,
}

#[derive(Debug, Clone)]
pub struct Skipped {
    pub destination: PathBuf,
    pub source: String,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct CommandRun {
    pub argv: Vec<String>,
    pub returncode: i32,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Default)]
pub struct Report {
    pub migrated: Vec<Write>,
    pub skipped: Vec<Skipped>,
    pub ambiguous: Vec<Write>,
    pub commands: Vec<CommandRun>,
}

pub fn run(repo: &Path, options: &CursorOptions) -> Result<i32> {
    let source = resolve_existing(&expand_home(&options.source));
    if !fsx::is_dir(&source) {
        eprintln!("error: source repo does not exist: {}", source.display());
        return Ok(1);
    }
    if source == resolve_existing(repo) {
        eprintln!("error: source and target repo must be different paths");
        return Ok(1);
    }

    let mut report = Report::default();
    let mut writes = plan_migration(repo, &source, &mut report, options.force)?;
    writes.extend(plan_reports(repo, &source, &writes, &report));
    if options.dry_run {
        println!("{}", render_console_summary(&writes, &report, true));
        return Ok(0);
    }
    apply_writes(&writes, &mut report, options.force)?;
    if !options.no_sync {
        run_maintenance(repo, &source, &mut report)?;
    }
    println!("{}", render_console_summary(&writes, &report, false));
    Ok(i32::from(
        report
            .commands
            .iter()
            .any(|command| command.returncode != 0),
    ))
}

fn plan_migration(
    repo: &Path,
    source: &Path,
    report: &mut Report,
    force: bool,
) -> Result<Vec<Write>> {
    let mut writes = Vec::new();
    let mut seen = BTreeSet::new();
    let mut claimed = BTreeMap::new();
    for doc in discover_docs(source, &SKILL_SOURCE_DIRS, &mut seen)? {
        let planned = claim_destinations(plan_skill(repo, &doc), &mut claimed, report);
        writes.extend(filter_existing(planned, report, force));
    }
    for doc in discover_docs(source, &AGENT_SOURCE_DIRS, &mut seen)? {
        let planned = claim_destinations(plan_agent(repo, &doc), &mut claimed, report);
        writes.extend(filter_existing(planned, report, force));
    }
    for doc in discover_docs(source, &MEMORY_SOURCE_DIRS, &mut seen)? {
        let planned = claim_destinations(vec![plan_memory(repo, &doc)], &mut claimed, report);
        let filtered = filter_existing(planned, report, force);
        if let Some(write) = filtered.first() {
            if write.confidence != "high" {
                report.ambiguous.push(write.clone());
            }
        }
        writes.extend(filtered);
    }
    Ok(writes)
}

fn discover_docs(
    source: &Path,
    source_dirs: &[&str],
    seen: &mut BTreeSet<PathBuf>,
) -> Result<Vec<SourceDoc>> {
    let mut docs = Vec::new();
    for raw_dir in source_dirs {
        for target in fsx::list_files_recursive(&source.join(raw_dir))? {
            let extension = target
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if !SUPPORTED_EXTENSIONS.contains(&extension.as_str()) {
                continue;
            }
            // Overlapping source roots (`.cursor/agent` linked to
            // `.cursor/agents`) must yield one document per physical file.
            let physical = target.canonicalize().unwrap_or_else(|_| target.clone());
            if !seen.insert(physical) {
                continue;
            }
            let content = fsx::read_text_lossy(&target)?;
            docs.push(load_source_doc(
                &relative_posix(source, &target),
                &file_stem(&target),
                &content,
            ));
        }
    }
    Ok(docs)
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub fn load_source_doc(rel_path: &str, stem: &str, content: &str) -> SourceDoc {
    let (frontmatter, raw_body) = split_frontmatter(content);
    let body = raw_body.trim().to_owned();
    let field = |key: &str| {
        frontmatter
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.is_empty())
    };
    let title = {
        let extracted = extract_title(&body);
        if !extracted.is_empty() {
            extracted
        } else {
            field("name")
                .or_else(|| field("description"))
                .unwrap_or(stem)
                .to_owned()
        }
    };
    let description = match field("description") {
        Some(value) => value.to_owned(),
        None => {
            let sentence = first_sentence(&body);
            if sentence.is_empty() {
                title.clone()
            } else {
                sentence
            }
        }
    };
    SourceDoc {
        rel_path: rel_path.to_owned(),
        stem: stem.to_owned(),
        frontmatter,
        body,
        title: title.trim().to_owned(),
        description: description.trim().to_owned(),
    }
}

/// Splits a `---` block into flat `key: value` pairs and the body. Values
/// lose one surrounding quote character; list-looking values are dropped.
pub fn split_frontmatter(content: &str) -> (Vec<(String, String)>, String) {
    let Split::Fenced(fence) = split_fence(content) else {
        return (Vec::new(), content.to_owned());
    };
    let mut metadata: Vec<(String, String)> = Vec::new();
    for raw_line in split_lines(fence.inner) {
        let Some((key, value)) = raw_line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let mut value = value.trim();
        if let Some(rest) = value.strip_prefix(['"', '\'']) {
            value = rest;
        }
        if let Some(rest) = value.strip_suffix(['"', '\'']) {
            value = rest;
        }
        if key.is_empty() || value.is_empty() || value.starts_with('[') {
            continue;
        }
        match metadata.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = value.to_owned(),
            None => metadata.push((key.to_owned(), value.to_owned())),
        }
    }
    (metadata, fence.body.trim_start().to_owned())
}

fn planned(destination: PathBuf, content: String, kind: &'static str, source: &str) -> Write {
    Write {
        destination,
        content,
        kind,
        source: source.to_owned(),
        confidence: "high",
        ambiguous_reason: String::new(),
    }
}

pub fn plan_skill(repo: &Path, doc: &SourceDoc) -> Vec<Write> {
    let source_system = detect_source_system(&doc.rel_path);
    let name = slugify(doc.field("name").unwrap_or(&doc.stem));
    let skill_dir = repo.join(".agents").join("skills").join(&name);
    let description = compact(&doc.description, 180);
    let display = display_name(&name);
    let body = if doc.body.is_empty() {
        format!("# {display}\n\n{description}")
    } else {
        doc.body.clone()
    };
    let skill_content = format!(
        "---\nname: {name}\ndescription: {}\ndisable-model-invocation: true\n---\n\n{MIGRATION_MARKER}\n<!-- source: {} -->\n<!-- source_system: {source_system} -->\n\n{}\n",
        quote_yaml(&description),
        doc.rel_path,
        normalize_heading(&body, &display)
    );
    let agent_content = format!(
        "# {MIGRATION_MARKER}\n# source: {}\n# source_system: {source_system}\ninterface:\n  display_name: {}\n  short_description: {}\n  default_prompt: {}\npolicy:\n  allow_implicit_invocation: false\n",
        doc.rel_path,
        quote_yaml(&display),
        quote_yaml(&compact(&description, 120)),
        quote_yaml(&format!("Use ${name} in this repository."))
    );
    vec![
        planned(
            skill_dir.join("SKILL.md"),
            skill_content,
            "skill",
            &doc.rel_path,
        ),
        planned(
            skill_dir.join("agents").join("openai.yaml"),
            agent_content,
            "skill-agent-metadata",
            &doc.rel_path,
        ),
    ]
}

pub fn plan_agent(repo: &Path, doc: &SourceDoc) -> Vec<Write> {
    let source_system = detect_source_system(&doc.rel_path);
    let id = slugify(doc.field("name").unwrap_or(&doc.stem));
    let today = local_date();
    let display = display_name(&id);
    let content = format!(
        "---\nid: {id}\ntitle: {}\ntype: agent\nstatus: active\ncreated: {today}\nupdated: {today}\ntags:\n  - imported\n  - {source_system}\n---\n\n{MIGRATION_MARKER}\n<!-- source: {} -->\n<!-- source_system: {source_system} -->\n\n{}\n",
        quote_yaml(&display),
        doc.rel_path,
        normalize_heading(&doc.body, &display)
    );
    vec![planned(
        repo.join(".agents").join("agents").join(format!("{id}.md")),
        content,
        "agent",
        &doc.rel_path,
    )]
}

/// Plans one memory file; its `confidence` and `ambiguous_reason` come from
/// the classifier so the caller can record ambiguity after the existence
/// filter.
pub fn plan_memory(repo: &Path, doc: &SourceDoc) -> Write {
    let source_system = detect_source_system(&doc.rel_path);
    let (category, confidence, reason) = classify_memory(doc);
    let id = slugify(doc.field("id").unwrap_or(&doc.stem));
    let today = local_date();
    let content = format!(
        "---\nid: {id}\ntitle: {}\ntype: {category}\nstatus: active\ncreated: {today}\nupdated: {today}\ntags:\n  - imported\n  - {source_system}\n  - {category}\nread_when:\n  - reviewing imported {source_system} context from {}\n---\n\n{MIGRATION_MARKER}\n<!-- source: {} -->\n<!-- source_system: {source_system} -->\n\n{}\n",
        quote_yaml(&doc.title),
        doc.rel_path,
        doc.rel_path,
        render_memory_body(doc, category, confidence, &reason)
    );
    let mut write = planned(
        repo.join(".agents")
            .join("memory")
            .join(category)
            .join(format!("{id}.md")),
        content,
        "memory",
        &doc.rel_path,
    );
    write.confidence = confidence;
    write.ambiguous_reason = reason;
    write
}

/// Scores keyword hits per category. Ties keep the first category in table
/// order and lower the confidence.
pub fn classify_memory(doc: &SourceDoc) -> (&'static str, &'static str, String) {
    let text = format!(
        "{}\n{}\n{}\n{}",
        doc.rel_path, doc.title, doc.description, doc.body
    )
    .to_lowercase();
    let count = |needles: &[&str]| -> usize {
        needles
            .iter()
            .map(|needle| text.matches(needle).count())
            .sum()
    };
    let scores = [
        (
            "architecture",
            count(&[
                "architecture",
                "structure",
                "routing",
                "component",
                "service",
                "system",
                "data flow",
            ]),
        ),
        (
            "decisions",
            count(&["adr", "decision", "decided", "rationale", "tradeoff"]),
        ),
        (
            "principles",
            count(&[
                "rule",
                "principle",
                "boundary",
                "must",
                "never",
                "always",
                "policy",
                "security",
            ]),
        ),
        (
            "operations",
            count(&[
                "command",
                "run ",
                "workflow",
                "deploy",
                "release",
                "maintenance",
                "debug",
                "incident",
            ]),
        ),
    ];
    let best = scores.iter().map(|(_, score)| *score).max().unwrap_or(0);
    let mut tied: Vec<&'static str> = scores
        .iter()
        .filter(|(_, score)| *score == best)
        .map(|(name, _)| *name)
        .collect();
    let category = tied[0];
    tied.sort_unstable();
    if best == 0 {
        return (
            "architecture",
            "low",
            "no strong category keywords found".to_owned(),
        );
    }
    if tied.len() > 1 {
        return (
            category,
            "medium",
            format!("category tie: {}", tied.join(", ")),
        );
    }
    (category, "high", String::new())
}

fn render_memory_body(doc: &SourceDoc, category: &str, confidence: &str, reason: &str) -> String {
    let mut lines = vec![
        format!("# {}", doc.title),
        String::new(),
        "## Digest".to_owned(),
        String::new(),
        format!("- Category: `{category}`"),
        format!("- Classification confidence: `{confidence}`"),
    ];
    if !reason.is_empty() {
        lines.push(format!("- Review note: {reason}"));
    }
    let paragraph = first_paragraph(&doc.body);
    let durable = if !paragraph.is_empty() {
        paragraph
    } else if !doc.description.is_empty() {
        doc.description.clone()
    } else {
        doc.title.clone()
    };
    let imported = if doc.body.trim().is_empty() {
        doc.description.clone()
    } else {
        doc.body.trim().to_owned()
    };
    lines.push(format!("- Source: `{}`", doc.rel_path));
    lines.push(String::new());
    lines.push("## Durable Memory".to_owned());
    lines.push(String::new());
    lines.push(compact(&durable, 360));
    lines.push(String::new());
    lines.push("## Imported Source".to_owned());
    lines.push(String::new());
    lines.push(imported);
    lines.join("\n")
}

fn plan_reports(repo: &Path, source: &Path, writes: &[Write], report: &Report) -> Vec<Write> {
    let report_dir = repo.join(".agents").join("migrations");
    let migrated: Vec<&Write> = report.migrated.iter().chain(writes.iter()).collect();
    let source_text = source.to_string_lossy().into_owned();
    vec![
        planned(
            report_dir.join("cursor-migration-report.md"),
            render_report_markdown(repo, source, &migrated, report),
            "migration-report",
            &source_text,
        ),
        planned(
            report_dir.join("cursor-migration-manifest.json"),
            render_manifest_json(repo, source, &migrated, report),
            "migration-manifest",
            &source_text,
        ),
    ]
}

fn sorted_by_destination<'a>(writes: &[&'a Write]) -> Vec<&'a Write> {
    let mut sorted: Vec<&Write> = writes.to_vec();
    sorted.sort_by(|a, b| {
        compare_names(
            &a.destination.to_string_lossy(),
            &b.destination.to_string_lossy(),
        )
    });
    sorted
}

fn render_report_markdown(
    repo: &Path,
    source: &Path,
    writes: &[&Write],
    report: &Report,
) -> String {
    let mut lines = vec![
        "# Cursor/Claude Migration Report".to_owned(),
        String::new(),
        MIGRATION_MARKER.to_owned(),
        String::new(),
        format!("- Source: `{}`", source.display()),
        format!("- Target: `{}`", repo.display()),
        format!("- Generated: `{}`", local_date()),
        format!("- Planned writes: `{}`", writes.len()),
        format!("- Skipped: `{}`", report.skipped.len()),
        format!("- Ambiguous: `{}`", report.ambiguous.len()),
        String::new(),
        "## Migrated".to_owned(),
        String::new(),
    ];
    if writes.is_empty() {
        lines.push("- None".to_owned());
    } else {
        for write in sorted_by_destination(writes) {
            lines.push(format!(
                "- `{}` `{}` from `{}`",
                write.kind,
                relative_posix(repo, &write.destination),
                write.source
            ));
        }
    }
    lines.extend([String::new(), "## Skipped".to_owned(), String::new()]);
    if report.skipped.is_empty() {
        lines.push("- None".to_owned());
    } else {
        for item in &report.skipped {
            lines.push(format!(
                "- `{}` from `{}`: {}",
                item.destination.display(),
                item.source,
                item.reason
            ));
        }
    }
    lines.extend([String::new(), "## Ambiguous".to_owned(), String::new()]);
    if report.ambiguous.is_empty() {
        lines.push("- None".to_owned());
    } else {
        for write in &report.ambiguous {
            let reason = if write.ambiguous_reason.is_empty() {
                "review recommended"
            } else {
                &write.ambiguous_reason
            };
            lines.push(format!(
                "- `{}` from `{}`: {reason}",
                relative_posix(repo, &write.destination),
                write.source
            ));
        }
    }
    lines.extend([
        String::new(),
        "## Maintenance Commands".to_owned(),
        String::new(),
    ]);
    if report.commands.is_empty() {
        lines.push("- Not run yet".to_owned());
    } else {
        for command in &report.commands {
            lines.push(format!(
                "- `{}` -> `{}`",
                command.argv.join(" "),
                command.returncode
            ));
        }
    }
    format!("{}\n", lines.join("\n"))
}

fn render_manifest_json(repo: &Path, source: &Path, writes: &[&Write], report: &Report) -> String {
    let value = json!({
        "ambiguous": report.ambiguous.iter().map(|write| json!({
            "destination": relative_posix(repo, &write.destination),
            "reason": write.ambiguous_reason,
            "source": write.source,
        })).collect::<Vec<_>>(),
        "commands": report.commands.iter().map(|command| json!({
            "argv": command.argv,
            "returncode": command.returncode,
            "stderr": command.stderr,
            "stdout": command.stdout,
        })).collect::<Vec<_>>(),
        "generated": local_date(),
        "migrated": sorted_by_destination(writes).iter().map(|write| json!({
            "ambiguous_reason": write.ambiguous_reason,
            "confidence": write.confidence,
            "destination": relative_posix(repo, &write.destination),
            "kind": write.kind,
            "source": write.source,
        })).collect::<Vec<_>>(),
        "migration_marker": MIGRATION_MARKER,
        "skipped": report.skipped.iter().map(|item| json!({
            "destination": item.destination.to_string_lossy(),
            "reason": item.reason,
            "source": item.source,
        })).collect::<Vec<_>>(),
        "source": source.to_string_lossy(),
        "target": repo.to_string_lossy(),
    });
    format!(
        "{}\n",
        serde_json::to_string_pretty(&value).expect("manifest JSON is always serializable")
    )
}

/// Keeps the first source that plans each destination in this run and
/// records every later one as skipped, so two documents that slugify to the
/// same id (for example `.cursor/commands/review.md` and
/// `.claude/commands/review.md`) never overwrite each other silently.
fn claim_destinations(
    writes: Vec<Write>,
    claimed: &mut BTreeMap<PathBuf, String>,
    report: &mut Report,
) -> Vec<Write> {
    let mut result = Vec::new();
    for write in writes {
        match claimed.get(&write.destination) {
            Some(first) => report.skipped.push(Skipped {
                destination: write.destination,
                source: write.source,
                reason: format!(
                    "conflicts with `{first}`, which already maps to this destination in this run; rename one source to migrate both"
                ),
            }),
            None => {
                claimed.insert(write.destination.clone(), write.source.clone());
                result.push(write);
            }
        }
    }
    result
}

fn filter_existing(writes: Vec<Write>, report: &mut Report, force: bool) -> Vec<Write> {
    let mut result = Vec::new();
    for write in writes {
        if fsx::exists(&write.destination) && !force && !is_migrated_file(&write.destination) {
            report.skipped.push(Skipped {
                destination: write.destination,
                source: write.source,
                reason:
                    "destination exists and is not migrator-owned; rerun with --force to overwrite"
                        .to_owned(),
            });
        } else {
            result.push(write);
        }
    }
    result
}

fn apply_writes(writes: &[Write], report: &mut Report, force: bool) -> Result<()> {
    for write in writes {
        if fsx::exists(&write.destination) && !force && !is_migrated_file(&write.destination) {
            report.skipped.push(Skipped {
                destination: write.destination.clone(),
                source: write.source.clone(),
                reason: "destination appeared before write and is not migrator-owned".to_owned(),
            });
            continue;
        }
        fsx::write_text(&write.destination, &write.content)?;
        report.migrated.push(write.clone());
    }
    Ok(())
}

/// Runs the maintenance commands through this same binary with captured
/// output, so the manifest keeps each command's exit code, stdout, and
/// stderr the way the replaced script recorded them. `skill sync` runs only
/// when the target has a skills root, since it fails without one and a
/// migration that imported no skills has nothing for it to do.
fn run_maintenance(repo: &Path, source: &Path, report: &mut Report) -> Result<()> {
    let binary = std::env::current_exe()?;
    let subcommands: Vec<[&str; 2]> = std::iter::once(["skill", "sync"])
        .filter(|_| fsx::is_dir(&skills_root(repo)))
        .chain([["ticket", "sync"], ["ticket", "validate"]])
        .collect();
    for subcommand in subcommands {
        let output = Command::new(&binary)
            .arg("--repo-root")
            .arg(repo)
            .args(subcommand)
            .current_dir(repo)
            .output();
        let (returncode, stdout, stderr) = match output {
            Ok(output) => (
                output.status.code().unwrap_or(1),
                String::from_utf8_lossy(&output.stdout).trim().to_owned(),
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ),
            Err(error) => (
                1,
                String::new(),
                format!("could not run gritt-agent: {error}"),
            ),
        };
        report.commands.push(CommandRun {
            argv: std::iter::once("gritt-agent")
                .chain(subcommand)
                .map(str::to_owned)
                .collect(),
            returncode,
            stdout,
            stderr,
        });
    }
    for write in plan_reports(repo, source, &[], report) {
        fsx::write_text(&write.destination, &write.content)?;
    }
    Ok(())
}

fn render_console_summary(writes: &[Write], report: &Report, dry_run: bool) -> String {
    let failed = report
        .commands
        .iter()
        .filter(|command| command.returncode != 0)
        .count();
    let mut lines = vec![
        format!(
            "cursor/claude migration {}",
            if dry_run { "planned" } else { "migrated" }
        ),
        format!("writes: {}", writes.len()),
        format!("skipped: {}", report.skipped.len()),
        format!("ambiguous: {}", report.ambiguous.len()),
    ];
    if failed > 0 {
        lines.push(format!("maintenance failures: {failed}"));
    }
    lines.join("\n")
}

pub fn is_migrated_file(target: &Path) -> bool {
    match fsx::read_text(target) {
        Ok(content) => {
            content.contains(MIGRATION_MARKER)
                || LEGACY_MARKERS.iter().any(|marker| content.contains(marker))
        }
        Err(_) => false,
    }
}

pub fn extract_title(body: &str) -> String {
    body.lines()
        .map(str::trim)
        .find(|line| line.starts_with('#'))
        .map(|line| line.trim_start_matches('#').trim().to_owned())
        .unwrap_or_default()
}

pub fn first_sentence(body: &str) -> String {
    let paragraph = first_paragraph(body);
    sentence_regex()
        .captures(&paragraph)
        .map(|captures| captures[1].to_owned())
        .unwrap_or(paragraph)
}

pub fn first_paragraph(body: &str) -> String {
    paragraph_split_regex()
        .split(body.trim())
        .map(str::trim)
        .find(|part| !part.is_empty() && !part.starts_with('#'))
        .map(|part| part.split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_default()
}

fn normalize_heading(body: &str, fallback_title: &str) -> String {
    let clean = body.trim();
    if clean.is_empty() {
        return format!("# {fallback_title}");
    }
    if clean.starts_with('#') {
        clean.to_owned()
    } else {
        format!("# {fallback_title}\n\n{clean}")
    }
}

pub fn compact(value: &str, limit: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= limit {
        return normalized;
    }
    let clipped: String = normalized.chars().take(limit.saturating_sub(3)).collect();
    format!("{}...", clipped.trim_end())
}

/// Kebab-case slug, `imported` when nothing is left.
pub fn slugify(value: &str) -> String {
    let slug = kebab_case(value);
    if slug.is_empty() {
        "imported".to_owned()
    } else {
        slug
    }
}

fn detect_source_system(rel_path: &str) -> &'static str {
    if rel_path.starts_with(".cursor/") {
        "cursor"
    } else if rel_path.starts_with(".claude/") {
        "claude"
    } else {
        "unknown"
    }
}

/// Canonical path when it exists, else the absolute path unchanged.
fn resolve_existing(target: &Path) -> PathBuf {
    target
        .canonicalize()
        .or_else(|_| std::path::absolute(target))
        .unwrap_or_else(|_| target.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_splits_and_strips_quotes() {
        let (meta, body) = split_frontmatter(
            "---\nname: \"Example\"\ndescription: 'Does it.'\ntags: [a, b]\nempty:\n---\n\n# Title\n\nBody.\n",
        );
        assert_eq!(
            meta,
            vec![
                ("name".to_owned(), "Example".to_owned()),
                ("description".to_owned(), "Does it.".to_owned())
            ]
        );
        assert_eq!(body, "# Title\n\nBody.\n");
        assert_eq!(split_frontmatter("plain").0, Vec::<(String, String)>::new());
        assert_eq!(split_frontmatter("---\nno close\n").1, "---\nno close\n");
    }

    #[test]
    fn frontmatter_accepts_crlf_and_whole_line_fences() {
        let (meta, body) = split_frontmatter("---\r\nname: win\r\n---\r\n\r\n# Win\r\n");
        assert_eq!(meta, vec![("name".to_owned(), "win".to_owned())]);
        assert_eq!(body, "# Win\r\n");
        let (meta, body) =
            split_frontmatter("---\nname: x\n----\ndescription: After.\n---\n\n----\n");
        assert_eq!(
            meta,
            vec![
                ("name".to_owned(), "x".to_owned()),
                ("description".to_owned(), "After.".to_owned())
            ]
        );
        assert_eq!(body, "----\n");
        let unclosed = "---\nname: x\n----\n";
        assert_eq!(
            split_frontmatter(unclosed),
            (Vec::new(), unclosed.to_owned())
        );
    }

    #[test]
    fn source_doc_derives_title_and_description() {
        let doc = load_source_doc(
            ".cursor/rules/x.mdc",
            "x",
            "Rules here.\n\nAlways run tests! Then ship.",
        );
        assert_eq!(doc.title, "x");
        assert_eq!(doc.description, "Rules here.");
        let doc = load_source_doc(".claude/skills/y.md", "y", "# Heading\n\ntext");
        assert_eq!(doc.title, "Heading");
        assert_eq!(doc.description, "text");
        let doc = load_source_doc("z.md", "z", "---\ndescription: From fm.\n---\n");
        assert_eq!(doc.title, "From fm.");
        assert_eq!(doc.description, "From fm.");
    }

    #[test]
    fn memory_classification_scores_ties_and_empties() {
        // The source path is scored too, so keep keywords out of it here.
        let doc = |body: &str| load_source_doc(".cursor/memory/r.md", "r", body);
        assert_eq!(
            classify_memory(&doc(
                "We decided this after a decision; the rationale was clear."
            )),
            ("decisions", "high", String::new())
        );
        assert_eq!(
            classify_memory(&doc("hello world")),
            (
                "architecture",
                "low",
                "no strong category keywords found".to_owned()
            )
        );
        let (category, confidence, reason) = classify_memory(&doc("one decision, one rule"));
        assert_eq!(category, "decisions");
        assert_eq!(confidence, "medium");
        assert_eq!(reason, "category tie: decisions, principles");
    }

    #[test]
    fn text_helpers_match_the_replaced_script() {
        assert_eq!(compact("  a   b  ", 10), "a b");
        assert_eq!(compact("abcdefghij", 8), "abcde...");
        assert_eq!(slugify("  My Rule!! "), "my-rule");
        assert_eq!(slugify("***"), "imported");
        assert_eq!(
            first_paragraph("# H\n\n\n  first  para\nline2\n\nsecond"),
            "first para line2"
        );
        assert_eq!(first_sentence("No stop here"), "No stop here");
        assert_eq!(normalize_heading("", "Fallback"), "# Fallback");
        assert_eq!(normalize_heading("text", "Fallback"), "# Fallback\n\ntext");
        assert_eq!(detect_source_system(".claude/agents/a.md"), "claude");
        assert_eq!(detect_source_system("other/a.md"), "unknown");
    }

    #[test]
    fn skill_plan_marks_ownership_and_renders_metadata() {
        let doc = load_source_doc(
            ".cursor/skills/example.md",
            "example",
            "---\nname: example\ndescription: Example imported skill.\n---\n\n# Example\n",
        );
        let writes = plan_skill(Path::new("/repo"), &doc);
        assert_eq!(writes.len(), 2);
        assert_eq!(
            writes[0].destination,
            Path::new("/repo/.agents/skills/example/SKILL.md")
        );
        assert!(writes[0].content.starts_with("---\nname: example\ndescription: \"Example imported skill.\"\ndisable-model-invocation: true\n---\n\n<!-- MIGRATED BY gritt-agent migrate cursor; DO NOT EDIT -->\n<!-- source: .cursor/skills/example.md -->\n<!-- source_system: cursor -->\n\n# Example\n"));
        assert_eq!(writes[1].kind, "skill-agent-metadata");
        assert!(writes[1].content.contains("  display_name: \"Example\"\n  short_description: \"Example imported skill.\"\n  default_prompt: \"Use $example in this repository.\"\n"));
    }
}
