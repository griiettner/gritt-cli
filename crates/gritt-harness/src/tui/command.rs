//! The command registry. Slash suggestions, the Ctrl-P palette, and the
//! keyboard shortcuts all resolve through this one table, so a command
//! cannot exist in one entry point and not another.
//!
//! Commands are handled locally. Nothing here produces a prompt for the
//! model: [`parse`] tells the composer whether a submission is a command,
//! a literal line, or ordinary prompt text, and multiline text is always
//! prompt text.

/// Every locally handled command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Command {
    Connect,
    Models,
    Effort,
    Mode,
    Plan,
    Code,
    Sessions,
    New,
    Details,
    Sidebar,
    Mcp,
    Version,
    Update,
    Help,
    Quit,
}

/// One registry row: the canonical name, alternative spellings, a one-line
/// summary for `/` search and the palette, and the shortcut to advertise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandSpec {
    pub command: Command,
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub summary: &'static str,
    pub shortcut: Option<&'static str>,
}

impl CommandSpec {
    /// True when `query` matches the name, an alias, or the summary. An
    /// empty query matches everything.
    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().trim_start_matches('/').to_ascii_lowercase();
        if query.is_empty() {
            return true;
        }
        self.name.contains(&query)
            || self.aliases.iter().any(|alias| alias.contains(&query))
            || self.summary.to_ascii_lowercase().contains(&query)
    }
}

/// The registry, in the order the palette and `/` list show it.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        command: Command::Connect,
        name: "connect",
        aliases: &["provider", "agent"],
        summary: "Choose an AI provider or an installed agent",
        shortcut: None,
    },
    CommandSpec {
        command: Command::Models,
        name: "models",
        aliases: &["model"],
        summary: "Search the selected provider's models and choose one",
        shortcut: None,
    },
    CommandSpec {
        command: Command::Effort,
        name: "effort",
        aliases: &["reasoning"],
        summary: "Choose the reasoning effort for native turns",
        shortcut: None,
    },
    CommandSpec {
        command: Command::Plan,
        name: "plan",
        aliases: &[],
        summary: "Switch to the planning phase",
        shortcut: None,
    },
    CommandSpec {
        command: Command::Code,
        name: "code",
        aliases: &[],
        summary: "Switch to the coding phase",
        shortcut: None,
    },
    CommandSpec {
        command: Command::Mode,
        name: "mode",
        aliases: &["permissions"],
        summary: "Choose Planning, Supervised, Auto Approve, or Full Access",
        shortcut: Some("Shift+Tab"),
    },
    CommandSpec {
        command: Command::Sessions,
        name: "sessions",
        aliases: &["resume"],
        summary: "Search and resume sessions for this workspace",
        shortcut: Some("Ctrl-S"),
    },
    CommandSpec {
        command: Command::New,
        name: "new",
        aliases: &[],
        summary: "Start a fresh draft without deleting the current session",
        shortcut: None,
    },
    CommandSpec {
        command: Command::Details,
        name: "details",
        aliases: &["tools"],
        summary: "Expand or collapse tool output",
        shortcut: None,
    },
    CommandSpec {
        command: Command::Sidebar,
        name: "sidebar",
        aliases: &["info"],
        summary: "Toggle the session information sidebar",
        shortcut: None,
    },
    CommandSpec {
        command: Command::Mcp,
        name: "mcp",
        aliases: &["servers"],
        summary: "Inspect configured MCP servers and their tools",
        shortcut: None,
    },
    CommandSpec {
        command: Command::Version,
        name: "version",
        aliases: &["outdated"],
        summary: "Check the installed agent CLI against its newest published version",
        shortcut: None,
    },
    CommandSpec {
        command: Command::Update,
        name: "update",
        aliases: &["upgrade"],
        summary: "Update the installed agent CLI with its installer's command, after approval",
        shortcut: None,
    },
    CommandSpec {
        command: Command::Help,
        name: "help",
        aliases: &["keys", "?"],
        summary: "Show commands, keys, and current limitations",
        shortcut: None,
    },
    CommandSpec {
        command: Command::Quit,
        name: "quit",
        aliases: &["exit"],
        summary: "Leave Gritt and restore the terminal",
        shortcut: Some("Ctrl-Q"),
    },
];

/// The registry row for a command.
pub fn spec(command: Command) -> &'static CommandSpec {
    COMMANDS
        .iter()
        .find(|spec| spec.command == command)
        // Every variant has a row; the table and the enum are edited
        // together and `every_command_has_one_row` proves it.
        .expect("every command has a registry row")
}

/// Resolves a name or alias, without its leading slash.
pub fn lookup(name: &str) -> Option<Command> {
    let name = name.trim().trim_start_matches('/').to_ascii_lowercase();
    COMMANDS
        .iter()
        .find(|spec| spec.name == name || spec.aliases.contains(&name.as_str()))
        .map(|spec| spec.command)
}

/// Registry rows matching `query`, for `/` suggestions and the palette.
pub fn search(query: &str) -> Vec<&'static CommandSpec> {
    COMMANDS.iter().filter(|spec| spec.matches(query)).collect()
}

/// What a submitted composer buffer is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// Ordinary prompt text for the model.
    Prompt(String),
    /// A registry command, with the rest of the line as its argument.
    Command {
        command: Command,
        argument: Option<String>,
    },
    /// A slash word that is not in the registry. The interface shows a
    /// local error and keeps the input.
    Unknown(String),
}

/// Classifies a submission.
///
/// A leading `/` opens a command. `//` escapes it: the line is prompt text
/// with one slash removed. Text with a newline in it is always prompt
/// text, so a pasted script or diff can never run a command.
pub fn parse(input: &str) -> Parsed {
    if input.contains('\n') {
        return Parsed::Prompt(input.to_owned());
    }
    let trimmed = input.trim();
    let Some(rest) = trimmed.strip_prefix('/') else {
        return Parsed::Prompt(input.trim().to_owned());
    };
    if let Some(literal) = rest.strip_prefix('/') {
        // `//deploy` is the literal line `/deploy`.
        return Parsed::Prompt(format!("/{literal}"));
    }
    let mut parts = rest.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or_default();
    let argument = parts
        .next()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned);
    match lookup(name) {
        Some(command) => Parsed::Command { command, argument },
        None => Parsed::Unknown(name.to_owned()),
    }
}

/// The suggestion query for a composer buffer, or `None` when suggestions
/// should stay closed. Suggestions only open on a single-line buffer that
/// starts with exactly one slash and has no argument yet.
pub fn suggestion_query(input: &str) -> Option<&str> {
    if input.contains('\n') {
        return None;
    }
    let rest = input.strip_prefix('/')?;
    if rest.starts_with('/') || rest.contains(char::is_whitespace) {
        return None;
    }
    Some(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_has_one_row_and_resolves_by_name_and_alias() {
        let all = [
            Command::Mode,
            Command::Connect,
            Command::Models,
            Command::Effort,
            Command::Plan,
            Command::Code,
            Command::Sessions,
            Command::New,
            Command::Details,
            Command::Sidebar,
            Command::Mcp,
            Command::Version,
            Command::Update,
            Command::Help,
            Command::Quit,
        ];
        assert_eq!(all.len(), COMMANDS.len());
        // `/resume` is an alias of `/sessions`: one searchable list.
        assert_eq!(lookup("resume"), Some(Command::Sessions));
        for command in all {
            let row = spec(command);
            assert_eq!(lookup(row.name), Some(command));
            for alias in row.aliases {
                assert_eq!(lookup(alias), Some(command), "alias {alias}");
            }
        }
        assert_eq!(lookup("/model"), Some(Command::Models));
        assert_eq!(lookup("nope"), None);
    }

    #[test]
    fn slash_commands_parse_and_unknown_ones_are_reported_not_sent() {
        assert_eq!(
            parse("/plan"),
            Parsed::Command {
                command: Command::Plan,
                argument: None
            }
        );
        assert_eq!(
            parse("/resume nightly build"),
            Parsed::Command {
                command: Command::Sessions,
                argument: Some("nightly build".into())
            }
        );
        assert_eq!(parse("/deploy"), Parsed::Unknown("deploy".into()));
        assert_eq!(parse("write a test"), Parsed::Prompt("write a test".into()));
    }

    #[test]
    fn a_double_slash_escapes_and_multiline_paste_is_never_a_command() {
        assert_eq!(parse("//quit"), Parsed::Prompt("/quit".into()));
        let pasted = "/quit\nrm -rf /\n";
        assert_eq!(parse(pasted), Parsed::Prompt(pasted.to_owned()));
        assert_eq!(suggestion_query(pasted), None);
        assert_eq!(suggestion_query("//qu"), None);
    }

    #[test]
    fn suggestions_open_on_a_bare_slash_word_and_close_after_an_argument() {
        assert_eq!(suggestion_query("/"), Some(""));
        assert_eq!(suggestion_query("/mo"), Some("mo"));
        assert_eq!(suggestion_query("/models gpt"), None);
        assert_eq!(suggestion_query("hello"), None);
        let hits = search("mo");
        assert!(hits.iter().any(|spec| spec.command == Command::Models));
        assert_eq!(search("").len(), COMMANDS.len());
        // The summary is searchable too, so "reasoning" finds /effort.
        assert!(search("reasoning")
            .iter()
            .any(|spec| spec.command == Command::Effort));
    }
}
