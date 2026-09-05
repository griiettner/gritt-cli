//! The one place that decides whether an explicit reasoning effort can be
//! sent for a model on a protocol. Adapters call it before a request;
//! the harness calls it while validating a session draft, so both refuse
//! the same cases for the same typed reason.
//!
//! The rule per protocol:
//!
//! - Responses documents `reasoning.effort`, so the level is sent whenever
//!   the model list does not say otherwise.
//! - Chat Completions has no protocol-level effort field; the OpenRouter
//!   form (`reasoning.effort`) is sent only when the provider's list reports
//!   reasoning support for the model. Unknown support is not a safe
//!   mapping, so an unreported model refuses an explicit level while a
//!   legacy `reasoning: true` request keeps its current behavior.
//! - Messages has no representation that is safe for every model: the
//!   `thinking` budget is rejected by newer models and `output_config.effort`
//!   by older ones, and Anthropic's list reports no capability flags. With
//!   nothing to route on but the model name, every explicit level is
//!   refused.
//!
//! When a list names explicit levels (`reasoning_efforts`), only those are
//! accepted on any protocol.

use gritt_core::provider::{EffortUnsupportedReason, ModelCapabilities, Protocol, ReasoningEffort};
use gritt_core::Error;

/// Whether an explicit effort can be sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffortSupport {
    Supported,
    Unsupported(EffortUnsupportedReason),
}

impl EffortSupport {
    pub fn is_supported(&self) -> bool {
        matches!(self, EffortSupport::Supported)
    }
}

/// Decides for one explicit level. `Auto` is always supported because it
/// sends nothing.
pub fn effort_support(
    protocol: Protocol,
    capabilities: Option<&ModelCapabilities>,
    effort: ReasoningEffort,
) -> EffortSupport {
    if !effort.is_explicit() {
        return EffortSupport::Supported;
    }
    if capabilities.and_then(|c| c.reasoning) == Some(false) {
        return EffortSupport::Unsupported(EffortUnsupportedReason::ReasoningUnsupported);
    }
    match protocol {
        Protocol::Messages => {
            return EffortSupport::Unsupported(EffortUnsupportedReason::Protocol { protocol });
        }
        Protocol::ChatCompletions => {
            if capabilities.and_then(|c| c.reasoning) != Some(true) {
                return EffortSupport::Unsupported(EffortUnsupportedReason::ReasoningNotReported);
            }
        }
        Protocol::Responses => {}
    }
    if let Some(offered) = capabilities.and_then(|c| c.reasoning_efforts.as_ref()) {
        if !offered.contains(&effort) {
            return EffortSupport::Unsupported(EffortUnsupportedReason::LevelNotOffered {
                offered: offered.clone(),
            });
        }
    }
    EffortSupport::Supported
}

/// The typed error an adapter returns for an unsupported level. The reason
/// travels in the diagnostic so callers can match on it.
pub fn unsupported_effort_error(
    model: &str,
    effort: ReasoningEffort,
    reason: &EffortUnsupportedReason,
) -> Error {
    Error::unsupported_capability(model, &reason.describe()).with_diagnostic(serde_json::json!({
        "effort": effort,
        "unsupported_effort": reason,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(reasoning: Option<bool>, levels: Option<Vec<ReasoningEffort>>) -> ModelCapabilities {
        ModelCapabilities {
            reasoning,
            reasoning_efforts: levels,
            ..Default::default()
        }
    }

    #[test]
    fn auto_is_always_supported() {
        for protocol in [
            Protocol::ChatCompletions,
            Protocol::Responses,
            Protocol::Messages,
        ] {
            assert!(effort_support(protocol, None, ReasoningEffort::Auto).is_supported());
        }
    }

    #[test]
    fn responses_sends_levels_unless_the_list_says_otherwise() {
        assert!(effort_support(Protocol::Responses, None, ReasoningEffort::High).is_supported());
        assert_eq!(
            effort_support(
                Protocol::Responses,
                Some(&caps(Some(false), None)),
                ReasoningEffort::High
            ),
            EffortSupport::Unsupported(EffortUnsupportedReason::ReasoningUnsupported)
        );
        assert_eq!(
            effort_support(
                Protocol::Responses,
                Some(&caps(None, Some(vec![ReasoningEffort::Low]))),
                ReasoningEffort::High
            ),
            EffortSupport::Unsupported(EffortUnsupportedReason::LevelNotOffered {
                offered: vec![ReasoningEffort::Low]
            })
        );
    }

    #[test]
    fn chat_completions_needs_reported_reasoning_support() {
        assert_eq!(
            effort_support(Protocol::ChatCompletions, None, ReasoningEffort::Low),
            EffortSupport::Unsupported(EffortUnsupportedReason::ReasoningNotReported)
        );
        assert_eq!(
            effort_support(
                Protocol::ChatCompletions,
                Some(&caps(None, None)),
                ReasoningEffort::Low
            ),
            EffortSupport::Unsupported(EffortUnsupportedReason::ReasoningNotReported)
        );
        assert!(effort_support(
            Protocol::ChatCompletions,
            Some(&caps(Some(true), None)),
            ReasoningEffort::Low
        )
        .is_supported());
    }

    #[test]
    fn messages_refuses_every_explicit_level_by_protocol() {
        let reason = match effort_support(
            Protocol::Messages,
            Some(&caps(Some(true), None)),
            ReasoningEffort::Medium,
        ) {
            EffortSupport::Unsupported(reason) => reason,
            EffortSupport::Supported => panic!("expected refusal"),
        };
        let error = unsupported_effort_error("claude-x", ReasoningEffort::Medium, &reason);
        assert_eq!(error.kind, gritt_core::ErrorKind::UnsupportedCapability);
        assert!(error.message.contains("Messages"));
        let diagnostic = error.diagnostic.unwrap();
        assert_eq!(diagnostic["effort"], "medium");
        assert_eq!(diagnostic["unsupported_effort"]["reason"], "protocol");
    }
}
