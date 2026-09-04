//! Splits a document into line-addressable chunks.
//!
//! Markdown headings start a new section. Sections longer than `MAX_LINES`
//! are split into overlapping windows so a citation never spans more than
//! eighty lines.

use crate::frontmatter::split_lines;

const MAX_LINES: usize = 80;
const OVERLAP_LINES: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub heading: Option<String>,
    pub start_line: usize,
    pub end_line: usize,
    pub content: String,
}

/// Returns the heading text of an ATX heading line.
pub fn heading_for_line(line: &str) -> Option<String> {
    let hashes = line.bytes().take_while(|b| *b == b'#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &line[hashes..];
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim_start();
    if rest.is_empty() {
        return None;
    }
    let trimmed = rest.trim_end_matches('#').trim_end();
    if trimmed.is_empty() {
        // Only hashes remain: the lazy capture keeps the first character.
        return rest.chars().next().map(|c| c.to_string());
    }
    Some(trimmed.to_owned())
}

fn split_section(lines: &[&str], heading: Option<&str>, start_line: usize) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut cursor = 0;
    while cursor < lines.len() {
        let end = (cursor + MAX_LINES).min(lines.len());
        let content = lines[cursor..end].join("\n").trim().to_owned();
        if !content.is_empty() {
            chunks.push(Chunk {
                heading: heading.map(str::to_owned),
                start_line: start_line + cursor,
                end_line: start_line + end - 1,
                content,
            });
        }
        if end == lines.len() {
            break;
        }
        cursor = end - OVERLAP_LINES;
    }
    chunks
}

pub fn chunk_document(content: &str) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut section: Vec<&str> = Vec::new();
    let mut section_heading: Option<String> = None;
    let mut section_start = 1;

    for (index, line) in split_lines(content).enumerate() {
        if let Some(heading) = heading_for_line(line) {
            if !section.is_empty() {
                chunks.extend(split_section(
                    &section,
                    section_heading.as_deref(),
                    section_start,
                ));
                section.clear();
            }
            section_heading = Some(heading);
            section_start = index + 1;
        }
        section.push(line);
    }
    chunks.extend(split_section(
        &section,
        section_heading.as_deref(),
        section_start,
    ));
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headings_are_detected() {
        assert_eq!(heading_for_line("# Title"), Some("Title".to_owned()));
        assert_eq!(heading_for_line("## Title ##"), Some("Title".to_owned()));
        assert_eq!(heading_for_line("#NoSpace"), None);
        assert_eq!(heading_for_line("####### Seven"), None);
        assert_eq!(heading_for_line("text"), None);
    }

    #[test]
    fn sections_split_on_headings_with_line_numbers() {
        let doc = "intro\n\n# One\nbody one\n\n## Two\nbody two\n";
        let chunks = chunk_document(doc);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].heading, None);
        assert_eq!((chunks[0].start_line, chunks[0].end_line), (1, 2));
        assert_eq!(chunks[1].heading.as_deref(), Some("One"));
        assert_eq!((chunks[1].start_line, chunks[1].end_line), (3, 5));
        assert_eq!(chunks[2].heading.as_deref(), Some("Two"));
        assert_eq!((chunks[2].start_line, chunks[2].end_line), (6, 8));
        assert_eq!(chunks[2].content, "## Two\nbody two");
    }

    #[test]
    fn long_sections_overlap() {
        let doc = (1..=150)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let chunks = chunk_document(&doc);
        assert_eq!(chunks.len(), 2);
        assert_eq!((chunks[0].start_line, chunks[0].end_line), (1, 80));
        assert_eq!((chunks[1].start_line, chunks[1].end_line), (71, 150));
    }
}
