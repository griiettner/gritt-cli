//! `gritt-agent codex trust`: checks or adds the `trust_level = "trusted"`
//! entry for a repository in Codex's `config.toml`.
//!
//! The file is edited line by line and only understands the
//! `[projects."<path>"]` table form Codex itself writes. When the same path
//! is keyed in another form (a literal-string header or an inline table
//! entry) the tool refuses to touch the file rather than append a duplicate
//! table, which would make the TOML invalid. Other paths, comments, and
//! values that merely contain the path text do not block the edit.

use std::env;
use std::path::{Path, PathBuf};

use crate::fsx;
use crate::repo::{expand_home, git_toplevel, home_dir};
use crate::{CliError, Result};

const TRUSTED_LINE: &str = "trust_level = \"trusted\"";

pub fn run(project: &Path, check: bool) -> Result<i32> {
    let project_path = resolve_project_root(project);
    let config_path = codex_config_path();
    if check {
        if is_trusted(&config_path, &project_path)? {
            println!("trusted: {}", project_path.display());
            return Ok(0);
        }
        println!("not trusted: {}", project_path.display());
        println!("config: {}", config_path.display());
        return Ok(1);
    }
    let changed = ensure_trusted(&config_path, &project_path)?;
    println!(
        "{}: {}",
        if changed {
            "trusted"
        } else {
            "already trusted"
        },
        project_path.display()
    );
    println!("config: {}", config_path.display());
    if changed {
        println!("restart required: start a fresh Codex session at this repository root");
    }
    Ok(0)
}

/// The git top level when `start` is inside a repository, else `start`
/// made absolute and lexically normalized without resolving symlinks, so
/// the TOML key matches the working directory Codex will look up.
fn resolve_project_root(start: &Path) -> PathBuf {
    let absolute = expand_home(start);
    git_toplevel(&absolute).unwrap_or(absolute)
}

pub fn codex_config_path() -> PathBuf {
    let codex_home = match env::var_os("CODEX_HOME").filter(|v| !v.is_empty()) {
        Some(value) => expand_home(Path::new(&value)),
        None => home_dir().join(".codex"),
    };
    codex_home.join("config.toml")
}

pub fn project_header(project_path: &Path) -> String {
    format!(
        "[projects.\"{}\"]",
        toml_basic_string(&project_path.to_string_lossy())
    )
}

fn toml_basic_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Returns the line range `(start, end)` of the table whose header line
/// equals `header`; `end` is the next header or the end of the file.
fn find_section(lines: &[String], header: &str) -> Option<(usize, usize)> {
    let start = lines.iter().position(|line| line.trim() == header)?;
    let end = lines[start + 1..]
        .iter()
        .position(|line| {
            let stripped = line.trim();
            stripped.starts_with('[') && stripped.ends_with(']')
        })
        .map(|offset| start + 1 + offset)
        .unwrap_or(lines.len());
    Some((start, end))
}

fn section_is_trusted(lines: &[String], (start, end): (usize, usize)) -> bool {
    lines[start + 1..end]
        .iter()
        .any(|line| line.trim() == TRUSTED_LINE)
}

fn split_config(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_owned())
        .collect();
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

/// True when `line` keys the project in a form this tool does not edit: a
/// literal-string table header, or an inline `"<path>" = { ... }` entry.
fn keys_project_in_another_form(line: &str, project: &str) -> bool {
    let trimmed = line.trim();
    if trimmed == format!("[projects.'{project}']") {
        return true;
    }
    for key in [
        format!("\"{}\"", toml_basic_string(project)),
        format!("'{project}'"),
    ] {
        if let Some(rest) = trimmed.strip_prefix(key.as_str()) {
            if rest.trim_start().starts_with('=') {
                return true;
            }
        }
    }
    false
}

/// Returns the updated config text, or `None` when the project is already
/// trusted and nothing needs to change. Errors when the path is keyed in a
/// form this tool does not edit, so it never appends a duplicate table.
pub fn apply_trust(text: &str, project_path: &Path) -> Result<Option<String>> {
    let header = project_header(project_path);
    let project = project_path.to_string_lossy();
    let mut lines = split_config(text);
    match find_section(&lines, &header) {
        None => {
            if let Some(line) = lines
                .iter()
                .find(|line| keys_project_in_another_form(line, &project))
            {
                return Err(CliError::new(format!(
                    "config.toml already mentions this project in a form this tool does not edit (`{}`); set trust_level = \"trusted\" there by hand",
                    line.trim()
                )));
            }
            if lines.last().is_some_and(|line| !line.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(header);
            lines.push(TRUSTED_LINE.to_owned());
        }
        Some(section) if section_is_trusted(&lines, section) => return Ok(None),
        Some((start, end)) => {
            let existing = lines[start + 1..end]
                .iter()
                .position(|line| line.trim().starts_with("trust_level"))
                .map(|offset| start + 1 + offset);
            match existing {
                Some(index) => lines[index] = TRUSTED_LINE.to_owned(),
                None => lines.insert(start + 1, TRUSTED_LINE.to_owned()),
            }
        }
    }
    Ok(Some(format!("{}\n", lines.join("\n"))))
}

fn ensure_trusted(config_path: &Path, project_path: &Path) -> Result<bool> {
    let text = fsx::read_text_or(config_path, "")?;
    match apply_trust(&text, project_path)? {
        Some(updated) => {
            fsx::write_text(config_path, &updated)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Trusted exactly when `apply_trust` would change nothing.
fn is_trusted(config_path: &Path, project_path: &Path) -> Result<bool> {
    let text = fsx::read_text_or(config_path, "")?;
    Ok(apply_trust(&text, project_path)?.is_none())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROJECT: &str = "/work/repo";

    fn trust(text: &str) -> Result<Option<String>> {
        apply_trust(text, Path::new(PROJECT))
    }

    #[test]
    fn appends_a_new_section_after_a_blank_line() {
        assert_eq!(
            trust("").unwrap().unwrap(),
            "[projects.\"/work/repo\"]\ntrust_level = \"trusted\"\n"
        );
        assert_eq!(
            trust("model = \"x\"\n").unwrap().unwrap(),
            "model = \"x\"\n\n[projects.\"/work/repo\"]\ntrust_level = \"trusted\"\n"
        );
    }

    #[test]
    fn upgrades_or_inserts_trust_level_inside_an_existing_section() {
        let untrusted =
            "[projects.\"/work/repo\"]\ntrust_level = \"untrusted\"\n\n[other]\nk = 1\n";
        assert_eq!(
            trust(untrusted).unwrap().unwrap(),
            "[projects.\"/work/repo\"]\ntrust_level = \"trusted\"\n\n[other]\nk = 1\n"
        );
        let missing_key = "[projects.\"/work/repo\"]\nnote = 1\n";
        assert_eq!(
            trust(missing_key).unwrap().unwrap(),
            "[projects.\"/work/repo\"]\ntrust_level = \"trusted\"\nnote = 1\n"
        );
        assert_eq!(
            trust("[projects.\"/work/repo\"]\ntrust_level = \"trusted\"\n").unwrap(),
            None
        );
    }

    #[test]
    fn refuses_to_duplicate_a_project_written_in_another_form() {
        let inline = "[projects]\n\"/work/repo\" = { trust_level = \"untrusted\" }\n";
        let error = trust(inline).unwrap_err();
        assert!(error.message.contains("form this tool does not edit"));
        assert!(error.message.contains("\"/work/repo\" = { trust_level"));
        let literal = "[projects.'/work/repo']\ntrust_level = \"untrusted\"\n";
        assert!(trust(literal).is_err());
    }

    #[test]
    fn other_paths_and_comments_do_not_block_the_append() {
        let config = "# see /work/repo for details\n\n[projects.\"/work/repo-old\"]\ntrust_level = \"trusted\"\n\n[projects.\"/work/repo/sub\"]\ntrust_level = \"trusted\"\nnotes = \"/work/repo\"\n";
        let updated = trust(config).unwrap().unwrap();
        assert!(updated.ends_with(
            "notes = \"/work/repo\"\n\n[projects.\"/work/repo\"]\ntrust_level = \"trusted\"\n"
        ));
        assert!(!keys_project_in_another_form(
            "\"/work/repo-old\" = {}",
            PROJECT
        ));
        assert!(keys_project_in_another_form(
            "  '/work/repo'   = { trust_level = \"x\" }",
            PROJECT
        ));
    }

    #[test]
    fn header_escapes_quotes_and_backslashes() {
        assert_eq!(
            project_header(Path::new("C:\\w\\my \"repo\"")),
            "[projects.\"C:\\\\w\\\\my \\\"repo\\\"\"]"
        );
    }
}
