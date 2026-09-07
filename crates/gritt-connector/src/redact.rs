//! Key redaction for connector output. Longer secrets are replaced first
//! so a shorter secret that is a substring of another cannot leave the
//! rest of the longer one behind.

use gritt_core::secret::Secret;

pub const REDACTED: &str = "[redacted]";

pub fn redact_text(text: &str, secrets: &[Secret]) -> String {
    let mut ordered: Vec<&str> = secrets
        .iter()
        .map(Secret::expose)
        .filter(|value| !value.is_empty())
        .collect();
    ordered.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    ordered.dedup();
    let mut out = text.to_owned();
    for value in ordered {
        if out.contains(value) {
            out = out.replace(value, REDACTED);
        }
    }
    out
}

pub fn redact_value(value: serde_json::Value, secrets: &[Secret]) -> serde_json::Value {
    match value {
        serde_json::Value::String(text) => serde_json::Value::String(redact_text(&text, secrets)),
        serde_json::Value::Array(items) => serde_json::Value::Array(
            items
                .into_iter()
                .map(|item| redact_value(item, secrets))
                .collect(),
        ),
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.into_iter()
                .map(|(key, item)| (redact_text(&key, secrets), redact_value(item, secrets)))
                .collect(),
        ),
        other => other,
    }
}

/// Longest target kept for display.
const TARGET_MAX: usize = 512;

/// Display text for an MCP server's launch command or URL, as an external
/// agent printed it. Known secrets are replaced, the value of any
/// credential-looking option is masked whether attached with `=` or
/// given as the next token, a URL loses its userinfo, query, and
/// fragment, and the result is capped. The text is never reparsed into
/// arguments or run.
pub fn redact_target(text: &str, secrets: &[Secret]) -> String {
    let text = redact_text(text, secrets);
    let mut out: Vec<String> = Vec::new();
    let mut value_next = false;
    for token in text.split_whitespace() {
        if value_next {
            out.push(REDACTED.to_owned());
            value_next = false;
        } else if token.contains("://") {
            out.push(redact_url(token));
        } else if crate::is_credential_option(token) {
            match token.split_once('=') {
                Some((name, _)) => out.push(format!("{name}={REDACTED}")),
                None => {
                    out.push(token.to_owned());
                    value_next = true;
                }
            }
        } else {
            out.push(token.to_owned());
        }
    }
    cap(&out.join(" "), TARGET_MAX)
}

/// A URL without its userinfo, query, or fragment: the parts a token or
/// key travels in.
pub fn redact_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_owned();
    };
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let strip_userinfo = |authority: &str| {
        authority
            .rsplit_once('@')
            .map(|(_, host)| host.to_owned())
            .unwrap_or_else(|| authority.to_owned())
    };
    let rest = match rest.split_once('/') {
        Some((authority, path)) => format!("{}/{path}", strip_userinfo(authority)),
        None => strip_userinfo(rest),
    };
    format!("{scheme}://{rest}")
}

/// Keeps a raw line short enough for a diagnostic without truncating in
/// the middle of a multi-byte character.
pub fn cap(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... ({} bytes)", &text[..end], text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longer_secrets_go_first() {
        let secrets = [Secret::new("ab1"), Secret::new("xab1yz")];
        assert_eq!(
            redact_text("k=xab1yz and ab1", &secrets),
            "k=[redacted] and [redacted]"
        );
        let value = redact_value(serde_json::json!({"xab1yz": ["ab1"]}), &secrets);
        assert_eq!(value, serde_json::json!({"[redacted]": ["[redacted]"]}));
    }

    #[test]
    fn targets_lose_credential_values_and_url_secrets() {
        let secrets = [Secret::new("sk-known-secret")];
        assert_eq!(
            redact_target("/bin/server --api-key sk-fake-123 --port 8080", &[]),
            "/bin/server --api-key [redacted] --port 8080"
        );
        assert_eq!(
            redact_target("npx thing --token=abc", &[]),
            "npx thing --token=[redacted]"
        );
        assert_eq!(
            redact_target("server --header sk-known-secret", &secrets),
            "server --header [redacted]"
        );
        assert_eq!(
            redact_url("https://user:pass@example.invalid/mcp?token=abc#frag"),
            "https://example.invalid/mcp"
        );
        assert_eq!(
            redact_target("https://example.invalid/mcp/v1", &[]),
            "https://example.invalid/mcp/v1"
        );
        assert_eq!(redact_url("not a url"), "not a url");
        let long = format!("cmd {}", "x".repeat(2000));
        assert!(redact_target(&long, &[]).len() < 600);
    }

    #[test]
    fn cap_respects_char_boundaries() {
        assert_eq!(cap("abc", 10), "abc");
        assert!(cap("héllo wörld", 2).starts_with("h..."));
    }
}
