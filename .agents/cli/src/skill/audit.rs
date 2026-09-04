//! Read-only quality checks for canonical project skills.

use std::path::{Path, PathBuf};

use crate::fsx;
use crate::repo::skills_root;
use crate::skill::sync::parse_skill_frontmatter;
use crate::{CliError, Result};

#[derive(Debug, Clone, Default)]
pub struct AuditOptions {
    pub skill: Option<String>,
    pub strict: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone)]
struct Finding {
    severity: Severity,
    path: PathBuf,
    message: String,
}

pub fn run(repo: &Path, options: &AuditOptions) -> Result<i32> {
    let root = skills_root(repo);
    if !fsx::is_dir(&root) {
        return Err(CliError::new(format!(
            "missing canonical skills root: {}",
            root.display()
        )));
    }

    let dirs = if let Some(name) = &options.skill {
        let dir = root.join(name);
        if !fsx::is_dir(&dir) {
            return Err(CliError::new(format!("skill not found: {name}")));
        }
        vec![dir]
    } else {
        fsx::list_dirs(&root)?
            .into_iter()
            .filter(|entry| fsx::exists(&entry.path.join("SKILL.md")))
            .map(|entry| entry.path)
            .collect()
    };

    let audited = dirs.len();
    let mut findings = Vec::new();
    for dir in dirs {
        audit_skill(repo, &dir, &mut findings)?;
    }

    for finding in &findings {
        let level = match finding.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        println!(
            "{level}: {}: {}",
            fsx::relative_posix(repo, &finding.path),
            finding.message
        );
    }

    let errors = findings
        .iter()
        .filter(|finding| finding.severity == Severity::Error)
        .count();
    let warnings = findings
        .iter()
        .filter(|finding| finding.severity == Severity::Warning)
        .count();
    if errors == 0 && (!options.strict || warnings == 0) {
        println!(
            "skill_audit ok ({} skill(s), {} warning(s))",
            audited, warnings
        );
        return Ok(0);
    }
    eprintln!("skill_audit failed ({errors} error(s), {warnings} warning(s))");
    Ok(1)
}

fn audit_skill(_repo: &Path, dir: &Path, findings: &mut Vec<Finding>) -> Result<()> {
    let skill_file = dir.join("SKILL.md");
    let content = fsx::read_text(&skill_file)?;
    let metadata = parse_skill_frontmatter(&content);
    let name = dir
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("");

    match metadata.get("name") {
        Some(value) if value == name => {}
        Some(value) => findings.push(Finding {
            severity: Severity::Error,
            path: skill_file.clone(),
            message: format!("frontmatter name `{value}` does not match directory `{name}`"),
        }),
        None => findings.push(Finding {
            severity: Severity::Error,
            path: skill_file.clone(),
            message: "missing frontmatter name".to_owned(),
        }),
    }
    match metadata.get("description") {
        Some(value) if !value.trim().is_empty() => {}
        _ => findings.push(Finding {
            severity: Severity::Error,
            path: skill_file.clone(),
            message: "missing frontmatter description".to_owned(),
        }),
    }

    if !content.contains("## Output") && !content.contains("## Verification") {
        findings.push(Finding {
            severity: Severity::Warning,
            path: skill_file.clone(),
            message: "add an explicit ## Output or ## Verification contract".to_owned(),
        });
    }
    for reference in markdown_references(&content) {
        if reference.starts_with('#') || reference.contains("://") {
            continue;
        }
        let path = dir.join(&reference);
        if !path.exists() {
            findings.push(Finding {
                severity: Severity::Error,
                path: skill_file.clone(),
                message: format!("local reference does not exist: {reference}"),
            });
        }
    }
    Ok(())
}

fn markdown_references(content: &str) -> Vec<String> {
    content
        .split("](")
        .skip(1)
        .filter_map(|part| part.split(')').next())
        .map(|value| value.split_whitespace().next().unwrap_or(value))
        .filter(|value| !value.is_empty())
        .filter(|value| !value.starts_with('#') && !value.contains("://"))
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_references_ignore_urls_and_anchors() {
        let refs = markdown_references("[local](references/a.md) [web](https://x.test) [part](#x)");
        assert_eq!(refs, vec!["references/a.md"]);
    }
}
