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
    fn cap_respects_char_boundaries() {
        assert_eq!(cap("abc", 10), "abc");
        assert!(cap("héllo wörld", 2).starts_with("h..."));
    }
}
