//! The session information sidebar: a typed view model and the lines it
//! draws. Nothing here reads a file, prices a token, or talks to a
//! connector; TKT-0019 fills these fields from session events and harness
//! observations. Until then the fields are `None`, which renders as
//! `unavailable` rather than as a zero.
//!
//! The distinction the plan insists on is encoded in the types: cumulative
//! token usage is not context occupancy, so [`UsageSection`] keeps them
//! apart and only reports occupancy when both the current context size and
//! the model limit are known.

use gritt_core::mcp::{McpServerSnapshot, McpServerState};
use ratatui::text::{Line, Span};

use super::theme::Theme;

/// Terminal width at or above which the sidebar shares the screen with
/// the conversation instead of collapsing.
pub const SIDEBAR_MIN_TERMINAL_WIDTH: u16 = 110;
/// The sidebar column itself.
pub const SIDEBAR_WIDTH: u16 = 30;
/// Blank columns between the transcript and the sidebar.
pub const SIDEBAR_GUTTER: u16 = 2;

/// Where the sidebar is on this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarPlacement {
    /// Not shown: either the user turned it off, or the terminal is too
    /// narrow and no drawer is open.
    Hidden,
    /// A column on the right of the conversation.
    Column,
    /// The narrow-terminal form: the same information over the
    /// conversation, closed with Escape.
    Drawer,
}

/// Decides the placement from the terminal width and the two user
/// choices. Below [`SIDEBAR_MIN_TERMINAL_WIDTH`] the column collapses
/// automatically and `/sidebar` opens the drawer instead.
pub fn placement(width: u16, enabled: bool, drawer_open: bool) -> SidebarPlacement {
    if width >= SIDEBAR_MIN_TERMINAL_WIDTH {
        if enabled {
            SidebarPlacement::Column
        } else {
            SidebarPlacement::Hidden
        }
    } else if drawer_open {
        SidebarPlacement::Drawer
    } else {
        SidebarPlacement::Hidden
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionSection {
    pub name: Option<String>,
    pub workspace: Option<String>,
    pub phase: Option<String>,
    /// What the session is doing right now: idle, streaming, a tool name.
    pub activity: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelSection {
    /// Provider profile or installed agent.
    pub backend: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// A connector owns its own model and effort. Set for connector
    /// sessions so the sidebar says so instead of showing Gritt's values.
    pub managed_by_agent: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageSection {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    /// Tokens currently in the model's context, when a source reports it.
    ///
    /// Nothing populates this today. The prompt tokens of the last request
    /// are a *lower bound* on it, not the value: the driver may add tool
    /// results and continuation state after it, and a provider that caches
    /// prompt tokens reports them differently again. Until a source
    /// establishes the current size, occupancy stays unavailable rather
    /// than being derived from something adjacent.
    pub context_tokens: Option<u64>,
    pub context_limit: Option<u64>,
    /// The prompt tokens the most recent request reported, under its own
    /// label. A fact about one request, never occupancy.
    pub last_request_input: Option<u64>,
    /// Set when a usage event arrived without one of its counts, which
    /// makes the totals a floor rather than a total and withholds the cost
    /// estimate.
    pub incomplete: bool,
}

impl UsageSection {
    /// Context occupancy as a fraction, only when both halves are known.
    /// Cumulative usage never stands in for it.
    pub fn occupancy(&self) -> Option<f64> {
        match (self.context_tokens, self.context_limit) {
            (Some(_), Some(0)) | (None, _) | (_, None) => None,
            (Some(used), Some(limit)) => Some(used as f64 / limit as f64),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CostSection {
    /// An estimate, never a billed amount.
    pub estimate_usd: Option<f64>,
    /// What the estimate covers, shown with it.
    pub scope: Option<String>,
}

/// The changed-file types live with the harness service that produces
/// them, so the renderer and the observer cannot drift apart.
pub use crate::changes::{ChangeSource, ChangeStatus, ChangedFile, ChangedFiles};

/// Integration sections. A section that is `None` is hidden entirely;
/// `Some(empty)` means an inventory was checked and found empty, which is
/// the only case that may say "none".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IntegrationsSection {
    pub mcp: Option<Vec<McpServerSnapshot>>,
    /// Who owns the servers in `mcp`. Always Gritt for this list: it comes
    /// from Gritt's runtime and Gritt's `.mcp.json`.
    pub mcp_owner: Option<String>,
    /// The connector whose *own* MCP clients Gritt cannot see. Set while a
    /// connector session is active, so the unknown is stated instead of
    /// Gritt's list standing in for it (ADR-010).
    pub connector_mcp: Option<String>,
}

/// Everything the sidebar shows for one session.
#[derive(Debug, Clone, Default)]
pub struct SidebarModel {
    /// Bumped whenever the session changes. An update carrying an older
    /// generation is a late message from the previous driver.
    pub generation: u64,
    pub session: SessionSection,
    pub model: ModelSection,
    pub usage: UsageSection,
    pub cost: CostSection,
    pub changed_files: ChangedFiles,
    pub integrations: IntegrationsSection,
}

impl SidebarModel {
    /// Clears every field and moves to the next generation. Called when
    /// the session changes so no value survives from the previous driver.
    pub fn reset(&mut self) {
        let generation = self.generation + 1;
        *self = SidebarModel {
            generation,
            ..SidebarModel::default()
        };
    }

    /// Whether an update stamped with `generation` still belongs to the
    /// session on screen.
    pub fn accepts(&self, generation: u64) -> bool {
        generation == self.generation
    }
}

fn value(text: &Option<String>) -> String {
    text.clone().unwrap_or_else(|| "unavailable".into())
}

fn tokens(count: Option<u64>) -> String {
    match count {
        Some(count) => count.to_string(),
        None => "unavailable".into(),
    }
}

fn state_word(state: &McpServerState) -> &'static str {
    match state {
        McpServerState::AwaitingApproval => "awaiting approval",
        McpServerState::Denied => "denied",
        McpServerState::Starting => "starting",
        McpServerState::Ready => "ready",
        McpServerState::Failed { .. } => "failed",
        McpServerState::Stopped => "stopped",
        McpServerState::Invalid { .. } => "invalid",
        McpServerState::UnsupportedTransport { .. } => "unsupported",
    }
}

/// The word shown for an MCP server state, exported so `/mcp` and the
/// sidebar cannot drift apart.
pub fn mcp_state_word(state: &McpServerState) -> &'static str {
    state_word(state)
}

fn truncate(text: &str, width: usize) -> String {
    use super::composer::{clusters, display_width};
    if display_width(text) <= width {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    // Cut between whole characters so a truncated path never ends on a
    // combining mark whose base was dropped.
    for (_, cluster) in clusters(text) {
        let next = display_width(cluster);
        if used + next > width.saturating_sub(1) {
            break;
        }
        out.push_str(cluster);
        used += next;
    }
    out.push('…');
    out
}

fn heading(theme: &Theme, text: &str) -> Line<'static> {
    Line::from(Span::styled(text.to_owned(), theme.heading()))
}

fn field(theme: &Theme, label: &str, text: String, width: usize) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label} "), theme.muted()),
        Span::styled(
            truncate(&text, width.saturating_sub(label.len() + 1).max(4)),
            if text == "unavailable" {
                theme.dim()
            } else {
                theme.text()
            },
        ),
    ])
}

impl SidebarModel {
    /// Renders the sidebar into lines `width` cells wide. Sections appear
    /// in the plan's order; an integration with no runtime is not drawn.
    pub fn lines(&self, theme: &Theme, width: usize) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        lines.push(Line::from(Span::styled(
            format!("Gritt {}", env!("CARGO_PKG_VERSION")),
            theme.accent(),
        )));
        lines.push(Line::default());

        lines.push(heading(theme, "Session"));
        lines.push(field(theme, "name", value(&self.session.name), width));
        lines.push(field(theme, "dir ", value(&self.session.workspace), width));
        lines.push(field(theme, "mode", value(&self.session.phase), width));
        lines.push(field(theme, "now ", value(&self.session.activity), width));
        lines.push(Line::default());

        lines.push(heading(theme, "Model"));
        lines.push(field(theme, "via ", value(&self.model.backend), width));
        if self.model.managed_by_agent {
            lines.push(Line::from(Span::styled(
                "Managed by agent".to_owned(),
                theme.muted(),
            )));
        } else {
            lines.push(field(theme, "id  ", value(&self.model.model), width));
            lines.push(field(theme, "effort", value(&self.model.effort), width));
        }
        lines.push(Line::default());

        lines.push(heading(theme, "Usage"));
        lines.push(field(theme, "in  ", tokens(self.usage.input_tokens), width));
        lines.push(field(
            theme,
            "out ",
            tokens(self.usage.output_tokens),
            width,
        ));
        lines.push(field(
            theme,
            "last",
            match self.usage.last_request_input {
                Some(count) => format!("{count} in, last request"),
                None => "unavailable".to_owned(),
            },
            width,
        ));
        if self.usage.incomplete {
            lines.push(Line::from(Span::styled(
                truncate("partial usage reported; totals are a floor", width),
                theme.dim(),
            )));
        }
        match self.usage.occupancy() {
            Some(fraction) => lines.push(field(
                theme,
                "ctx ",
                format!("{:.0}% of context", fraction * 100.0),
                width,
            )),
            None => lines.push(field(theme, "ctx ", "unavailable".into(), width)),
        }
        lines.push(Line::default());

        lines.push(heading(theme, "Cost"));
        match self.cost.estimate_usd {
            Some(amount) => {
                lines.push(field(theme, "est ", format!("~${amount:.4}"), width));
                lines.push(field(theme, "for ", value(&self.cost.scope), width));
            }
            None => lines.push(field(theme, "est ", "unavailable".into(), width)),
        }
        lines.push(Line::default());

        lines.push(heading(theme, "Changed files"));
        match &self.changed_files {
            ChangedFiles::Unavailable { reason } => {
                lines.push(Line::from(Span::styled(
                    truncate(reason, width),
                    theme.dim(),
                )));
            }
            ChangedFiles::Observed { source, files } => {
                if let Some(caveat) = source.caveat() {
                    lines.push(Line::from(Span::styled(
                        truncate(caveat, width),
                        theme.dim(),
                    )));
                }
                if files.is_empty() {
                    lines.push(Line::from(Span::styled("none".to_owned(), theme.muted())));
                }
                for file in files {
                    let mark = if file.pre_existing { "·" } else { "+" };
                    lines.push(Line::from(vec![
                        Span::styled(format!("{mark} "), theme.muted()),
                        Span::styled(truncate(&file.path, width.saturating_sub(2)), theme.text()),
                    ]));
                    lines.push(Line::from(Span::styled(
                        format!(
                            "  {}{}",
                            file.status.label(),
                            if file.pre_existing {
                                " (pre-existing)"
                            } else {
                                ""
                            }
                        ),
                        theme.muted(),
                    )));
                }
            }
        }

        // Only implemented integrations get a section. LSP and skills have
        // no runtime, so they are absent rather than shown as empty.
        if let Some(servers) = &self.integrations.mcp {
            lines.push(Line::default());
            lines.push(heading(theme, "Integrations"));
            if let Some(owner) = &self.integrations.mcp_owner {
                lines.push(Line::from(Span::styled(
                    truncate(&format!("MCP owned by {owner}"), width),
                    theme.muted(),
                )));
            }
            if let Some(connector) = &self.integrations.connector_mcp {
                lines.push(Line::from(Span::styled(
                    truncate(&format!("{connector}'s own MCP: not reported"), width),
                    theme.dim(),
                )));
            }
            if servers.is_empty() {
                lines.push(Line::from(Span::styled(
                    "no MCP servers configured".to_owned(),
                    theme.muted(),
                )));
            }
            for server in servers {
                let word = state_word(&server.state);
                let style = match &server.state {
                    McpServerState::Ready => theme.success(),
                    McpServerState::Failed { .. }
                    | McpServerState::Invalid { .. }
                    | McpServerState::UnsupportedTransport { .. } => theme.error(),
                    _ => theme.muted(),
                };
                lines.push(Line::from(vec![
                    Span::styled(
                        truncate(&server.name, width.saturating_sub(word.len() + 1)),
                        theme.text(),
                    ),
                    Span::styled(format!(" {word}"), style),
                ]));
                let tools = if server.state.is_ready() {
                    format!("  {} tools", server.tool_count)
                } else {
                    format!("  {}", server.state.explain())
                };
                lines.push(Line::from(Span::styled(
                    truncate(&tools, width),
                    theme.muted(),
                )));
            }
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::theme::{Theme, ThemeMode};

    fn text_of(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_column_collapses_below_110_columns_and_the_drawer_replaces_it() {
        assert_eq!(placement(110, true, false), SidebarPlacement::Column);
        assert_eq!(placement(111, true, false), SidebarPlacement::Column);
        assert_eq!(placement(110, false, false), SidebarPlacement::Hidden);
        assert_eq!(placement(109, true, false), SidebarPlacement::Hidden);
        assert_eq!(placement(109, true, true), SidebarPlacement::Drawer);
        assert_eq!(placement(80, false, true), SidebarPlacement::Drawer);
    }

    #[test]
    fn unknown_values_render_as_unavailable_and_never_as_zero() {
        let model = SidebarModel::default();
        let text = text_of(&model.lines(&Theme::new(ThemeMode::NoColor), 30));
        for label in [
            "name", "dir", "mode", "now", "via", "id", "effort", "in", "out", "ctx", "est",
        ] {
            assert!(
                text.lines()
                    .any(|line| line.starts_with(label) && line.ends_with("unavailable")),
                "{label} is not reported as unavailable in {text}"
            );
        }
        // Not one unknown value is reported as a zero.
        for zero in ["in   0", "out  0", "ctx  0", "est  ~$0"] {
            assert!(!text.contains(zero), "{zero} in {text}");
        }
        // An integration with no runtime is hidden, not shown as empty.
        assert!(!text.contains("Integrations"), "{text}");
        assert!(!text.contains("LSP"), "{text}");
    }

    #[test]
    fn cumulative_usage_is_never_reported_as_context_occupancy() {
        let mut usage = UsageSection {
            input_tokens: Some(12_000),
            output_tokens: Some(3_000),
            ..UsageSection::default()
        };
        assert_eq!(usage.occupancy(), None);
        usage.context_limit = Some(200_000);
        assert_eq!(usage.occupancy(), None);
        usage.context_tokens = Some(50_000);
        assert_eq!(usage.occupancy(), Some(0.25));
        usage.context_limit = Some(0);
        assert_eq!(usage.occupancy(), None);
    }

    #[test]
    fn switching_sessions_clears_every_field_and_rejects_late_updates() {
        let mut model = SidebarModel {
            session: SessionSection {
                name: Some("old".into()),
                ..SessionSection::default()
            },
            usage: UsageSection {
                input_tokens: Some(99),
                ..UsageSection::default()
            },
            ..SidebarModel::default()
        };
        let stale = model.generation;
        model.reset();
        assert!(model.accepts(model.generation));
        assert!(!model.accepts(stale));
        assert_eq!(model.session.name, None);
        assert_eq!(model.usage.input_tokens, None);
    }

    #[test]
    fn pre_existing_changes_are_labelled_and_a_non_git_list_says_it_is_partial() {
        let model = SidebarModel {
            changed_files: ChangedFiles::Observed {
                source: ChangeSource::ObservedWrites,
                files: vec![
                    ChangedFile {
                        path: "src/lib.rs".into(),
                        status: ChangeStatus::Modified,
                        pre_existing: true,
                    },
                    ChangedFile {
                        path: "notes.txt".into(),
                        status: ChangeStatus::Added,
                        pre_existing: false,
                    },
                ],
            },
            ..SidebarModel::default()
        };
        let text = text_of(&model.lines(&Theme::new(ThemeMode::NoColor), 30));
        assert!(text.contains("partial: observed writes only"), "{text}");
        assert!(text.contains("modified (pre-existing)"), "{text}");
        assert!(text.contains("added"), "{text}");
    }

    #[test]
    fn mcp_servers_show_their_state_word_and_a_reason_when_not_ready() {
        let model = SidebarModel {
            integrations: IntegrationsSection {
                mcp: Some(vec![
                    McpServerSnapshot {
                        name: "gritt-local-memory".into(),
                        state: McpServerState::Ready,
                        transport: None,
                        tool_count: 4,
                        tools: Vec::new(),
                        protocol_version: None,
                        server_version: None,
                        fingerprint: "f1".into(),
                    },
                    McpServerSnapshot {
                        name: "broken".into(),
                        state: McpServerState::Failed {
                            reason: "the command exited".into(),
                        },
                        transport: None,
                        tool_count: 0,
                        tools: Vec::new(),
                        protocol_version: None,
                        server_version: None,
                        fingerprint: "f2".into(),
                    },
                ]),
                mcp_owner: None,
                connector_mcp: None,
            },
            ..SidebarModel::default()
        };
        let text = text_of(&model.lines(&Theme::new(ThemeMode::NoColor), 30));
        assert!(text.contains("gritt-local-memory ready"), "{text}");
        assert!(text.contains("4 tools"), "{text}");
        assert!(text.contains("broken failed"), "{text}");
        assert!(text.contains("the command exited"), "{text}");
    }

    #[test]
    fn a_checked_but_empty_inventory_may_say_none() {
        let model = SidebarModel {
            integrations: IntegrationsSection {
                mcp: Some(Vec::new()),
                mcp_owner: None,
                connector_mcp: None,
            },
            ..SidebarModel::default()
        };
        let text = text_of(&model.lines(&Theme::new(ThemeMode::NoColor), 30));
        assert!(text.contains("no MCP servers configured"), "{text}");
    }
}
