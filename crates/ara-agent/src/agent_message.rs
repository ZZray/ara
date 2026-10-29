//! Lossless Agent message records. Fixed OMP AgentMessage/context semantics:
//! 596f2da7101178214aa27a753529d15e6b7ad91d (MIT; THIRD_PARTY_NOTICES.md).
//!
//! A clone shares the record, not an array or a provider fragment. Hosts own the
//! opaque source; Core stores no journal paths, entry IDs or product identity.

use crate::aside::{AsideCallbacks, AsideError, callback};
use ara_ai::{Message, StopReason};
use ara_rpc::{WireString, value_heap::ValueHandle};
use serde::{Serialize, Serializer};
use serde_json::Value;

#[path = "agent_message_graph.rs"]
mod graph;
pub(crate) use graph::StagedGraphPreparation;
pub use graph::{CurrentArgumentsSlot, GraphAdmissionError, GraphOwnerError};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, RwLock, Weak};

// Preserve the public owned body representation without adding a new allocation boundary.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum AgentMessageBody {
    Model(Message),
    /// Validated known roles keep their original JSON, including extensions.
    RawJson(Value),
    /// Actual shared-carrier root; legacy projections are explicit checked boundaries.
    Graph(ValueHandle),
}

impl PartialEq for AgentMessageBody {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Model(left), Self::Model(right)) => left == right,
            (Self::RawJson(left), Self::RawJson(right)) => left == right,
            (Self::Graph(left), Self::Graph(right)) => left.same_identity(right),
            _ => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawContextError(pub String);

impl std::fmt::Display for RawContextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for RawContextError {}

#[derive(Clone, Debug, PartialEq, Eq)]
struct OwnedExecutionBoundary {
    role: WireString,
    calls: Vec<(WireString, WireString)>,
    runnable: Option<bool>,
    result: Option<(WireString, WireString)>,
}

impl OwnedExecutionBoundary {
    /// Known typed messages have structural facts without any graph projection.
    fn from_model(message: &Message) -> Self {
        let mut boundary = Self { role: message.role().into(), calls: Vec::new(), runnable: None, result: None };
        match message {
            Message::Assistant(message) => {
                boundary.calls =
                    message.tool_calls().map(|call| (call.id.as_str().into(), call.name.as_str().into())).collect();
                if !boundary.calls.is_empty() {
                    boundary.runnable = Some(matches!(message.stop_reason, StopReason::ToolUse | StopReason::Stop));
                }
            }
            Message::ToolResult(result) => {
                boundary.result = Some((result.tool_call_id.as_str().into(), result.tool_name.as_str().into()))
            }
            _ => {}
        }
        boundary
    }
}

struct AgentMessageRecord {
    body: AgentMessageBody,
    owned_boundary: Option<OwnedExecutionBoundary>,
    derivation_generation: u64,
    // Ephemeral Core terminal -> retained-root provenance. Ordinary Host
    // constructors and public replacements cannot manufacture this receipt.
    stream_terminal_origin: Option<StreamTerminalOrigin>,
}
struct StreamTerminalOrigin {
    source: ValueHandle,
    retained: ValueHandle,
}
struct AgentMessageCell {
    record: RwLock<AgentMessageRecord>,
    source: Option<Arc<Value>>,
    aside_callbacks: RwLock<AsideCallbacks>,
}

#[derive(Clone)]
pub struct AgentMessage(Arc<AgentMessageCell>);

/// Allocation identity for a Host weak cache. Never a durable Session ID.
#[derive(Clone)]
pub struct AgentMessageIdentity(Weak<AgentMessageCell>);

impl AgentMessageIdentity {
    pub fn upgrade(&self) -> Option<AgentMessage> {
        self.0.upgrade().map(AgentMessage)
    }
}
impl PartialEq for AgentMessageIdentity {
    fn eq(&self, other: &Self) -> bool {
        Weak::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for AgentMessageIdentity {}
impl Hash for AgentMessageIdentity {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.as_ptr().hash(state);
    }
}
impl std::fmt::Debug for AgentMessageIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentMessageIdentity").field("alive", &self.0.strong_count().ne(&0)).finish()
    }
}

impl AgentMessageBody {
    pub fn json(&self) -> Result<Value, RawContextError> {
        match self {
            Self::Model(message) => serde_json::to_value(message)
                .map_err(|error| RawContextError(format!("Agent message JSON projection failed: {error}"))),
            Self::RawJson(value) => Ok(value.clone()),
            Self::Graph(root) => root.heap().export_serde(root).map_err(|error| {
                RawContextError(format!("Graph Agent message checked JSON projection failed: {error}"))
            }),
        }
    }
    pub fn checked_model_snapshot(&self) -> Result<Option<Message>, RawContextError> {
        match self {
            Self::Graph(_) => {
                Err(RawContextError("Graph Agent message requires an actual Graph typed consumer".into()))
            }
            Self::Model(message) => Ok(Some(message.clone())),
            Self::RawJson(value) => {
                let role = value
                    .as_object()
                    .and_then(|body| body.get("role"))
                    .and_then(Value::as_str)
                    .filter(|role| !role.is_empty())
                    .ok_or_else(|| RawContextError("Agent message requires an object and nonempty role".into()))?;
                if matches!(role, "user" | "developer" | "assistant" | "toolResult") {
                    serde_json::from_value(value.clone())
                        .map(Some)
                        .map_err(|error| RawContextError(format!("Malformed {role} Agent message: {error}")))
                } else {
                    Ok(None)
                }
            }
        }
    }
    fn execution_boundary(&self) -> Result<OwnedExecutionBoundary, RawContextError> {
        match self {
            Self::Model(message) => Ok(OwnedExecutionBoundary::from_model(message)),
            Self::Graph(_) => Err(RawContextError("Graph execution boundary requires record-to-heap capture".into())),
            Self::RawJson(value) => {
                let model = self.checked_model_snapshot()?;
                let role = value.get("role").and_then(Value::as_str).ok_or_else(|| {
                    RawContextError("Agent message execution boundary requires a validated role".into())
                })?;
                Ok(match model {
                    Some(message) => OwnedExecutionBoundary::from_model(&message),
                    None => {
                        OwnedExecutionBoundary { role: role.into(), calls: Vec::new(), runnable: None, result: None }
                    }
                })
            }
        }
    }
}

impl AgentMessage {
    pub fn from_model(message: Message) -> Self {
        Self::from_model_with_source(message, None)
    }
    pub fn from_model_with_source(message: Message, source: Option<Arc<Value>>) -> Self {
        Self::new(AgentMessageBody::Model(message), source)
    }
    /// Core's already typed synthetic records require no fallible Raw export.
    pub(crate) fn from_owned_model(message: Message, source: Option<Arc<Value>>) -> Self {
        let boundary = OwnedExecutionBoundary::from_model(&message);
        Self::new_with_boundary(AgentMessageBody::Model(message), source, Some(boundary))
    }
    pub fn from_raw(body: Value, source: Option<Arc<Value>>) -> Result<Self, RawContextError> {
        let body = AgentMessageBody::RawJson(body);
        body.checked_model_snapshot()?;
        Ok(Self::new(body, source))
    }
    fn new(body: AgentMessageBody, source: Option<Arc<Value>>) -> Self {
        Self::new_with_boundary(body, source, None)
    }
    fn new_with_boundary(
        body: AgentMessageBody,
        source: Option<Arc<Value>>,
        owned_boundary: Option<OwnedExecutionBoundary>,
    ) -> Self {
        Self(Arc::new(AgentMessageCell {
            source,
            aside_callbacks: RwLock::new(AsideCallbacks::default()),
            record: RwLock::new(AgentMessageRecord {
                body,
                owned_boundary,
                derivation_generation: 0,
                stream_terminal_origin: None,
            }),
        }))
    }
    /// Ephemeral Native symbol callbacks: shared by clones but never in the
    /// body, Host source, Serialize or Debug representation.
    pub fn set_aside_callbacks(&self, callbacks: AsideCallbacks) {
        *self.0.aside_callbacks.write().unwrap_or_else(|poison| poison.into_inner()) = callbacks;
    }

    pub(crate) fn commit_aside(&self) -> Result<(), AsideError> {
        let commit = self.0.aside_callbacks.read().unwrap_or_else(|poison| poison.into_inner()).commit.clone();
        if let Some(commit) = commit { callback("commit", || commit()) } else { Ok(()) }
    }

    pub(crate) fn discard_aside(&self, reason: &AsideError) -> Result<(), AsideError> {
        let discard = self.0.aside_callbacks.read().unwrap_or_else(|poison| poison.into_inner()).discard.clone();
        if let Some(discard) = discard { callback("discard", || discard(reason)) } else { Ok(()) }
    }

    pub fn body_snapshot(&self) -> AgentMessageBody {
        self.0.record.read().unwrap_or_else(|poison| poison.into_inner()).body.clone()
    }
    pub fn json(&self) -> Result<Value, RawContextError> {
        self.body_snapshot().json()
    }
    pub fn checked_model_snapshot(&self) -> Result<Option<Message>, RawContextError> {
        self.body_snapshot().checked_model_snapshot()
    }
    pub fn replace_body(&self, body: AgentMessageBody) -> Result<(), RawContextError> {
        if let AgentMessageBody::Graph(root) = &body {
            let mut live = self.0.record.write().unwrap_or_else(|poison| poison.into_inner());
            let (boundary, _) = graph::capture_boundary(root)
                .map_err(|error| RawContextError(format!("Graph replacement admission failed: {error}")))?;
            if live.owned_boundary.as_ref().is_some_and(|pinned| pinned != &boundary) {
                return Err(RawContextError("Graph replacement changed its OwnedExecutionBoundary".into()));
            }
            live.body = body;
            live.stream_terminal_origin = None;
            return Ok(());
        }
        let boundary = body.execution_boundary()?;
        let mut live = self.0.record.write().unwrap_or_else(|poison| poison.into_inner());
        if live.owned_boundary.as_ref().is_some_and(|pinned| pinned != &boundary) {
            return Err(RawContextError(
                "OwnedExecutionBoundary: role, tool identities or runnable effects require guarded owner replacement"
                    .into(),
            ));
        }
        live.body = body;
        live.stream_terminal_origin = None;
        Ok(())
    }
    pub fn same_record(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn weak_identity(&self) -> AgentMessageIdentity {
        AgentMessageIdentity(Arc::downgrade(&self.0))
    }
    pub fn source(&self) -> Option<Arc<Value>> {
        self.0.source.clone()
    }
    pub fn invalidate_derived(&self) {
        let mut live = self.0.record.write().unwrap_or_else(|poison| poison.into_inner());
        live.derivation_generation = live.derivation_generation.wrapping_add(1);
    }
    pub fn derivation_generation(&self) -> u64 {
        self.0.record.read().unwrap_or_else(|poison| poison.into_inner()).derivation_generation
    }
    /// Pin only the compact execution boundary in the same lock as its body.
    /// Already-owned records can never be unpinned by another Agent or a Host.
    pub(crate) fn adopt(&self) -> Result<(), RawContextError> {
        let mut live = self.0.record.write().unwrap_or_else(|poison| poison.into_inner());
        if matches!(&live.body, AgentMessageBody::Graph(_)) {
            return graph::admit_record(&mut live)
                .map_err(|error| RawContextError(format!("Graph adoption admission failed: {error}")));
        }
        if live.owned_boundary.is_none() {
            live.owned_boundary = Some(live.body.execution_boundary()?);
        }
        Ok(())
    }
    /// Only Core's finalized typed accumulator may install and pin a streamed
    /// Assistant. A previously adopted record still enforces its original pin.
    pub(crate) fn install_final_model(&self, message: Message) -> Result<(), RawContextError> {
        let body = AgentMessageBody::Model(message);
        let boundary = body.execution_boundary()?;
        let mut live = self.0.record.write().unwrap_or_else(|poison| poison.into_inner());
        if live.owned_boundary.as_ref().is_some_and(|pinned| pinned != &boundary) {
            return Err(RawContextError(
                "Cannot overwrite an owned execution boundary during stream finalization".into(),
            ));
        }
        live.body = body;
        live.owned_boundary = Some(boundary);
        Ok(())
    }
}

impl From<Message> for AgentMessage {
    fn from(message: Message) -> Self {
        Self::from_model(message)
    }
}
/// Explicit comparison preserves the prior record/source semantics and reports
/// projection failure. Allocation identity remains a valid fast path.
pub fn checked_record_equal(left: &AgentMessage, right: &AgentMessage) -> Result<bool, RawContextError> {
    if left.same_record(right) {
        return Ok(true);
    }
    Ok(left.json()? == right.json()? && left.0.source == right.0.source)
}

/// Typed comparison distinguishes a legitimate non-model record from a failed
/// typed boundary. A boundary error is never mapped to unequal.
pub fn checked_model_equal(record: &AgentMessage, model: &Message) -> Result<bool, RawContextError> {
    Ok(record.checked_model_snapshot()?.as_ref() == Some(model))
}

impl std::fmt::Debug for AgentMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentMessage")
            .field("body", &self.body_snapshot())
            .field("has_source", &self.0.source.is_some())
            .finish()
    }
}
impl Serialize for AgentMessage {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.json().map_err(serde::ser::Error::custom)?.serialize(serializer)
    }
}

/// Read-only typed convenience view; never used as a replacement transcript.
pub fn checked_model_messages(messages: &[AgentMessage]) -> Result<Vec<Message>, RawContextError> {
    messages.iter().filter_map(|message| message.checked_model_snapshot().transpose()).collect()
}

/// Native Agent default: User/ToolResult/non-refusal Assistant. The coding-agent
/// Host converter separately supplies Developer and application roles.
pub fn default_agent_message_conversion(messages: &[AgentMessage]) -> Result<Vec<Message>, RawContextError> {
    Ok(checked_model_messages(messages)?
        .into_iter()
        .filter(|message| match message {
            Message::User(_) | Message::ToolResult(_) => true,
            Message::Assistant(assistant) => {
                !(assistant.stop_reason == StopReason::Error
                    && assistant
                        .stop_details
                        .as_ref()
                        .and_then(|details| details.get("type"))
                        .and_then(Value::as_str)
                        .is_some_and(|kind| matches!(kind, "refusal" | "sensitive")))
            }
            Message::Developer(_) => false,
        })
        .collect())
}

/// The legacy context admission adapter must not invent a typed transcript by
/// silently filtering application records that its callback cannot understand.
pub(crate) fn typed_admission_context(messages: &[AgentMessage]) -> Result<Vec<Message>, RawContextError> {
    messages
        .iter()
        .map(|message| {
            message
                .checked_model_snapshot()?
                .ok_or_else(|| RawContextError("Host context admission requires its raw AgentMessage port".into()))
        })
        .collect()
}
