//! Ticket ids, namespaces, and chunk folder resolution (ADR-003).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;

use crate::fsx;
use crate::{CliError, Result};

pub const SHARED_NAMESPACE: &str = "_shared";
pub const TASK_SHARD_SIZE: u32 = 25;

fn namespace_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^(?:_shared|[A-Za-z0-9](?:[A-Za-z0-9._-]{0,37}[A-Za-z0-9])?)$").unwrap()
    })
}

fn qualified_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^([^/]+)/(TKT-\d{4})$").unwrap())
}

fn is_four_digits(text: &str) -> bool {
    text.len() == 4 && text.bytes().all(|b| b.is_ascii_digit())
}

pub fn pad_ticket_number(value: u32) -> String {
    format!("{value:04}")
}

pub fn is_ticket_id(value: &str) -> bool {
    value.strip_prefix("TKT-").is_some_and(is_four_digits)
}

pub fn is_chunk_dir_name(value: &str) -> bool {
    match value.strip_prefix("TKT-") {
        Some(rest) => match rest.split_once('-') {
            Some((start, end)) => is_four_digits(start) && is_four_digits(end),
            None => false,
        },
        None => false,
    }
}

pub fn is_namespace_name(value: &str) -> bool {
    namespace_regex().is_match(value) && !is_chunk_dir_name(value) && value != "." && value != ".."
}

pub fn ticket_number(ticket_id: &str) -> u32 {
    ticket_id
        .split('-')
        .nth(1)
        .and_then(|digits| digits.parse().ok())
        .unwrap_or(0)
}

pub fn chunk_start(number: u32) -> u32 {
    (number.saturating_sub(1) / TASK_SHARD_SIZE) * TASK_SHARD_SIZE + 1
}

pub fn chunk_name(number: u32) -> String {
    let start = chunk_start(number);
    format!(
        "TKT-{}-{}",
        pad_ticket_number(start),
        pad_ticket_number(start + TASK_SHARD_SIZE - 1)
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketRef {
    pub namespace: Option<String>,
    pub ticket_id: String,
}

pub fn parse_ticket_ref(value: &str) -> Option<TicketRef> {
    let raw = value.trim();
    if let Some(captures) = qualified_regex().captures(raw) {
        return Some(TicketRef {
            namespace: Some(captures[1].to_owned()),
            ticket_id: captures[2].to_owned(),
        });
    }
    if is_ticket_id(raw) {
        return Some(TicketRef {
            namespace: None,
            ticket_id: raw.to_owned(),
        });
    }
    None
}

pub fn qualified_ticket_id(namespace: &str, ticket_id: &str) -> String {
    format!("{namespace}/{ticket_id}")
}

pub fn namespace_root(tasks_root: &Path, namespace: &str) -> PathBuf {
    if namespace == SHARED_NAMESPACE {
        tasks_root.to_path_buf()
    } else {
        tasks_root.join(namespace)
    }
}

pub fn ticket_dir(tasks_root: &Path, namespace: &str, ticket_id: &str) -> PathBuf {
    namespace_root(tasks_root, namespace)
        .join(chunk_name(ticket_number(ticket_id)))
        .join(ticket_id)
}

#[derive(Debug, Clone)]
pub struct Namespace {
    pub id: String,
    pub root: PathBuf,
}

/// Lists the shared namespace first, then every developer namespace folder.
pub fn list_namespaces(tasks_root: &Path) -> Result<Vec<Namespace>> {
    let mut namespaces = vec![Namespace {
        id: SHARED_NAMESPACE.to_owned(),
        root: tasks_root.to_path_buf(),
    }];
    if !fsx::is_dir(tasks_root) {
        return Ok(namespaces);
    }
    for entry in fsx::list_dirs(tasks_root)? {
        if is_chunk_dir_name(&entry.name) || !is_namespace_name(&entry.name) {
            continue;
        }
        namespaces.push(Namespace {
            id: entry.name.clone(),
            root: entry.path,
        });
    }
    Ok(namespaces)
}

#[derive(Debug, Clone)]
pub struct TicketDir {
    pub dir: PathBuf,
    pub ticket_id: String,
    pub namespace: String,
}

pub fn iter_ticket_dirs(tasks_root: &Path) -> Result<Vec<TicketDir>> {
    let mut result = Vec::new();
    for namespace in list_namespaces(tasks_root)? {
        if !fsx::is_dir(&namespace.root) {
            continue;
        }
        for chunk in fsx::list_dirs(&namespace.root)? {
            if !is_chunk_dir_name(&chunk.name) {
                continue;
            }
            for ticket in fsx::list_dirs(&chunk.path)? {
                if !is_ticket_id(&ticket.name) {
                    continue;
                }
                result.push(TicketDir {
                    dir: ticket.path,
                    ticket_id: ticket.name,
                    namespace: namespace.id.clone(),
                });
            }
        }
    }
    Ok(result)
}

/// Returns the next contiguous ticket number in a namespace. Fails when an
/// earlier id is missing so gaps are never papered over.
pub fn next_ticket_number(tasks_root: &Path, namespace: &str) -> Result<u32> {
    let root = namespace_root(tasks_root, namespace);
    if !fsx::is_dir(&root) {
        return Ok(1);
    }
    let mut numbers = BTreeSet::new();
    for chunk in fsx::list_dirs(&root)? {
        if !is_chunk_dir_name(&chunk.name) {
            continue;
        }
        for ticket in fsx::list_dirs(&chunk.path)? {
            if is_ticket_id(&ticket.name) {
                numbers.insert(ticket_number(&ticket.name));
            }
        }
    }
    let highest = numbers.iter().copied().max().unwrap_or(0);
    let missing: Vec<String> = (1..highest)
        .filter(|number| !numbers.contains(number))
        .map(|number| format!("TKT-{}", pad_ticket_number(number)))
        .collect();
    if !missing.is_empty() {
        return Err(CliError::new(format!(
            "ticket sequence has missing ids in namespace {namespace}: {}; restore or explicitly account for the missing ticket before allocating another id",
            missing.join(", ")
        )));
    }
    Ok(highest + 1)
}

/// Resolves a bare or qualified ticket reference to its folder.
///
/// A bare id prefers `preferred_namespace`, then a unique match across every
/// namespace. Ambiguity and missing folders are usage errors so callers exit
/// with code 2, as the replaced script did.
pub fn find_ticket_dir(
    tasks_root: &Path,
    ticket_ref: &str,
    preferred_namespace: Option<&str>,
) -> Result<TicketDir> {
    find_ticket_dir_with(tasks_root, ticket_ref, || {
        preferred_namespace.map(str::to_owned)
    })
}

/// Like [`find_ticket_dir`], but only asks for the preferred namespace when a
/// bare id matches more than one namespace, so callers whose lookup may be
/// expensive (a `gh` call) pay for it only when it can change the answer.
pub fn find_ticket_dir_with(
    tasks_root: &Path,
    ticket_ref: &str,
    preferred_namespace: impl FnOnce() -> Option<String>,
) -> Result<TicketDir> {
    let Some(parsed) = parse_ticket_ref(ticket_ref) else {
        return Err(CliError::usage(format!("invalid ticket id: {ticket_ref}")));
    };
    if let Some(namespace) = parsed.namespace {
        if !is_namespace_name(&namespace) {
            return Err(CliError::usage(format!(
                "invalid ticket namespace: {namespace}"
            )));
        }
        let dir = ticket_dir(tasks_root, &namespace, &parsed.ticket_id);
        if !fsx::is_dir(&dir) {
            return Err(CliError::usage(format!(
                "ticket folder does not exist: {namespace}/{}",
                parsed.ticket_id
            )));
        }
        return Ok(TicketDir {
            dir,
            ticket_id: parsed.ticket_id,
            namespace,
        });
    }

    let mut matches = Vec::new();
    for namespace in list_namespaces(tasks_root)? {
        let dir = ticket_dir(tasks_root, &namespace.id, &parsed.ticket_id);
        if fsx::is_dir(&dir) {
            matches.push(TicketDir {
                dir,
                ticket_id: parsed.ticket_id.clone(),
                namespace: namespace.id,
            });
        }
    }
    if matches.len() > 1 {
        if let Some(preferred) = preferred_namespace() {
            if let Some(index) = matches.iter().position(|m| m.namespace == preferred) {
                return Ok(matches.swap_remove(index));
            }
        }
    }
    match matches.len() {
        1 => Ok(matches.pop().unwrap()),
        0 => Err(CliError::usage(format!(
            "ticket folder does not exist: {}",
            parsed.ticket_id
        ))),
        _ => {
            let options: Vec<String> = matches
                .iter()
                .map(|m| qualified_ticket_id(&m.namespace, &m.ticket_id))
                .collect();
            Err(CliError::usage(format!(
                "ambiguous ticket id {}; use one of: {}",
                parsed.ticket_id,
                options.join(", ")
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_names_follow_adr_003() {
        assert_eq!(chunk_name(1), "TKT-0001-0025");
        assert_eq!(chunk_name(25), "TKT-0001-0025");
        assert_eq!(chunk_name(26), "TKT-0026-0050");
        assert_eq!(chunk_name(0), "TKT-0001-0025");
    }

    #[test]
    fn recognises_ids_and_namespaces() {
        assert!(is_ticket_id("TKT-0019"));
        assert!(!is_ticket_id("TKT-019"));
        assert!(is_chunk_dir_name("TKT-0001-0025"));
        assert!(is_namespace_name("griiettner"));
        assert!(is_namespace_name("_shared"));
        assert!(is_namespace_name("a"));
        assert!(!is_namespace_name("-bad"));
        assert!(!is_namespace_name("TKT-0001-0025"));
        assert!(!is_namespace_name(".."));
    }

    #[test]
    fn parses_refs() {
        assert_eq!(
            parse_ticket_ref("alice/TKT-0002"),
            Some(TicketRef {
                namespace: Some("alice".to_owned()),
                ticket_id: "TKT-0002".to_owned()
            })
        );
        assert_eq!(parse_ticket_ref("TKT-0002").unwrap().namespace, None);
        assert_eq!(parse_ticket_ref("nope"), None);
    }
}
