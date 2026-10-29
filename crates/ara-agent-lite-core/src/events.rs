//! Versioned typed view of the JSON events `execute` sends (requirement 28).
//!
//! `execute` still sends `serde_json::Value`; this enum is an additive
//! decoding of those values. Each known `type` maps to one named variant
//! whose struct holds the known fields plus `extra` for unknown ones, so a
//! decode and re-encode returns JSON equal to the input. An unknown `type`
//! keeps the complete original JSON in [`LiteEvent::Unknown`]. Field types
//! follow the samples in `tests/fixtures/events-v1/` captured before this
//! module existed (DR-P4-1). Nested payloads owned by other modules (Goal,
//! budgets, stop evidence, telemetry) stay JSON objects.
use std::fmt;

use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

/// Version of the [`LiteEvent`] schema. Incremented when a named variant or
/// one of its fields is added, removed or renamed. Never written into events.
pub const EVENT_SCHEMA_VERSION: u32 = 1;

/// A JSON object field.
pub type Object = Map<String, Value>;

/// Optional field: absent when `None`; an explicit `null` is a type error.
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(deserializer: D) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

/// Required field whose value may be `null`; absence is an error.
fn nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(deserializer: D) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(deserializer)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentStart {
    pub status: String,
    #[serde(flatten)]
    pub extra: Object,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentEnd {
    pub status: String,
    pub output: String,
    pub detail: String,
    pub code: i64,
    /// Closed-budget checkpoint proof (`termination::ClosedBudgetTermination`).
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    pub termination: Option<Object>,
    /// Typed stop evidence (`termination::StopEvidence`).
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    pub stop_evidence: Option<Object>,
    /// Last Completion_Gate check of a failed Run.
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    pub completion_gate: Option<Object>,
    #[serde(flatten)]
    pub extra: Object,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextDelta {
    pub output: String,
    #[serde(flatten)]
    pub extra: Object,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Text {
    pub output: String,
    #[serde(flatten)]
    pub extra: Object,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolStart {
    pub status: String,
    pub step_id: String,
    pub title: String,
    pub detail: String,
    #[serde(flatten)]
    pub extra: Object,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolEnd {
    pub step_id: String,
    pub title: String,
    pub status: String,
    pub output: String,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    pub effects_unknown: Option<bool>,
    #[serde(flatten)]
    pub extra: Object,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelCapabilities {
    pub source: String,
    pub model: String,
    pub declared: Object,
    pub text_budget: Object,
    pub budget_unit: String,
    pub host_input: Vec<String>,
    #[serde(flatten)]
    pub extra: Object,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GenerationPolicyEvent {
    pub source: String,
    pub model: String,
    pub protocol: String,
    pub policy: String,
    pub requested_output_tokens: u64,
    pub effective_output_tokens: u64,
    pub output_budget_rule: String,
    pub output_limit_sources: Vec<String>,
    pub provider_effect_verified: bool,
    #[serde(flatten)]
    pub extra: Object,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelCall {
    pub phase: String,
    pub call: u64,
    pub usage: String,
    pub status: String,
    /// Agent calls only.
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    pub context_observation: Option<Object>,
    pub output_limit: Object,
    #[serde(flatten)]
    pub extra: Object,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelResult {
    pub phase: String,
    pub call: u64,
    pub status: String,
    pub stop_reason: String,
    pub usage: String,
    pub usage_metrics: Object,
    /// Agent calls only.
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    pub elapsed_ms: Option<u64>,
    /// Agent calls only.
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    pub title: Option<String>,
    /// Summary calls only.
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    pub content_types: Option<Vec<String>>,
    /// Summary calls only.
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    pub provider_error_present: Option<bool>,
    #[serde(flatten)]
    pub extra: Object,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContextStatus {
    pub status: String,
    pub projection_json_bytes: u64,
    pub projection_limit_bytes: u64,
    pub unit: String,
    pub near_threshold: bool,
    pub source: String,
    #[serde(flatten)]
    pub extra: Object,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Compact {
    pub status: String,
    pub title: String,
    pub covered_prefix: u64,
    pub raw_messages: u64,
    pub index_unit: String,
    pub summary_model_calls: u64,
    #[serde(flatten)]
    pub extra: Object,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LengthRecovery {
    pub status: String,
    pub policy: String,
    pub source_raw_index: u64,
    pub source_sha256: String,
    pub source_hash_format: String,
    pub source_text_utf8_bytes: u64,
    pub source_thinking_utf8_bytes: u64,
    pub reference_id: String,
    pub reference_sha256: String,
    pub run_id: String,
    pub source_call: u64,
    pub calls_used: u64,
    pub max_calls: u64,
    pub effective_output_tokens: u64,
    pub next_request_reserved: bool,
    pub source: String,
    pub new_human_authorization: bool,
    #[serde(flatten)]
    pub extra: Object,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GoalStatus {
    /// `execution::GoalState`.
    pub goal: Object,
    #[serde(flatten)]
    pub extra: Object,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GoalUpdate {
    /// `execution::GoalState`.
    pub goal: Object,
    #[serde(flatten)]
    pub extra: Object,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskUpdate {
    pub step_id: String,
    pub output: String,
    pub detail: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    pub evidence: Option<String>,
    #[serde(flatten)]
    pub extra: Object,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompletionGateEvent {
    pub check_id: String,
    pub sequence: u64,
    pub result: String,
    /// Always present; `null` when the check passed.
    #[serde(deserialize_with = "nullable")]
    pub feedback_ref: Option<String>,
    pub calls_used: u64,
    #[serde(flatten)]
    pub extra: Object,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StuckDetectorEvent {
    pub action: String,
    pub rule: String,
    pub count: u64,
    pub threshold: u64,
    pub native_call_ids: Vec<String>,
    pub calls_used: u64,
    #[serde(flatten)]
    pub extra: Object,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompactionRefreshEvent {
    /// One `{path, result, sha256}` object per refreshed file.
    pub files: Vec<Object>,
    pub bytes: u64,
    pub plan_state: bool,
    #[serde(flatten)]
    pub extra: Object,
}

/// Why a JSON value is not a valid [`LiteEvent`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventDecodeError {
    /// No `type` field (including a value that is not a JSON object).
    MissingType,
    /// `type` is present but not a string.
    TypeNotString,
    /// A known `type` with a missing required field or a field of the wrong JSON type.
    Field { event_type: String, detail: String },
}

impl fmt::Display for EventDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingType => f.write_str("event has no `type` field"),
            Self::TypeNotString => f.write_str("event `type` is not a string"),
            Self::Field { event_type, detail } => write!(f, "invalid `{event_type}` event: {detail}"),
        }
    }
}

impl std::error::Error for EventDecodeError {}

/// Serializes `body` with `type` first; `body` never holds a `type` key
/// when it came from decoding.
#[derive(Serialize)]
struct Tagged<'a, T> {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(flatten)]
    body: &'a T,
}

macro_rules! lite_events {
    ($($variant:ident($body:ident) = $name:literal,)*) => {
        /// One event sent by `execute`, decoded by its `type`.
        #[derive(Clone, Debug, PartialEq)]
        pub enum LiteEvent {
            $($variant($body),)*
            /// Unknown `type`; the complete original JSON is kept.
            Unknown(Value),
        }

        impl LiteEvent {
            /// Every `type` value with a named variant, in declaration order.
            pub const KNOWN_TYPES: &'static [&'static str] = &[$($name,)*];

            /// The `type` value; `None` for an unknown event whose `type` is
            /// not a string (never produced by decoding).
            pub fn event_type(&self) -> Option<&str> {
                match self {
                    $(Self::$variant(_) => Some($name),)*
                    Self::Unknown(value) => value.get("type").and_then(Value::as_str),
                }
            }
        }

        impl TryFrom<Value> for LiteEvent {
            type Error = EventDecodeError;

            fn try_from(value: Value) -> Result<Self, EventDecodeError> {
                let Value::Object(mut object) = value else { return Err(EventDecodeError::MissingType) };
                let kind = match object.get("type") {
                    None => return Err(EventDecodeError::MissingType),
                    Some(Value::String(kind)) => kind.clone(),
                    Some(_) => return Err(EventDecodeError::TypeNotString),
                };
                let field = |error: serde_json::Error| EventDecodeError::Field {
                    event_type: kind.clone(),
                    detail: error.to_string(),
                };
                match kind.as_str() {
                    $($name => {
                        object.remove("type");
                        serde_json::from_value(Value::Object(object)).map(Self::$variant).map_err(field)
                    })*
                    // The original object, `type` included, is kept untouched.
                    _ => Ok(Self::Unknown(Value::Object(object))),
                }
            }
        }

        impl Serialize for LiteEvent {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                match self {
                    $(Self::$variant(body) => Tagged { kind: $name, body }.serialize(serializer),)*
                    Self::Unknown(value) => value.serialize(serializer),
                }
            }
        }
    };
}

lite_events! {
    AgentStart(AgentStart) = "agent_start",
    AgentEnd(AgentEnd) = "agent_end",
    TextDelta(TextDelta) = "text_delta",
    Text(Text) = "text",
    ToolStart(ToolStart) = "tool_start",
    ToolEnd(ToolEnd) = "tool_end",
    ModelCapabilities(ModelCapabilities) = "model_capabilities",
    GenerationPolicy(GenerationPolicyEvent) = "generation_policy",
    ModelCall(ModelCall) = "model_call",
    ModelResult(ModelResult) = "model_result",
    ContextStatus(ContextStatus) = "context_status",
    Compact(Compact) = "compact",
    LengthRecovery(LengthRecovery) = "length_recovery",
    GoalStatus(GoalStatus) = "goal_status",
    GoalUpdate(GoalUpdate) = "goal_update",
    TaskUpdate(TaskUpdate) = "task_update",
    CompletionGate(CompletionGateEvent) = "completion_gate",
    StuckDetector(StuckDetectorEvent) = "stuck_detector",
    CompactionRefresh(CompactionRefreshEvent) = "compaction_refresh",
}

impl<'de> Deserialize<'de> for LiteEvent {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::try_from(Value::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

impl From<LiteEvent> for Value {
    fn from(event: LiteEvent) -> Self {
        match event {
            LiteEvent::Unknown(value) => value,
            // Only string-keyed maps, strings, integers, booleans and JSON
            // values are serialized, which `serde_json` cannot fail on.
            known => serde_json::to_value(&known).expect("LiteEvent fields serialize to JSON"),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn decode_dispatches_by_type_and_keeps_unknown_fields() {
        let input = json!({"type":"tool_end","step_id":"a","title":"t","status":"completed","output":"o","later":[1]});
        let event = LiteEvent::try_from(input.clone()).unwrap();
        let LiteEvent::ToolEnd(end) = &event else { panic!("{event:?}") };
        assert_eq!(end.effects_unknown, None);
        assert_eq!(end.extra["later"], json!([1]));
        assert_eq!(Value::from(event), input);
        let unknown = json!({"type":"future_kind","n":1.5});
        assert_eq!(LiteEvent::try_from(unknown.clone()).unwrap(), LiteEvent::Unknown(unknown));
    }

    #[test]
    fn decode_errors_name_the_reason() {
        assert_eq!(LiteEvent::try_from(json!({"output":"x"})), Err(EventDecodeError::MissingType));
        assert_eq!(LiteEvent::try_from(json!("text")), Err(EventDecodeError::MissingType));
        assert_eq!(LiteEvent::try_from(json!({"type":7})), Err(EventDecodeError::TypeNotString));
        let missing = LiteEvent::try_from(
            json!({"type":"completion_gate","check_id":"c","sequence":1,"result":"passed","calls_used":1}),
        );
        assert!(
            matches!(missing, Err(EventDecodeError::Field { ref event_type, ref detail })
            if event_type == "completion_gate" && detail.contains("feedback_ref")),
            "{missing:?}"
        );
        let null = LiteEvent::try_from(
            json!({"type":"task_update","step_id":"s","output":"o","detail":"done","status":"done","evidence":null}),
        );
        assert!(matches!(null, Err(EventDecodeError::Field { .. })), "{null:?}");
    }
}
