//! Small filesystem helpers shared by every command.

use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};

use crate::Result;

pub fn exists(target: &Path) -> bool {
    target.exists()
}

pub fn is_dir(target: &Path) -> bool {
    target.is_dir()
}

/// One directory entry with its name already decoded.
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_file: bool,
    pub is_symlink: bool,
}

/// Orders names the way the replaced scripts did: case-insensitively first,
/// then by bytes so the order is still total.
pub fn compare_names(left: &str, right: &str) -> Ordering {
    left.to_lowercase()
        .cmp(&right.to_lowercase())
        .then_with(|| left.cmp(right))
}

/// Lists a directory sorted by name. Entries whose names are not valid UTF-8
/// are skipped because no repository file is addressed that way.
pub fn list_entries(dir: &Path) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let file_type = entry.file_type()?;
        let path = entry.path();
        let is_symlink = file_type.is_symlink();
        let (is_dir, is_file) = if is_symlink {
            let meta = fs::metadata(&path).ok();
            (
                meta.as_ref().is_some_and(|m| m.is_dir()),
                meta.as_ref().is_some_and(|m| m.is_file()),
            )
        } else {
            (file_type.is_dir(), file_type.is_file())
        };
        entries.push(Entry {
            name,
            path,
            is_dir,
            is_file,
            is_symlink,
        });
    }
    entries.sort_by(|a, b| compare_names(&a.name, &b.name));
    Ok(entries)
}

pub fn list_dirs(dir: &Path) -> Result<Vec<Entry>> {
    Ok(list_entries(dir)?
        .into_iter()
        .filter(|e| e.is_dir && !e.is_symlink)
        .collect())
}

pub fn read_text(target: &Path) -> Result<String> {
    Ok(fs::read_to_string(target)?)
}

/// Reads a file, returning `fallback` when it does not exist.
pub fn read_text_or(target: &Path, fallback: &str) -> Result<String> {
    match fs::read_to_string(target) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(fallback.to_owned()),
        Err(error) => Err(error.into()),
    }
}

pub fn write_text(target: &Path, content: &str) -> Result<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(target, content)?;
    Ok(())
}

pub fn remove_file(target: &Path) -> Result<()> {
    fs::remove_file(target)?;
    Ok(())
}

pub fn remove_dir_all(target: &Path) -> Result<()> {
    if target.exists() {
        fs::remove_dir_all(target)?;
    }
    Ok(())
}

/// Renders `target` relative to `root` with forward slashes. Paths outside
/// the root are returned unchanged, as the replaced scripts did.
pub fn relative_posix(root: &Path, target: &Path) -> String {
    match target.strip_prefix(root) {
        Ok(relative) => relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
        Err(_) => target.to_string_lossy().into_owned(),
    }
}

/// Renders the path from directory `from` to `target` with `..` segments and
/// forward slashes, like Node's `path.relative`. Both paths should be
/// absolute or share the same base.
pub fn relative_path_posix(from: &Path, target: &Path) -> String {
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = target.components().collect();
    let common = from
        .iter()
        .zip(to.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let mut parts: Vec<String> = vec!["..".to_owned(); from.len() - common];
    parts.extend(
        to[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    parts.join("/")
}

/// Resolves `.` and `..` segments lexically and drops a trailing separator,
/// like Node's `path.resolve`, without touching the filesystem.
pub fn normalize_lexical(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !matches!(
                    result.components().next_back(),
                    None | Some(Component::RootDir) | Some(Component::Prefix(_))
                ) {
                    result.pop();
                }
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}

/// Lowercases `value` and collapses every run of non-alphanumerics into one
/// hyphen, trimming the ends. Returns an empty string when nothing is left;
/// callers choose their own fallback.
pub fn kebab_case(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut pending = false;
    for ch in value.trim().to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            if pending && !result.is_empty() {
                result.push('-');
            }
            pending = false;
            result.push(ch);
        } else {
            pending = true;
        }
    }
    result
}

/// Lists every regular file under `dir` recursively, sorted by full path.
/// Symlinks are skipped so a migration never reads outside its source tree.
pub fn list_files_recursive(dir: &Path) -> Result<Vec<PathBuf>> {
    fn walk(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
        for entry in list_entries(dir)? {
            if entry.is_symlink {
                continue;
            }
            if entry.is_dir {
                walk(&entry.path, files)?;
            } else if entry.is_file {
                files.push(entry.path);
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    if is_dir(dir) {
        walk(dir, &mut files)?;
        files.sort();
    }
    Ok(files)
}

/// Reads a file as UTF-8, replacing invalid sequences instead of failing, the
/// way Node's `readFile(..., 'utf8')` did for imported documents.
pub fn read_text_lossy(target: &Path) -> Result<String> {
    Ok(String::from_utf8_lossy(&fs::read(target)?).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexical_normalization_matches_path_resolve() {
        assert_eq!(
            normalize_lexical(Path::new("/a/b/../c/./d/")),
            PathBuf::from("/a/c/d")
        );
        assert_eq!(normalize_lexical(Path::new("/../x")), PathBuf::from("/x"));
        assert_eq!(normalize_lexical(Path::new("/a/b")), PathBuf::from("/a/b"));
    }

    #[test]
    fn kebab_case_collapses_and_trims() {
        assert_eq!(kebab_case("  My Skill!! v2 "), "my-skill-v2");
        assert_eq!(kebab_case("--already-kebab--"), "already-kebab");
        assert_eq!(kebab_case("???"), "");
    }

    #[test]
    fn relative_paths_walk_up_and_down() {
        let from = Path::new("/r/.agents/tasks/a/TKT-0001-0025/TKT-0025");
        assert_eq!(
            relative_path_posix(
                from,
                Path::new("/r/.agents/tasks/a/TKT-0001-0025/TKT-0002/task.md")
            ),
            "../TKT-0002/task.md"
        );
        assert_eq!(
            relative_path_posix(
                from,
                Path::new("/r/.agents/tasks/a/TKT-0026-0050/TKT-0026/task.md")
            ),
            "../../TKT-0026-0050/TKT-0026/task.md"
        );
    }
}
