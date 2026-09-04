//! Frontmatter shared by every ticket scaffold, so `ticket new` and
//! `ticket new-chain` render the same block from one place.

use super::sync::render_list;

/// Every field a scaffolded ticket artifact can carry. Optional scalars and
/// empty lists are omitted from the output.
#[derive(Debug, Clone, Copy, Default)]
pub struct Frontmatter<'a> {
    pub ticket_id: &'a str,
    pub namespace: &'a str,
    pub title: &'a str,
    pub artifact: &'a str,
    pub status: &'a str,
    pub owner: &'a str,
    pub created: &'a str,
    pub updated: &'a str,
    pub chain_role: Option<&'a str>,
    pub chain_parent: Option<&'a str>,
    pub chain_children: &'a [String],
    pub dependencies: &'a [String],
    pub areas: &'a [String],
    pub skills: &'a [String],
}

/// Renders the frontmatter block followed by one blank line, ready for the
/// artifact heading.
pub fn render_frontmatter(values: &Frontmatter<'_>) -> String {
    let mut lines = vec![
        "---".to_owned(),
        format!("id: {}", values.ticket_id),
        format!("namespace: {}", values.namespace),
        format!("title: {}", yaml_scalar(values.title)),
        format!("artifact: {}", values.artifact),
        format!("status: {}", values.status),
        format!("owner: {}", values.owner),
        format!("created: {}", values.created),
        format!("updated: {}", values.updated),
    ];
    if let Some(role) = values.chain_role {
        lines.push(format!("chain_role: {role}"));
    }
    if let Some(parent) = values.chain_parent {
        lines.push(format!("chain_parent: {parent}"));
    }
    append_list(&mut lines, "chain_children", values.chain_children);
    append_list(&mut lines, "dependencies", values.dependencies);
    append_list(&mut lines, "areas", values.areas);
    append_list(&mut lines, "skills", values.skills);
    lines.push("---".to_owned());
    lines.push(String::new());
    lines.push(String::new());
    lines.join("\n")
}

/// Block list in the same shape `ticket sync` renders, omitted when empty.
fn append_list(lines: &mut Vec<String>, name: &str, values: &[String]) {
    if !values.is_empty() {
        render_list(lines, name, values, "");
    }
}

/// Quotes a scalar the frontmatter parser would otherwise reject as a
/// structured value, for example a title that starts with `[` or `{`.
pub fn yaml_scalar(value: &str) -> String {
    if value.starts_with(['[', '{']) {
        format!("\"{value}\"")
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontmatter::parse_document;

    #[test]
    fn renders_the_eight_scalars_and_only_the_lists_that_are_set() {
        let plain = Frontmatter {
            ticket_id: "TKT-0001",
            namespace: "alice",
            title: "[Spike] eval: thing",
            artifact: "task",
            status: "ready",
            owner: "alice",
            created: "2026-09-04",
            updated: "2026-09-04",
            ..Frontmatter::default()
        };
        assert_eq!(
            render_frontmatter(&plain),
            "---\nid: TKT-0001\nnamespace: alice\ntitle: \"[Spike] eval: thing\"\nartifact: task\nstatus: ready\nowner: alice\ncreated: 2026-09-04\nupdated: 2026-09-04\n---\n\n"
        );
        let parsed = parse_document("task.md", &render_frontmatter(&plain));
        assert!(parsed.errors.is_empty());
        assert_eq!(parsed.metadata.scalar("title"), Some("[Spike] eval: thing"));
        assert_eq!(yaml_scalar("Plain title"), "Plain title");

        let children = vec!["TKT-0002".to_owned()];
        let areas = vec![".agents/tasks".to_owned()];
        let full = Frontmatter {
            chain_role: Some("orchestrator"),
            chain_parent: Some("TKT-0000"),
            chain_children: &children,
            areas: &areas,
            ..plain
        };
        let text = render_frontmatter(&full);
        assert!(text.contains(
            "updated: 2026-09-04\nchain_role: orchestrator\nchain_parent: TKT-0000\nchain_children:\n  - TKT-0002\nareas:\n  - .agents/tasks\n---\n\n"
        ));
        assert!(!text.contains("dependencies"));
        assert!(!text.contains("skills"));
    }
}
