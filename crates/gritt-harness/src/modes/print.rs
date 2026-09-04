//! Print mode: streamed text on stdout, activity on stderr, approvals
//! answered through a caller-supplied prompt so scripts and tests can drive
//! it without a terminal.

use std::io::Write;
use std::sync::{Arc, Mutex};

use gritt_core::event::{ApprovalDecision, ApprovalRequest, Event, EventKind, SessionStatus};
use gritt_core::session::BoxFuture;

use crate::agent::Ui;
use crate::policy::Decision;

/// Answers an approval prompt. The binary reads stdin; tests script it.
pub type Prompter =
    Arc<dyn Fn(&ApprovalRequest, &Decision, Option<&str>) -> ApprovalDecision + Send + Sync>;

#[derive(Clone)]
pub struct PrintUiOptions {
    /// Show status changes and reasoning summaries on stderr.
    pub verbose: bool,
    pub prompter: Prompter,
}

impl PrintUiOptions {
    /// Denies everything, for non-interactive use.
    pub fn deny_all(verbose: bool) -> Self {
        Self {
            verbose,
            prompter: Arc::new(|_, _, _| ApprovalDecision::Denied),
        }
    }
}

/// Writes to any pair of writers; the binary passes stdout and stderr.
pub struct PrintUi<O: Write + Send, E: Write + Send> {
    out: O,
    err: E,
    options: PrintUiOptions,
    wrote_text: bool,
}

impl<O: Write + Send, E: Write + Send> PrintUi<O, E> {
    pub fn new(out: O, err: E, options: PrintUiOptions) -> Self {
        Self {
            out,
            err,
            options,
            wrote_text: false,
        }
    }

    /// Ends the streamed text with a newline when any was written.
    pub fn finish(&mut self) {
        if self.wrote_text {
            let _ = writeln!(self.out);
            let _ = self.out.flush();
            self.wrote_text = false;
        }
    }

    pub fn into_parts(self) -> (O, E) {
        (self.out, self.err)
    }

    pub fn parts_mut(&mut self) -> (&mut O, &mut E) {
        (&mut self.out, &mut self.err)
    }
}

/// One line describing a tool call for the activity stream. Arguments are
/// summarized, never dumped whole.
pub fn describe_call(name: &str, arguments: &serde_json::Value) -> String {
    let target = arguments
        .get("path")
        .or_else(|| arguments.get("command"))
        .and_then(|value| value.as_str())
        .map(|value| {
            let line = value.lines().next().unwrap_or_default();
            if line.chars().count() > 80 {
                let cut: String = line.chars().take(77).collect();
                format!("{cut}...")
            } else {
                line.to_owned()
            }
        })
        .unwrap_or_default();
    if target.is_empty() {
        name.to_owned()
    } else {
        format!("{name} {target}")
    }
}

/// The approval prompt text, shared by print, REPL, and the diff view.
pub fn approval_text(
    request: &ApprovalRequest,
    decision: &Decision,
    preview: Option<&str>,
) -> String {
    let mut text = String::new();
    if decision.destructive {
        text.push_str("DESTRUCTIVE ");
    }
    text.push_str(&format!(
        "approval needed: {} on {}\n  reason: {}\n",
        request.tool, request.resource, decision.reason
    ));
    if let Some(diff) = preview {
        text.push_str(diff);
        if !diff.ends_with('\n') {
            text.push('\n');
        }
    }
    text
}

impl<O: Write + Send, E: Write + Send> Ui for PrintUi<O, E> {
    fn event(&mut self, event: &Event) {
        match &event.kind {
            EventKind::TextDelta { text } => {
                let _ = self.out.write_all(text.as_bytes());
                let _ = self.out.flush();
                self.wrote_text = true;
            }
            EventKind::ReasoningSummary { text } if self.options.verbose => {
                let _ = writeln!(self.err, "[reasoning] {text}");
            }
            EventKind::ToolCall { call } => {
                self.finish();
                let _ = writeln!(
                    self.err,
                    "-> {}",
                    describe_call(&call.name, &call.arguments)
                );
            }
            EventKind::ToolResult { result } => {
                let _ = writeln!(
                    self.err,
                    "<- {} {} ({} bytes)",
                    result.name,
                    if result.is_error { "error" } else { "ok" },
                    result.output.len()
                );
                if result.is_error {
                    let first = result.output.lines().next().unwrap_or_default();
                    let _ = writeln!(self.err, "   {first}");
                }
            }
            EventKind::ApprovalDecided { decision, .. } => {
                let _ = writeln!(self.err, "   {decision:?}");
            }
            EventKind::StatusChanged { status } => {
                if self.options.verbose || *status == SessionStatus::WaitingForApproval {
                    let _ = writeln!(self.err, "[status] {status:?}");
                }
            }
            EventKind::Error { message, .. } => {
                self.finish();
                let _ = writeln!(self.err, "error: {message}");
            }
            EventKind::Cancelled => {
                self.finish();
                let _ = writeln!(self.err, "cancelled");
            }
            EventKind::Usage { usage } if self.options.verbose => {
                let _ = writeln!(
                    self.err,
                    "[usage] in={} out={}",
                    usage.input_tokens.unwrap_or(0),
                    usage.output_tokens.unwrap_or(0)
                );
            }
            _ => {}
        }
        if let Some(warning) = event
            .diagnostic
            .as_ref()
            .and_then(|d| d.get("capability_warning"))
        {
            let features = warning
                .get("features")
                .map(|f| f.to_string())
                .unwrap_or_default();
            let _ = writeln!(
                self.err,
                "warning: the provider did not report support for {features}"
            );
        }
    }

    fn approve<'a>(
        &'a mut self,
        request: &'a ApprovalRequest,
        decision: &'a Decision,
        preview: Option<&'a str>,
    ) -> BoxFuture<'a, ApprovalDecision> {
        self.finish();
        let _ = self
            .err
            .write_all(approval_text(request, decision, preview).as_bytes());
        let _ = self.err.flush();
        let answer = (self.options.prompter)(request, decision, preview);
        Box::pin(async move { answer })
    }
}

/// A writer that tests can read back.
#[derive(Clone, Default)]
pub struct SharedBuffer(pub Arc<Mutex<Vec<u8>>>);

impl SharedBuffer {
    pub fn contents(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().expect("buffer")).into_owned()
    }
}

impl Write for SharedBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("buffer").extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Reads one `y`/`n` line from a reader; anything else denies.
pub fn read_yes_no(input: &mut dyn std::io::BufRead) -> ApprovalDecision {
    let mut line = String::new();
    if input.read_line(&mut line).is_err() {
        return ApprovalDecision::Denied;
    }
    match line.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => ApprovalDecision::Approved,
        _ => ApprovalDecision::Denied,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_descriptions_are_short() {
        let long = "x".repeat(200);
        let text = describe_call("shell", &serde_json::json!({ "command": long }));
        assert!(text.starts_with("shell xxx"));
        assert!(text.ends_with("..."));
        assert!(text.chars().count() < 90);
        assert_eq!(
            describe_call("file_read", &serde_json::json!({ "path": "a.txt" })),
            "file_read a.txt"
        );
    }

    #[test]
    fn yes_no_parsing() {
        let mut yes = std::io::Cursor::new(b"Y\n".to_vec());
        assert_eq!(read_yes_no(&mut yes), ApprovalDecision::Approved);
        let mut other = std::io::Cursor::new(b"maybe\n".to_vec());
        assert_eq!(read_yes_no(&mut other), ApprovalDecision::Denied);
        let mut empty = std::io::Cursor::new(Vec::new());
        assert_eq!(read_yes_no(&mut empty), ApprovalDecision::Denied);
    }
}
