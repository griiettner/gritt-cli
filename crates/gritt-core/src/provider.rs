//! Provider profile, model list, and adapter contracts (ADR-007, ADR-008).

use std::collections::BTreeMap;
use std::pin::Pin;

use chrono::{DateTime, Utc};
use futures_core::Stream;
use serde::{Deserialize, Serialize};

use crate::event::Event;
use crate::secret::SecretRef;
use crate::session::{BoxFuture, ContinuationState};
use crate::tool::{ToolDefinition, ToolResult};
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    /// OpenAI-compatible Chat Completions. Serves OpenRouter, OpenAI in Chat
    /// Completions mode, and any generic endpoint.
    ChatCompletions,
    /// OpenAI Responses with `previous_response_id` continuation.
    Responses,
    /// Anthropic Messages.
    Messages,
}

/// A configured endpoint. Routing is by profile, never by model name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderProfile {
    pub name: String,
    pub protocol: Protocol,
    pub base_url: String,
    pub key: SecretRef,
    /// Alias to model id, scoped to this profile.
    #[serde(default)]
    pub aliases: BTreeMap<String, String>,
}

/// User-facing reasoning intensity for one native turn. Provider neutral:
/// each adapter maps a level to its own wire field, or refuses it with a
/// typed unsupported-capability error when the protocol has no safe
/// mapping. `Auto` means "model default": no explicit effort is sent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    #[default]
    Auto,
    Low,
    Medium,
    High,
}

impl ReasoningEffort {
    /// Every level, `Auto` first.
    pub const ALL: [ReasoningEffort; 4] = [
        ReasoningEffort::Auto,
        ReasoningEffort::Low,
        ReasoningEffort::Medium,
        ReasoningEffort::High,
    ];

    /// The levels that name an explicit provider setting.
    pub const EXPLICIT: [ReasoningEffort; 3] = [
        ReasoningEffort::Low,
        ReasoningEffort::Medium,
        ReasoningEffort::High,
    ];

    /// The serde name, which is also the user-facing spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            ReasoningEffort::Auto => "auto",
            ReasoningEffort::Low => "low",
            ReasoningEffort::Medium => "medium",
            ReasoningEffort::High => "high",
        }
    }

    pub fn is_explicit(self) -> bool {
        self != ReasoningEffort::Auto
    }
}

impl std::fmt::Display for ReasoningEffort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ReasoningEffort {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        ReasoningEffort::ALL
            .into_iter()
            .find(|level| level.as_str().eq_ignore_ascii_case(text.trim()))
            .ok_or_else(|| {
                Error::config(format!(
                    "unknown effort `{text}`; use auto, low, medium, or high"
                ))
            })
    }
}

/// Why an explicit effort cannot be sent for a model on a protocol. Typed so
/// an interface can explain the refusal without parsing an error message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum EffortUnsupportedReason {
    /// The protocol has no request field that safely carries the level.
    Protocol { protocol: Protocol },
    /// The provider's list reports the model without reasoning support.
    ReasoningUnsupported,
    /// The protocol needs the provider's list to report reasoning support
    /// for the model before the level can be sent, and it does not.
    ReasoningNotReported,
    /// The provider's list names explicit levels and this is not one.
    LevelNotOffered { offered: Vec<ReasoningEffort> },
}

impl EffortUnsupportedReason {
    /// One line for the human-readable error message.
    pub fn describe(&self) -> String {
        match self {
            EffortUnsupportedReason::Protocol { protocol } => format!(
                "explicit reasoning effort on the {} protocol",
                match protocol {
                    Protocol::ChatCompletions => "Chat Completions",
                    Protocol::Responses => "Responses",
                    Protocol::Messages => "Messages",
                }
            ),
            EffortUnsupportedReason::ReasoningUnsupported => "reasoning".to_owned(),
            EffortUnsupportedReason::ReasoningNotReported => {
                "reasoning (the model list does not report it, so no explicit effort can be sent)"
                    .to_owned()
            }
            EffortUnsupportedReason::LevelNotOffered { offered } => format!(
                "that reasoning effort (the model list offers {})",
                offered
                    .iter()
                    .map(|level| level.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelCapabilities {
    pub context_length: Option<u64>,
    pub tools: Option<bool>,
    pub vision: Option<bool>,
    pub structured_output: Option<bool>,
    pub reasoning: Option<bool>,
    /// Price per million input tokens, when reported.
    pub input_price_per_million: Option<f64>,
    pub output_price_per_million: Option<f64>,
    /// The explicit effort levels the provider's list reports for the
    /// model. `None` when the provider does not report them, which is the
    /// case for every list Gritt parses today; the gap is never filled by
    /// inferring levels from a model name. `Auto` never appears here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_efforts: Option<Vec<ReasoningEffort>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: Option<String>,
    #[serde(default)]
    pub capabilities: ModelCapabilities,
    /// Provider-declared replacement for a deprecated model, when any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced_by: Option<String>,
    #[serde(default)]
    pub deprecated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ModelListStatus {
    Fresh {
        fetched_at: DateTime<Utc>,
    },
    /// The refresh failed and the last cached list is in use.
    Stale {
        fetched_at: DateTime<Utc>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelList {
    pub profile: String,
    pub status: ModelListStatus,
    pub models: Vec<ModelInfo>,
}

/// One turn of input to a provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
    #[serde(default)]
    pub options: RequestOptions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

/// Per-request settings. `effort` is the typed reasoning contract; the
/// older `reasoning` switch is kept so stored continuation state and older
/// callers keep working. See [`RequestOptions::reasoning_intent`] for how
/// the two combine.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestOptions {
    pub max_tokens: Option<u32>,
    /// Legacy switch: `Some(true)` asks for reasoning at the provider's
    /// default level, `Some(false)` or `None` asks for nothing.
    pub reasoning: Option<bool>,
    pub structured_output: Option<serde_json::Value>,
    /// Reasoning intensity. Absent in data written before this field
    /// existed, which deserializes as `Auto`.
    #[serde(default)]
    pub effort: ReasoningEffort,
}

/// What a request asks of the provider's reasoning, after combining the
/// legacy `reasoning` switch with `effort`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningIntent {
    /// Send nothing; the model decides.
    Default,
    /// Legacy `reasoning: Some(true)` with `effort: Auto`: reasoning on at
    /// the provider's default level, no explicit level sent.
    Enabled,
    /// `effort` names a level. Sent through the adapter's mapping, or
    /// refused before the request when the protocol has no safe one.
    Explicit(ReasoningEffort),
}

impl RequestOptions {
    /// Combines `reasoning` and `effort`:
    ///
    /// | `reasoning`   | `effort` | intent                       |
    /// | ------------- | -------- | ---------------------------- |
    /// | `None`/`false`| `Auto`   | `Default`                    |
    /// | `Some(true)`  | `Auto`   | `Enabled` (legacy behavior)  |
    /// | `None`/`true` | level    | `Explicit(level)`            |
    /// | `Some(false)` | level    | error: contradictory request |
    ///
    /// The last row is a caller bug, not a provider limit, so it is a
    /// `Config` error rather than `UnsupportedCapability`.
    pub fn reasoning_intent(&self) -> Result<ReasoningIntent> {
        match (self.reasoning, self.effort) {
            (_, ReasoningEffort::Auto) => Ok(if self.reasoning == Some(true) {
                ReasoningIntent::Enabled
            } else {
                ReasoningIntent::Default
            }),
            (Some(false), level) => Err(Error::config(format!(
                "request asks for reasoning effort `{level}` while reasoning is disabled"
            ))),
            (_, level) => Ok(ReasoningIntent::Explicit(level)),
        }
    }
}

pub type EventStream<'a> = Pin<Box<dyn Stream<Item = Result<Event>> + Send + 'a>>;

/// The one trait every wire protocol implements. Nothing above it learns
/// which provider served a request.
pub trait ProviderAdapter: Send + Sync {
    fn protocol(&self) -> Protocol;
    /// Sends a prompt and streams provider-neutral events.
    fn send(&self, request: PromptRequest) -> BoxFuture<'_, Result<EventStream<'_>>>;
    /// Submits tool results for outstanding tool calls and streams the
    /// continuation.
    fn submit_tool_results(
        &self,
        results: Vec<ToolResult>,
    ) -> BoxFuture<'_, Result<EventStream<'_>>>;
    /// Restores adapter state from stored continuation data.
    fn restore(&self, state: ContinuationState) -> BoxFuture<'_, Result<()>>;
    /// Exports the state needed to continue later.
    fn continuation(&self) -> BoxFuture<'_, Result<Option<ContinuationState>>>;
    fn capabilities(&self, model: &str) -> BoxFuture<'_, Result<ModelCapabilities>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effort_serde_names_are_the_user_facing_spellings() {
        for level in ReasoningEffort::ALL {
            let json = serde_json::to_string(&level).unwrap();
            assert_eq!(json, format!("\"{}\"", level.as_str()));
            assert_eq!(
                serde_json::from_str::<ReasoningEffort>(&json).unwrap(),
                level
            );
            assert_eq!(level.as_str().parse::<ReasoningEffort>().unwrap(), level);
        }
        assert_eq!(
            " HIGH ".parse::<ReasoningEffort>().unwrap(),
            ReasoningEffort::High
        );
        let error = "max".parse::<ReasoningEffort>().unwrap_err();
        assert_eq!(error.kind, crate::ErrorKind::Config);
        assert!(!ReasoningEffort::EXPLICIT.contains(&ReasoningEffort::Auto));
    }

    #[test]
    fn request_options_written_before_effort_still_deserialize() {
        let old = serde_json::json!({
            "max_tokens": 100, "reasoning": true, "structured_output": null
        });
        let options: RequestOptions = serde_json::from_value(old).unwrap();
        assert_eq!(options.effort, ReasoningEffort::Auto);
        assert_eq!(options.reasoning, Some(true));
        assert_eq!(options.max_tokens, Some(100));
        let round = serde_json::to_value(&options).unwrap();
        assert_eq!(round["effort"], "auto");
        assert_eq!(
            serde_json::from_value::<RequestOptions>(round).unwrap(),
            options
        );
    }

    #[test]
    fn legacy_reasoning_switch_and_effort_combine_as_documented() {
        let intent = |reasoning: Option<bool>, effort: ReasoningEffort| {
            RequestOptions {
                reasoning,
                effort,
                ..Default::default()
            }
            .reasoning_intent()
        };
        assert_eq!(
            intent(None, ReasoningEffort::Auto).unwrap(),
            ReasoningIntent::Default
        );
        assert_eq!(
            intent(Some(false), ReasoningEffort::Auto).unwrap(),
            ReasoningIntent::Default
        );
        assert_eq!(
            intent(Some(true), ReasoningEffort::Auto).unwrap(),
            ReasoningIntent::Enabled
        );
        assert_eq!(
            intent(None, ReasoningEffort::Low).unwrap(),
            ReasoningIntent::Explicit(ReasoningEffort::Low)
        );
        assert_eq!(
            intent(Some(true), ReasoningEffort::High).unwrap(),
            ReasoningIntent::Explicit(ReasoningEffort::High)
        );
        let error = intent(Some(false), ReasoningEffort::Medium).unwrap_err();
        assert_eq!(error.kind, crate::ErrorKind::Config);
        assert!(error.message.contains("medium"));
    }

    #[test]
    fn capabilities_without_effort_levels_round_trip_unchanged() {
        let old = serde_json::json!({
            "context_length": 1000, "tools": true, "vision": null,
            "structured_output": null, "reasoning": null,
            "input_price_per_million": null, "output_price_per_million": null
        });
        let capabilities: ModelCapabilities = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(capabilities.reasoning_efforts, None);
        assert_eq!(serde_json::to_value(&capabilities).unwrap(), old);
        let with_levels = ModelCapabilities {
            reasoning_efforts: Some(vec![ReasoningEffort::Low, ReasoningEffort::High]),
            ..Default::default()
        };
        let json = serde_json::to_value(&with_levels).unwrap();
        assert_eq!(
            json["reasoning_efforts"],
            serde_json::json!(["low", "high"])
        );
        let reason = EffortUnsupportedReason::LevelNotOffered {
            offered: vec![ReasoningEffort::Low],
        };
        assert_eq!(
            serde_json::to_value(&reason).unwrap()["reason"],
            "level_not_offered"
        );
        assert!(reason.describe().contains("low"));
    }
}
