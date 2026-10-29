//! Fixed OMP 596f2da, packages/ai/src/dialect/types.ts (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See LICENSE.

/// Catalog dialect identity. Resolution belongs to the Host's fixed catalog
/// classifier; this enum does not infer a dialect from a model-name substring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Glm,
    Hermes,
    Kimi,
    Xml,
    Anthropic,
    Deepseek,
    Harmony,
    Qwen3,
    Gemini,
    Gemma,
    Minimax,
}

impl Dialect {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Glm => "glm",
            Self::Hermes => "hermes",
            Self::Kimi => "kimi",
            Self::Xml => "xml",
            Self::Anthropic => "anthropic",
            Self::Deepseek => "deepseek",
            Self::Harmony => "harmony",
            Self::Qwen3 => "qwen3",
            Self::Gemini => "gemini",
            Self::Gemma => "gemma",
            Self::Minimax => "minimax",
        }
    }
}

/// `V` is the shared lossless argument carrier supplied by the Host/Core
/// bridge. Parsing never substitutes a serde map for that carrier.
#[derive(Debug, Clone, PartialEq)]
pub enum InbandScanEvent<V, S = String, N = S> {
    Text { text: S },
    ThinkingStart,
    ThinkingDelta { delta: S },
    ThinkingEnd { thinking: S },
    ImpliedThinkingEnd,
    ToolStart { id: S, name: N },
    ToolArgDelta { id: S, name: N, key: S, delta: S },
    ToolEnd { id: S, name: N, arguments: V, raw_block: Option<S> },
}

/// Source-produced names or an actual repeated Get result. Native catches do
/// not coerce/revalidate the final Actual value after their first typeof check.
#[derive(Debug, Clone, PartialEq)]
pub enum InbandNameValue<V> {
    Source(ara_rpc::WireString),
    Actual(V),
}
pub type ActualInbandScanEvent<V> = InbandScanEvent<V, ara_rpc::WireString, InbandNameValue<V>>;

#[derive(Debug)]
pub enum InbandEventProjectionError<E> {
    Runtime(E),
    NonStringName,
    InvalidUtf16(std::string::FromUtf16Error),
}
/// Explicit restricted ARA facade, outside Native scanner catches. Actual name
/// values stay V until this caller-requested checked scalar projection.
pub fn try_project_actual_events_to_strings<F: crate::relaxed_json::FinalValueFactory>(
    events: Vec<ActualInbandScanEvent<F::Value>>,
    factory: &F,
) -> Result<Vec<InbandScanEvent<F::Value>>, InbandEventProjectionError<F::Error>> {
    fn scalar<E>(value: ara_rpc::WireString) -> Result<String, InbandEventProjectionError<E>> {
        value.to_utf8().map_err(InbandEventProjectionError::InvalidUtf16)
    }
    fn name<F: crate::relaxed_json::FinalValueFactory>(
        value: InbandNameValue<F::Value>,
        factory: &F,
    ) -> Result<String, InbandEventProjectionError<F::Error>> {
        let text = match value {
            InbandNameValue::Source(text) => text,
            InbandNameValue::Actual(value) => factory
                .string_value(&value)
                .map_err(InbandEventProjectionError::Runtime)?
                .ok_or(InbandEventProjectionError::NonStringName)?,
        };
        scalar(text)
    }
    events
        .into_iter()
        .map(|event| {
            Ok(match event {
                InbandScanEvent::Text { text } => InbandScanEvent::Text { text: scalar(text)? },
                InbandScanEvent::ThinkingStart => InbandScanEvent::ThinkingStart,
                InbandScanEvent::ThinkingDelta { delta } => InbandScanEvent::ThinkingDelta { delta: scalar(delta)? },
                InbandScanEvent::ThinkingEnd { thinking } => {
                    InbandScanEvent::ThinkingEnd { thinking: scalar(thinking)? }
                }
                InbandScanEvent::ImpliedThinkingEnd => InbandScanEvent::ImpliedThinkingEnd,
                InbandScanEvent::ToolStart { id, name: value } => {
                    InbandScanEvent::ToolStart { id: scalar(id)?, name: name(value, factory)? }
                }
                InbandScanEvent::ToolArgDelta { id, name: value, key, delta } => InbandScanEvent::ToolArgDelta {
                    id: scalar(id)?,
                    name: name(value, factory)?,
                    key: scalar(key)?,
                    delta: scalar(delta)?,
                },
                InbandScanEvent::ToolEnd { id, name: value, arguments, raw_block } => InbandScanEvent::ToolEnd {
                    id: scalar(id)?,
                    name: name(value, factory)?,
                    arguments,
                    raw_block: raw_block.map(scalar).transpose()?,
                },
            })
        })
        .collect()
}

impl<V, S, N> InbandScanEvent<V, S, N> {
    /// Change only source strings; actual names and argument identities move.
    pub fn map_source_strings<T>(self, mut map: impl FnMut(S) -> T) -> InbandScanEvent<V, T, N> {
        match self {
            Self::Text { text } => InbandScanEvent::Text { text: map(text) },
            Self::ThinkingStart => InbandScanEvent::ThinkingStart,
            Self::ThinkingDelta { delta } => InbandScanEvent::ThinkingDelta { delta: map(delta) },
            Self::ThinkingEnd { thinking } => InbandScanEvent::ThinkingEnd { thinking: map(thinking) },
            Self::ImpliedThinkingEnd => InbandScanEvent::ImpliedThinkingEnd,
            Self::ToolStart { id, name } => InbandScanEvent::ToolStart { id: map(id), name },
            Self::ToolArgDelta { id, name, key, delta } => {
                InbandScanEvent::ToolArgDelta { id: map(id), name, key: map(key), delta: map(delta) }
            }
            Self::ToolEnd { id, name, arguments, raw_block } => {
                InbandScanEvent::ToolEnd { id: map(id), name, arguments, raw_block: raw_block.map(map) }
            }
        }
    }
}

impl<V, S> InbandScanEvent<V, S> {
    /// Map only source/event strings. The argument handle is moved unchanged.
    pub fn map_strings<T>(self, mut map: impl FnMut(S) -> T) -> InbandScanEvent<V, T> {
        match self {
            Self::Text { text } => InbandScanEvent::Text { text: map(text) },
            Self::ThinkingStart => InbandScanEvent::ThinkingStart,
            Self::ThinkingDelta { delta } => InbandScanEvent::ThinkingDelta { delta: map(delta) },
            Self::ThinkingEnd { thinking } => InbandScanEvent::ThinkingEnd { thinking: map(thinking) },
            Self::ImpliedThinkingEnd => InbandScanEvent::ImpliedThinkingEnd,
            Self::ToolStart { id, name } => InbandScanEvent::ToolStart { id: map(id), name: map(name) },
            Self::ToolArgDelta { id, name, key, delta } => {
                InbandScanEvent::ToolArgDelta { id: map(id), name: map(name), key: map(key), delta: map(delta) }
            }
            Self::ToolEnd { id, name, arguments, raw_block } => {
                InbandScanEvent::ToolEnd { id: map(id), name: map(name), arguments, raw_block: raw_block.map(map) }
            }
        }
    }
}

pub trait InbandScanner {
    type Arguments;
    fn feed(&mut self, text: &str) -> Vec<InbandScanEvent<Self::Arguments>>;
    fn flush(&mut self) -> Vec<InbandScanEvent<Self::Arguments>>;
}

/// Full Native JS source domain. Every event string retains exact code units.
pub trait InbandScannerUnits {
    type Arguments;
    fn feed_units(&mut self, text: &ara_rpc::WireString) -> Vec<InbandScanEvent<Self::Arguments, ara_rpc::WireString>>;
    fn flush_units(&mut self) -> Vec<InbandScanEvent<Self::Arguments, ara_rpc::WireString>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialectToolResult {
    pub id: String,
    pub name: String,
    pub index: usize,
    pub text: String,
    pub is_error: bool,
}

#[derive(Clone, Copy, Default)]
pub struct DialectRenderOptions<'a> {
    pub tools: &'a [crate::Tool],
}

/// Root implements this over the actual shared ToolCall argument carrier.
/// Native utils.stringifyJson uses a BigInt-to-decimal-string replacer;
/// unsupported top-level values return None, while cycles/throwing values
/// retain an error. There is deliberately no serde fallback implementation.
pub trait ToolArgumentJson {
    type Error;
    fn stringify_tool_arguments(&self, call: &crate::ToolCall) -> Result<Option<String>, Self::Error>;
}

/// Rendering portion of Native DialectDefinition. Scanner construction remains
/// typed separately until Root freezes its runtime Send/carrier interface.
pub trait DialectRenderer {
    type Error;
    fn dialect(&self) -> Dialect;
    fn prompt(&self) -> &'static str;
    fn render_tool_call(
        &self,
        call: &crate::ToolCall,
        options: DialectRenderOptions<'_>,
    ) -> Result<String, Self::Error>;
    fn render_assistant_tool_calls(
        &self,
        calls: &[&crate::ToolCall],
        options: DialectRenderOptions<'_>,
    ) -> Result<String, Self::Error>;
    fn render_tool_results(&self, results: &[DialectToolResult], options: DialectRenderOptions<'_>) -> String;
    fn render_thinking(&self, text: &str) -> String;
    fn render_transcript(
        &self,
        messages: &[crate::Message],
        options: DialectRenderOptions<'_>,
    ) -> Result<String, Self::Error>;
}
