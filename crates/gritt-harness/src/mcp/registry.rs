//! The tool registry and the result renderer.
//!
//! Every discovered tool gets one provider-valid dispatch name that maps
//! back to its server and its original name, so two servers offering
//! `search` both stay callable. The definitions leave here as ordinary
//! [`ToolDefinition`]s, which is why no provider adapter needs to know MCP
//! exists.

use std::collections::HashMap;

use gritt_core::mcp::McpToolRef;
use gritt_core::tool::ToolDefinition;
use serde_json::Value;

/// The prefix that marks a dispatch name as belonging to an MCP server.
pub const DISPATCH_PREFIX: &str = "mcp__";

/// Providers accept `[A-Za-z0-9_-]{1,64}` for a function name across the
/// three supported protocols, so the registry stays inside that.
const MAX_NAME_BYTES: usize = 64;

/// One tool as the registry holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct RegisteredTool {
    pub reference: McpToolRef,
    pub definition: ToolDefinition,
    /// The server's `annotations` object, kept for display only. It is never
    /// consulted for a permission decision: the specification says a client
    /// must treat annotations from an unvetted server as untrusted.
    pub annotations: Option<Value>,
}

#[derive(Debug, Clone, Default)]
pub struct ToolRegistry {
    tools: Vec<RegisteredTool>,
    by_name: HashMap<String, usize>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Adds one discovered tool and returns the dispatch name it received.
    /// A name already taken gets a numbered suffix, so a collision after
    /// sanitizing or truncating never hides a tool.
    pub fn insert(&mut self, server: &str, tool: &Value) -> Option<String> {
        let name = tool.get("name")?.as_str()?;
        if name.is_empty() {
            return None;
        }
        let dispatch = self.unique_name(server, name);
        let description = tool
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| {
                let title = tool.get("title").and_then(Value::as_str).unwrap_or(name);
                format!("{title} (from the `{server}` MCP server)")
            });
        // A server may omit the schema or send a non-object; the adapters
        // need an object schema, so an empty one stands in.
        let parameters = match tool.get("inputSchema") {
            Some(schema) if schema.is_object() => schema.clone(),
            _ => serde_json::json!({"type": "object", "properties": {}}),
        };
        let registered = RegisteredTool {
            reference: McpToolRef {
                dispatch_name: dispatch.clone(),
                server: server.to_owned(),
                tool: name.to_owned(),
            },
            definition: ToolDefinition {
                name: dispatch.clone(),
                description,
                parameters,
            },
            annotations: tool.get("annotations").cloned(),
        };
        self.by_name.insert(dispatch.clone(), self.tools.len());
        self.tools.push(registered);
        Some(dispatch)
    }

    fn unique_name(&self, server: &str, tool: &str) -> String {
        let base = dispatch_name(server, tool);
        if !self.by_name.contains_key(&base) {
            return base;
        }
        for suffix in 2..u32::MAX {
            let marker = format!("_{suffix}");
            let mut candidate = base.clone();
            if candidate.len() + marker.len() > MAX_NAME_BYTES {
                candidate.truncate(MAX_NAME_BYTES - marker.len());
            }
            candidate.push_str(&marker);
            if !self.by_name.contains_key(&candidate) {
                return candidate;
            }
        }
        unreachable!("a unique tool name always exists below u32::MAX")
    }

    pub fn lookup(&self, dispatch_name: &str) -> Option<&RegisteredTool> {
        self.by_name
            .get(dispatch_name)
            .and_then(|index| self.tools.get(*index))
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .map(|tool| tool.definition.clone())
            .collect()
    }

    pub fn tools(&self) -> &[RegisteredTool] {
        &self.tools
    }

    pub fn names_for(&self, server: &str) -> Vec<String> {
        self.tools
            .iter()
            .filter(|tool| tool.reference.server == server)
            .map(|tool| tool.reference.dispatch_name.clone())
            .collect()
    }

    /// Drops every tool belonging to `server`, for a restart or a reload.
    pub fn remove_server(&mut self, server: &str) {
        self.tools.retain(|tool| tool.reference.server != server);
        self.reindex();
    }

    fn reindex(&mut self) {
        self.by_name = self
            .tools
            .iter()
            .enumerate()
            .map(|(index, tool)| (tool.reference.dispatch_name.clone(), index))
            .collect();
    }
}

/// `mcp__<server>__<tool>`, with anything a provider would reject replaced
/// by `_`. The server name is part of the name, so two servers with the same
/// tool name do not collide.
pub fn dispatch_name(server: &str, tool: &str) -> String {
    let mut name = format!("{DISPATCH_PREFIX}{}__{}", sanitize(server), sanitize(tool));
    if name.len() > MAX_NAME_BYTES {
        name.truncate(MAX_NAME_BYTES);
    }
    name
}

/// True when a tool name belongs to an MCP server rather than a native tool.
pub fn is_dispatch_name(name: &str) -> bool {
    name.starts_with(DISPATCH_PREFIX)
}

fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// What one `tools/call` produced, ready for the shared tool-result event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedResult {
    pub output: String,
    pub is_error: bool,
    /// Content block types the result carried that Gritt cannot show to a
    /// model, reported rather than dropped.
    pub unsupported: Vec<String>,
    pub truncated: bool,
}

/// Turns a `CallToolResult` into text for the model.
///
/// `isError` on a successful response is a tool execution error, not a
/// protocol error: the content is handed back so the model can correct
/// itself. Structured output is appended whole or not at all, so bounding
/// the size never produces invalid JSON.
pub fn render_result(value: &Value, max_bytes: usize) -> RenderedResult {
    let is_error = value
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut parts: Vec<String> = Vec::new();
    let mut unsupported: Vec<String> = Vec::new();
    if let Some(blocks) = value.get("content").and_then(Value::as_array) {
        for block in blocks {
            match render_block(block) {
                Ok(text) => parts.push(text),
                Err(kind) => {
                    if !unsupported.contains(&kind) {
                        unsupported.push(kind.clone());
                    }
                    parts.push(format!("[unsupported `{kind}` content omitted]"));
                }
            }
        }
    }
    let mut output = parts.join("\n");
    let mut truncated = false;
    if output.len() > max_bytes {
        let cut = (0..=max_bytes)
            .rev()
            .find(|index| output.is_char_boundary(*index))
            .unwrap_or(0);
        output.truncate(cut);
        output.push_str("\n[output truncated]");
        truncated = true;
    }
    if let Some(structured) = value.get("structuredContent") {
        let text = serde_json::to_string_pretty(structured).unwrap_or_default();
        if text.len() <= max_bytes {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str("[structured result]\n");
            output.push_str(&text);
        } else {
            // Cutting JSON in half would hand the model something it cannot
            // parse; saying it was dropped is the honest answer.
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str("[structured result omitted: larger than the result limit]");
            truncated = true;
        }
    }
    if output.is_empty() {
        output = if is_error {
            "the tool reported an error with no detail".to_owned()
        } else {
            "the tool returned no content".to_owned()
        };
    }
    RenderedResult {
        output,
        is_error,
        unsupported,
        truncated,
    }
}

/// One content block as text, or the block type when it has none.
fn render_block(block: &Value) -> std::result::Result<String, String> {
    let kind = block
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    match kind {
        "text" => Ok(block
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()),
        "resource_link" => {
            let uri = block.get("uri").and_then(Value::as_str).unwrap_or("");
            let name = block.get("name").and_then(Value::as_str).unwrap_or("");
            Ok(format!("[resource link {name} {uri}]").replace("  ", " "))
        }
        "resource" => {
            let resource = block.get("resource").cloned().unwrap_or(Value::Null);
            let uri = resource.get("uri").and_then(Value::as_str).unwrap_or("");
            match resource.get("text").and_then(Value::as_str) {
                Some(text) => Ok(format!("[resource {uri}]\n{text}")),
                // A blob is base64 binary; the model cannot use it.
                None => Err("binary resource".to_owned()),
            }
        }
        other => Err(other.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str) -> Value {
        serde_json::json!({
            "name": name,
            "description": "does a thing",
            "inputSchema": {"type": "object", "properties": {"q": {"type": "string"}}}
        })
    }

    #[test]
    fn duplicate_tool_names_across_servers_stay_addressable() {
        let mut registry = ToolRegistry::new();
        let first = registry.insert("memory", &tool("search")).unwrap();
        let second = registry.insert("docs", &tool("search")).unwrap();
        assert_ne!(first, second);
        assert_eq!(first, "mcp__memory__search");
        assert_eq!(second, "mcp__docs__search");
        assert_eq!(registry.lookup(&first).unwrap().reference.tool, "search");
        assert_eq!(registry.lookup(&second).unwrap().reference.server, "docs");
        assert_eq!(registry.definitions().len(), 2);
    }

    #[test]
    fn names_are_sanitized_bounded_and_never_reused() {
        let mut registry = ToolRegistry::new();
        let long = "x".repeat(120);
        let a = registry.insert("a server/one", &tool(&long)).unwrap();
        let b = registry
            .insert("a server/one", &tool(&format!("{long}-other")))
            .unwrap();
        assert!(a.len() <= MAX_NAME_BYTES && b.len() <= MAX_NAME_BYTES);
        assert_ne!(a, b);
        assert!(a.starts_with("mcp__a_server_one__"));
        assert!(a
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));
        assert!(is_dispatch_name(&a));
        assert!(!is_dispatch_name("file_read"));
    }

    #[test]
    fn removing_a_server_leaves_the_others_addressable() {
        let mut registry = ToolRegistry::new();
        registry.insert("a", &tool("one")).unwrap();
        let keep = registry.insert("b", &tool("two")).unwrap();
        registry.remove_server("a");
        assert_eq!(registry.len(), 1);
        assert!(registry.lookup("mcp__a__one").is_none());
        assert_eq!(registry.lookup(&keep).unwrap().reference.server, "b");
        assert_eq!(registry.names_for("b"), vec![keep]);
    }

    #[test]
    fn a_tool_without_a_schema_still_registers() {
        let mut registry = ToolRegistry::new();
        let name = registry
            .insert("s", &serde_json::json!({"name": "bare"}))
            .unwrap();
        let registered = registry.lookup(&name).unwrap();
        assert_eq!(registered.definition.parameters["type"], "object");
        assert!(registered.definition.description.contains("`s`"));
        assert!(registry.insert("s", &serde_json::json!({})).is_none());
    }

    #[test]
    fn text_and_structured_results_both_reach_the_model() {
        let rendered = render_result(
            &serde_json::json!({
                "content": [{"type": "text", "text": "found 2 rows"}],
                "structuredContent": {"rows": 2}
            }),
            1024,
        );
        assert!(!rendered.is_error);
        assert!(rendered.output.starts_with("found 2 rows"));
        assert!(rendered.output.contains("\"rows\": 2"));
        assert!(rendered.unsupported.is_empty());
    }

    #[test]
    fn an_execution_error_returns_its_content_instead_of_failing() {
        let rendered = render_result(
            &serde_json::json!({
                "content": [{"type": "text", "text": "the API rejected the query"}],
                "isError": true
            }),
            1024,
        );
        assert!(rendered.is_error);
        assert_eq!(rendered.output, "the API rejected the query");
    }

    #[test]
    fn unsupported_blocks_are_reported_not_dropped() {
        let rendered = render_result(
            &serde_json::json!({"content": [
                {"type": "text", "text": "before"},
                {"type": "image", "data": "AAAA", "mimeType": "image/png"},
                {"type": "resource", "resource": {"uri": "file:///a", "text": "inline"}},
                {"type": "resource", "resource": {"uri": "file:///b", "blob": "AAAA"}},
                {"type": "resource_link", "uri": "file:///c", "name": "c"}
            ]}),
            1024,
        );
        assert!(rendered.unsupported.contains(&"image".to_string()));
        assert!(rendered
            .unsupported
            .contains(&"binary resource".to_string()));
        assert!(rendered.output.contains("before"));
        assert!(rendered
            .output
            .contains("[unsupported `image` content omitted]"));
        assert!(rendered.output.contains("inline"));
        assert!(rendered.output.contains("file:///c"));
    }

    #[test]
    fn bounding_the_payload_never_corrupts_structured_output() {
        let big = "y".repeat(4096);
        let rendered = render_result(
            &serde_json::json!({
                "content": [{"type": "text", "text": big}],
                "structuredContent": {"blob": "z".repeat(4096)}
            }),
            256,
        );
        assert!(rendered.truncated);
        assert!(rendered.output.contains("[output truncated]"));
        assert!(rendered.output.contains("[structured result omitted"));
        assert!(!rendered.output.contains("\"blob\""));
        // Whatever survives is still valid UTF-8 the model can read.
        assert!(rendered.output.is_char_boundary(rendered.output.len()));
    }

    #[test]
    fn an_empty_result_says_so() {
        let rendered = render_result(&serde_json::json!({"content": []}), 64);
        assert_eq!(rendered.output, "the tool returned no content");
        assert!(!rendered.is_error);
    }
}
