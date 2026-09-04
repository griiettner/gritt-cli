//! `gritt-agent skill new`: scaffolds a canonical skill and its Codex
//! metadata, then refreshes the generated adapters in-process.

use std::path::Path;

use super::sync::{self, display_name, first_sentence, quote_yaml, SyncOptions};
use crate::fsx::{self, kebab_case, relative_posix};
use crate::repo::skills_root;
use crate::{CliError, Result};

const MAX_NAME_LENGTH: usize = 64;

#[derive(Debug, Clone, Default)]
pub struct NewOptions {
    pub name: String,
    pub description: String,
    pub title: Option<String>,
    pub force: bool,
    pub no_openai: bool,
    pub no_sync: bool,
    pub dry_run: bool,
}

pub fn run(repo: &Path, options: &NewOptions) -> Result<i32> {
    let name = normalize_name(&options.name);
    validate_name(&name)?;
    let skill_dir = skills_root(repo).join(&name);
    let skill_file = skill_dir.join("SKILL.md");
    let agent_file = skill_dir.join("agents").join("openai.yaml");
    if fsx::exists(&skill_file) && !options.force {
        return Err(CliError::new(format!(
            "skill already exists: {}",
            skill_file.display()
        )));
    }
    // An explicit --title is used verbatim for both. The default heading is
    // sentence case, as `skill-management/audit` expects, while the Codex
    // display name keeps the Title Case `skill sync` would generate.
    let heading = options
        .title
        .clone()
        .unwrap_or_else(|| sentence_case(&name));
    let display = options.title.clone().unwrap_or_else(|| display_name(&name));
    if options.dry_run {
        println!("would create skill: {}", relative_posix(repo, &skill_file));
        if !options.no_openai {
            println!(
                "would create Codex metadata: {}",
                relative_posix(repo, &agent_file)
            );
        }
        if !options.no_sync {
            println!("would run: gritt-agent skill sync");
        }
        return Ok(0);
    }

    fsx::write_text(
        &skill_file,
        &render_skill(&name, &options.description, &heading),
    )?;
    if !options.no_openai {
        fsx::write_text(
            &agent_file,
            &render_openai_yaml(&name, &options.description, &display),
        )?;
    }
    let summary = if options.no_sync {
        None
    } else {
        Some(sync::sync(repo, SyncOptions::default())?)
    };
    println!("created skill: .agents/skills/{name}/SKILL.md");
    if !options.no_openai {
        println!("created Codex metadata: .agents/skills/{name}/agents/openai.yaml");
    }
    match summary {
        Some(summary) => {
            summary.print();
            Ok(summary.exit_code())
        }
        None => Ok(0),
    }
}

pub fn normalize_name(raw: &str) -> String {
    kebab_case(raw)
}

/// Turns `tkt-new` into `Tkt new`: the first word capitalised, the rest as
/// typed, which is the heading case the skill audit expects.
pub fn sentence_case(slug: &str) -> String {
    let mut chars = slug.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().replace('-', " "),
        None => String::new(),
    }
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(CliError::new(
            "skill name must contain at least one letter or digit",
        ));
    }
    if name.len() > MAX_NAME_LENGTH {
        return Err(CliError::new(format!(
            "skill name must be {MAX_NAME_LENGTH} characters or fewer"
        )));
    }
    Ok(())
}

pub fn render_skill(name: &str, description: &str, title: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: {}\ndisable-model-invocation: true\n---\n\n# {title}\n\n## Purpose\n\n{description}\n\n## Workflow\n\n1. Read `AGENTS.md`.\n2. Gather only the context needed for the requested work.\n3. Follow this skill's workflow and keep changes scoped.\n4. Run relevant validation before reporting completion.\n\n## Output\n\n- State what changed.\n- Report validation performed.\n- Call out unresolved follow-up or risk.\n",
        quote_yaml(description)
    )
}

pub fn render_openai_yaml(name: &str, description: &str, title: &str) -> String {
    format!(
        "interface:\n  display_name: {}\n  short_description: {}\n  default_prompt: {}\npolicy:\n  allow_implicit_invocation: false\n",
        quote_yaml(title),
        quote_yaml(&short_description(description)),
        quote_yaml(&format!("Use ${name} in this repository."))
    )
}

/// The whole description when it fits in 120 characters, else the same
/// first-sentence-or-clip rule `skill sync` uses for a missing interface.
/// `skill sync` keeps whatever value the file already has afterwards.
fn short_description(description: &str) -> String {
    let normalized = description.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= 120 {
        return normalized;
    }
    first_sentence(&normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_normalize_to_kebab_case() {
        assert_eq!(normalize_name("  My Skill!! v2 "), "my-skill-v2");
        assert_eq!(sentence_case("my-skill-v2"), "My skill v2");
        assert_eq!(sentence_case(""), "");
        assert!(validate_name("").is_err());
        assert!(validate_name(&"a".repeat(65)).is_err());
        assert!(validate_name(&"a".repeat(64)).is_ok());
    }

    #[test]
    fn short_description_keeps_whole_text_when_it_fits() {
        assert_eq!(
            short_description("Does a thing. Use when asked."),
            "Does a thing. Use when asked."
        );
        let long = format!("First sentence here. {}", "word ".repeat(40));
        assert_eq!(short_description(&long), "First sentence here.");
        let no_stop = "word ".repeat(40);
        let clipped = short_description(&no_stop);
        assert!(clipped.ends_with("..."));
        assert!(clipped.len() <= 120);
    }

    #[test]
    fn rendered_files_round_trip_through_sync_parsers() {
        let skill = render_skill(
            "sample-skill",
            "Sample \"quoted\" description.",
            "Sample skill",
        );
        let parsed = sync::parse_skill_frontmatter(&skill);
        assert_eq!(parsed.get("name").map(String::as_str), Some("sample-skill"));
        assert_eq!(
            parsed.get("description").map(String::as_str),
            Some("Sample \"quoted\" description.")
        );
        assert!(skill.contains("# Sample skill\n\n## Purpose\n\nSample \"quoted\" description.\n"));
        let yaml = render_openai_yaml("sample-skill", "Sample description.", "Sample Skill");
        assert_eq!(
            yaml,
            "interface:\n  display_name: \"Sample Skill\"\n  short_description: \"Sample description.\"\n  default_prompt: \"Use $sample-skill in this repository.\"\npolicy:\n  allow_implicit_invocation: false\n"
        );
    }
}
