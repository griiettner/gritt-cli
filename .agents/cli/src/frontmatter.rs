//! Parser for the restricted YAML frontmatter used by ticket and memory files.
//!
//! Only scalar `key: value` lines and scalar lists are supported. Anything
//! else is reported as an error with the line number so sync and validate
//! can surface it.

use std::collections::BTreeMap;
use std::path::Path;

use crate::fsx;

pub const LIST_FIELDS: &[&str] = &[
    "dependencies",
    "areas",
    "skills",
    "tags",
    "read_when",
    "chain_children",
];

pub const SCALAR_FIELDS: &[&str] = &[
    "id",
    "title",
    "type",
    "artifact",
    "status",
    "date",
    "related_ticket",
    "owner",
    "namespace",
    "priority",
    "created",
    "updated",
    "chain_role",
    "chain_parent",
];

pub fn is_list_field(key: &str) -> bool {
    LIST_FIELDS.contains(&key)
}

pub fn is_supported_field(key: &str) -> bool {
    is_list_field(key) || SCALAR_FIELDS.contains(&key)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Scalar(String),
    List(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontmatterError {
    pub path: String,
    pub message: String,
}

impl FrontmatterError {
    pub fn render(&self) -> String {
        format!("{}: {}", self.path, self.message)
    }
}

#[derive(Debug, Default, Clone)]
pub struct Metadata {
    fields: BTreeMap<String, Value>,
}

impl Metadata {
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// Returns a scalar value. Empty strings count as missing, matching the
    /// truthiness checks in the replaced scripts.
    pub fn scalar(&self, key: &str) -> Option<&str> {
        match self.fields.get(key) {
            Some(Value::Scalar(value)) if !value.is_empty() => Some(value.as_str()),
            _ => None,
        }
    }

    pub fn list(&self, key: &str) -> Option<&[String]> {
        match self.fields.get(key) {
            Some(Value::List(values)) => Some(values.as_slice()),
            _ => None,
        }
    }

    pub fn list_or_empty(&self, key: &str) -> Vec<String> {
        self.list(key).map(<[String]>::to_vec).unwrap_or_default()
    }
}

#[derive(Debug, Default)]
pub struct Parsed {
    pub metadata: Metadata,
    pub errors: Vec<FrontmatterError>,
}

/// Loads and parses a file. A missing file yields empty metadata without an
/// error because callers treat optional artifacts that way.
pub fn load(path: &Path) -> Parsed {
    if !fsx::exists(path) {
        return Parsed::default();
    }
    let display = path.to_string_lossy().into_owned();
    match fsx::read_text(path) {
        Ok(content) => parse_document(&display, &content),
        Err(error) => Parsed {
            metadata: Metadata::default(),
            errors: vec![FrontmatterError {
                path: display,
                message: format!("cannot read file: {}", error.message),
            }],
        },
    }
}

pub fn parse_document(path: &str, content: &str) -> Parsed {
    match split_fence(content) {
        Split::None => Parsed::default(),
        Split::Unclosed => Parsed {
            metadata: Metadata::default(),
            errors: vec![FrontmatterError {
                path: path.to_owned(),
                message: "frontmatter starts with `---` but does not close".to_owned(),
            }],
        },
        Split::Fenced(fence) => parse_block(path, fence.inner),
    }
}

/// A document split at its `---` fences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fence<'a> {
    /// The lines between the fences, without the closing line terminator.
    pub inner: &'a str,
    /// Everything after the closing fence line.
    pub body: &'a str,
    /// The raw block from the opening `---` through the closing `---`.
    pub block: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Split<'a> {
    /// The content does not open with a fence.
    None,
    /// The content opens with a fence that never closes.
    Unclosed,
    Fenced(Fence<'a>),
}

/// Splits a document at its frontmatter fences. The opening fence is `---`
/// on the first line, ending in `\n` or `\r\n`. The closing fence is the
/// next line that is `---` once trailing whitespace and `\r` are dropped,
/// so a line such as `----` inside the block does not close it while a
/// hand-edited `--- ` still does. Every parser in the crate goes through
/// this function so they agree on both rules.
pub fn split_fence(content: &str) -> Split<'_> {
    let Some(first) = content.split_inclusive('\n').next() else {
        return Split::None;
    };
    if !first.ends_with('\n') || !is_fence(first) {
        return Split::None;
    }
    let open = first.len();
    let mut offset = open;
    for line in content[open..].split_inclusive('\n') {
        if is_fence(line) {
            let inner = &content[open..offset];
            let inner = inner.strip_suffix('\n').unwrap_or(inner);
            let inner = inner.strip_suffix('\r').unwrap_or(inner);
            return Split::Fenced(Fence {
                inner,
                body: &content[offset + line.len()..],
                block: &content[..offset + 3],
            });
        }
        offset += line.len();
    }
    Split::Unclosed
}

/// A line is a fence when it is `---` after dropping the line ending and
/// any trailing whitespace.
fn is_fence(line: &str) -> bool {
    line.trim_end() == "---"
}

/// Returns the raw frontmatter block including both `---` fences, or an
/// empty string when the content has none.
pub fn extract_block(content: &str) -> &str {
    match split_fence(content) {
        Split::Fenced(fence) => fence.block,
        Split::None | Split::Unclosed => "",
    }
}

pub fn parse_block(path: &str, block: &str) -> Parsed {
    let mut metadata = Metadata::default();
    let mut errors = Vec::new();
    let mut current_list: Option<String> = None;
    let error = |line: usize, message: String| FrontmatterError {
        path: path.to_owned(),
        message: format!("line {line}: {message}"),
    };

    for (offset, raw_line) in split_lines(block).enumerate() {
        let line_number = offset + 2;
        if raw_line.trim().is_empty() {
            continue;
        }
        if let Some(item) = raw_line.strip_prefix("  - ") {
            let Some(list) = current_list.as_ref() else {
                errors.push(error(
                    line_number,
                    "list item without a list field".to_owned(),
                ));
                continue;
            };
            let value = item.trim();
            if value.is_empty() || value.contains(':') {
                errors.push(error(
                    line_number,
                    format!("only scalar list items are supported in `{list}`"),
                ));
                continue;
            }
            match metadata
                .fields
                .entry(list.clone())
                .or_insert_with(|| Value::List(Vec::new()))
            {
                Value::List(values) => values.push(clean_scalar(value)),
                Value::Scalar(_) => {}
            }
            continue;
        }

        current_list = None;
        if raw_line.starts_with(' ') {
            errors.push(error(
                line_number,
                "nested mappings are not supported in scaffold frontmatter".to_owned(),
            ));
            continue;
        }
        let Some(separator) = raw_line.find(':') else {
            errors.push(error(line_number, "expected `key: value`".to_owned()));
            continue;
        };
        let key = raw_line[..separator].trim();
        let value = raw_line[separator + 1..].trim();
        if !is_supported_field(key) {
            errors.push(error(line_number, format!("unsupported field `{key}`")));
            continue;
        }
        if is_list_field(key) {
            match parse_list_value(key, value) {
                Ok(values) => {
                    metadata.fields.insert(key.to_owned(), Value::List(values));
                    if value.is_empty() {
                        current_list = Some(key.to_owned());
                    }
                }
                Err(message) => errors.push(error(line_number, message)),
            }
            continue;
        }
        if value.starts_with('[') || value.starts_with('{') {
            errors.push(error(
                line_number,
                format!("structured values are not supported for scalar field `{key}`"),
            ));
            continue;
        }
        metadata
            .fields
            .insert(key.to_owned(), Value::Scalar(clean_scalar(value)));
    }
    Parsed { metadata, errors }
}

fn parse_list_value(key: &str, value: &str) -> std::result::Result<Vec<String>, String> {
    if value.is_empty() || value == "[]" {
        return Ok(Vec::new());
    }
    if value.starts_with('[') && value.ends_with(']') {
        let inner = value[1..value.len() - 1].trim();
        if inner.is_empty() {
            return Ok(Vec::new());
        }
        let items: Vec<&str> = inner.split(',').map(str::trim).collect();
        if items.iter().any(|item| item.is_empty()) {
            return Err(format!("malformed inline list for `{key}`"));
        }
        if items.iter().any(|item| item.contains(':')) {
            return Err(format!(
                "only scalar inline list items are supported for `{key}`"
            ));
        }
        return Ok(items.into_iter().map(clean_scalar).collect());
    }
    Err(format!(
        "unsupported list syntax for `{key}`; use `[]`, `[a, b]`, or block list items"
    ))
}

/// Trims and strips one leading and one trailing quote character.
pub fn clean_scalar(value: &str) -> String {
    let mut text = value.trim();
    if text.starts_with('"') || text.starts_with('\'') {
        text = &text[1..];
    }
    if text.ends_with('"') || text.ends_with('\'') {
        text = &text[..text.len() - 1];
    }
    text.to_owned()
}

/// Splits on `\n` and drops a trailing `\r` from each line.
pub fn split_lines(text: &str) -> impl Iterator<Item = &str> {
    text.split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scalars_and_block_lists() {
        let doc = "---\nid: TKT-0001\ntitle: \"Quoted\"\nareas:\n  - .agents/tools\nskills: []\n---\n\n# Body\n";
        let parsed = parse_document("t.md", doc);
        assert!(parsed.errors.is_empty());
        assert_eq!(parsed.metadata.scalar("id"), Some("TKT-0001"));
        assert_eq!(parsed.metadata.scalar("title"), Some("Quoted"));
        assert_eq!(
            parsed.metadata.list("areas"),
            Some(&[".agents/tools".to_owned()][..])
        );
        assert_eq!(parsed.metadata.list("skills"), Some(&[][..]));
    }

    #[test]
    fn reports_unsupported_and_unclosed_frontmatter() {
        let parsed = parse_document("t.md", "---\nid: X\nbogus: 1\n---\n");
        assert_eq!(parsed.errors.len(), 1);
        assert_eq!(
            parsed.errors[0].message,
            "line 3: unsupported field `bogus`"
        );
        let unclosed = parse_document("t.md", "---\nid: X\n");
        assert_eq!(
            unclosed.errors[0].message,
            "frontmatter starts with `---` but does not close"
        );
    }

    #[test]
    fn inline_lists_and_errors() {
        assert_eq!(parse_list_value("tags", "[a, b]").unwrap(), vec!["a", "b"]);
        assert!(parse_list_value("tags", "[a,,b]").is_err());
        assert!(parse_list_value("tags", "- a").is_err());
    }

    #[test]
    fn extracts_block() {
        assert_eq!(extract_block("---\na: 1\n---\nbody"), "---\na: 1\n---");
        assert_eq!(extract_block("no frontmatter"), "");
    }

    #[test]
    fn fence_accepts_crlf_and_matches_the_closing_line_whole() {
        let crlf = "---\r\nid: TKT-0001\r\ntitle: Windows\r\n---\r\n\r\n# Body\r\n";
        let parsed = parse_document("t.md", crlf);
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        assert_eq!(parsed.metadata.scalar("id"), Some("TKT-0001"));
        assert_eq!(parsed.metadata.scalar("title"), Some("Windows"));
        let Split::Fenced(fence) = split_fence(crlf) else {
            panic!("expected a fenced document");
        };
        assert_eq!(fence.inner, "id: TKT-0001\r\ntitle: Windows");
        assert_eq!(fence.body, "\r\n# Body\r\n");
        assert_eq!(fence.block, "---\r\nid: TKT-0001\r\ntitle: Windows\r\n---");

        // A `----` line is not a closing fence; the real one comes later.
        let dashes = "---\nid: TKT-0002\n----\ntitle: After\n---\n\n----\n";
        let Split::Fenced(fence) = split_fence(dashes) else {
            panic!("expected a fenced document");
        };
        assert_eq!(fence.inner, "id: TKT-0002\n----\ntitle: After");
        assert_eq!(fence.body, "\n----\n");
        let parsed = parse_document("t.md", dashes);
        assert_eq!(parsed.metadata.scalar("title"), Some("After"));
        assert_eq!(parsed.errors.len(), 1);
        assert_eq!(parsed.errors[0].message, "line 3: expected `key: value`");
        assert_eq!(split_fence("---\nid: X\n----\n"), Split::Unclosed);
        // Trailing whitespace on either fence line is tolerated.
        let Split::Fenced(fence) = split_fence("--- \nid: X\n---\t\nbody") else {
            panic!("expected a fenced document");
        };
        assert_eq!(fence.inner, "id: X");
        assert_eq!(fence.body, "body");
        assert_eq!(
            split_fence("---\n---"),
            Split::Fenced(Fence {
                inner: "",
                body: "",
                block: "---\n---"
            })
        );
        assert_eq!(split_fence("--- not a fence\n"), Split::None);
    }
}
