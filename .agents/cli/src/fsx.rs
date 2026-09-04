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
fn compare_names(left: &str, right: &str) -> Ordering {
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
