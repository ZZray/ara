//! Actual Chat Completions producer, fixed OMP 596f2da (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See LICENSE.
//! Canonical messages/arguments are allocated from raw SSE on the supplied
//! Session factory. No typed AssistantMessage export/import bridge is used.
mod callbacks;
mod history;
pub mod markup;
mod qwen_xml;
mod schema;
use super::{ChunkFlow, CompletionsTransportState, OpenAICompat, PostRetryState, ProviderError, StreamOptions};
use crate::actual_provider::{GraphBuilder, GraphProviderContext, concat, new_assistant, trim_empty};
use crate::event::ActualStreamFailureReceipt;
use crate::graph_values::GraphValueFactory;
use crate::relaxed_json::{FinalParseError, parse_streaming_json_units};
use crate::{ActualAssistantEvent, ActualAssistantStream, AssistantMessageEvent, Model, StopReason, ToolChoice};
use ara_rpc::{
    WireString, WireValue,
    value_heap::{PropertyKey, ValueError, ValueHandle, ValueKind, ValueSnapshot},
};
use std::{
    cell::Cell,
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
    time::Instant,
};

fn scalar(value: &WireString) -> Result<String, ValueError> {
    value
        .to_utf8()
        .map_err(|_| ValueError::UnsupportedOperation("This provider control requires a Unicode scalar string"))
}
fn number_key(value: f64) -> u64 {
    // Native Map uses SameValueZero for number indexes, including -0 and NaN.
    if value == 0.0 {
        0
    } else if value.is_nan() {
        f64::NAN.to_bits()
    } else {
        value.to_bits()
    }
}
fn text_property(builder: &GraphBuilder, root: &ValueHandle, key: &str) -> Result<WireString, ValueError> {
    builder.string(&builder.get(root, key)?)
}
fn kind_is(builder: &GraphBuilder, root: &ValueHandle, kind: &str) -> Result<bool, ValueError> {
    Ok(builder.optional_string(root, "type")?.is_some_and(|value| value.equals_ascii(kind)))
}
fn wire_text(builder: &GraphBuilder, value: &ValueHandle) -> Result<WireString, ValueError> {
    if let ValueSnapshot::String(text) = builder.0.heap().snapshot(value)? {
        return Ok(text);
    }
    let kind = builder.0.heap().kind(value)?;
    if kind == ValueKind::Object {
        return Ok(content_record_text(builder, value)?.unwrap_or_else(|| "".into()));
    }
    if kind != ValueKind::Array {
        return Ok("".into());
    }
    let mut output = WireString::from("");
    for member in builder.members(value)? {
        let part = match builder.0.heap().snapshot(&member)? {
            ValueSnapshot::String(text) => Some(text),
            _ if matches!(builder.0.heap().kind(&member)?, ValueKind::Object | ValueKind::Array) => {
                content_record_text(builder, &member)?
            }
            _ => None,
        };
        if let Some(part) = part {
            output = concat(&output, &part);
        }
    }
    Ok(output)
}
fn content_record_text(builder: &GraphBuilder, value: &ValueHandle) -> Result<Option<WireString>, ValueError> {
    let kind = builder.get(value, "type")?;
    let text_part = matches!(builder.0.heap().snapshot(&kind)?, ValueSnapshot::Undefined)
        || matches!(builder.0.heap().snapshot(&kind)?, ValueSnapshot::String(kind) if kind.equals_ascii("text"));
    if !text_part {
        return Ok(None);
    }
    let text = builder.get(value, "text")?;
    Ok(match builder.0.heap().snapshot(&text)? {
        ValueSnapshot::String(text) => Some(text),
        _ => None,
    })
}

/// Fixed serializeToolArguments (311–333): records stringify in its Native
/// catch, strings strictly parse to a record first, other values become {}.
/// Missing Host/carrier support is not a Native exception eligible for repair.
fn serialize_arguments(builder: &GraphBuilder, source: &ValueHandle) -> Result<ValueHandle, ValueError> {
    let record = match builder.0.heap().kind(source)? {
        ValueKind::Object => Some(source.clone()),
        ValueKind::String => {
            let text = builder.string(source)?;
            match WireValue::parse_units(text.units()) {
                Ok(wire @ WireValue::Object(_)) => Some(builder.import_wire(wire)?),
                _ => None,
            }
        }
        _ => None,
    };
    let Some(record) = record else {
        return builder.text("{}");
    };
    match builder.0.heap().stringify(&record, ara_rpc::value_heap::StringifyMode::Plain, None, builder.0.runtime()) {
        Ok(Some(json)) => builder.text(json),
        Ok(None) => builder.0.heap().undefined(),
        Err(error) if error.is_native_exception() => builder.text("{}"),
        Err(error) => Err(error),
    }
}

/// Graph wire request projection. Only new request objects are written;
/// source messages/arguments and schema handles retain their original lineage.
pub fn build_request(
    model: &Model,
    context: &GraphProviderContext,
    options: &StreamOptions,
    factory: Arc<GraphValueFactory>,
) -> Result<Vec<u8>, ValueError> {
    let disabled = options.provider_session_state.as_ref().is_some_and(|state| state.strict_tools_disabled(model));
    build_request_and_compat(model, context, options, factory, disabled.then_some(super::ToolStrictMode::None))
        .map(|prepared| prepared.bytes)
}

struct PreparedChatRequest {
    bytes: Vec<u8>,
    compat: OpenAICompat,
    strict_applied: bool,
    retry_on_grammar_error: bool,
}

fn build_request_and_compat(
    model: &Model,
    context: &GraphProviderContext,
    options: &StreamOptions,
    factory: Arc<GraphValueFactory>,
    strict_override: Option<super::ToolStrictMode>,
) -> Result<PreparedChatRequest, ValueError> {
    context.check_lineage(&factory)?;
    let builder = GraphBuilder(factory);
    let mut knobs = serde_json::json!({"model": model.id, "stream": true});
    // These are typed request controls only, never exported Native tools or
    // history. Fixed initialPolicy sees the requested, not surviving, choice.
    let requested_choice = options.tool_choice.as_ref().and_then(|choice| match choice {
        ToolChoice::Auto => Some(serde_json::json!("auto")),
        ToolChoice::None => Some(serde_json::json!("none")),
        ToolChoice::Any | ToolChoice::Required => Some(serde_json::json!("required")),
        ToolChoice::Function(name) | ToolChoice::FunctionNested(name) | ToolChoice::Tool(name) => {
            Some(serde_json::json!({"type":"function","function":{"name":name}}))
        }
        ToolChoice::Computer => None,
    });
    let initial = crate::reasoning::native_chat_initial_policy(
        model,
        options.reasoning.as_ref(),
        options.reasoning_policy.as_ref(),
        &options.compat,
        requested_choice.as_ref(),
    )
    .map_err(|error| ValueError::TypeError(error.to_string()))?;
    if let Some(temperature) = options.temperature {
        knobs["temperature"] = serde_json::json!(temperature);
    }
    if initial.compat.supports_usage_in_streaming {
        knobs["stream_options"] = serde_json::json!({"include_usage": true});
    }
    let request = builder.derived(&knobs)?;
    let prefix_binding =
        options.reasoning_policy.as_ref().is_some_and(crate::OpenAIReasoningPolicy::history_prefix_binding);
    let ex = crate::graph_history::NativeExecution { heap: builder.0.heap(), runtime: builder.0.runtime() };
    // Fixed buildParams reads tools?.length, then independently reads tools
    // again for convertTools. Getters may return a different original array.
    let original_tools = context.original_field(builder.0.as_ref(), "tools")?;
    let mut strict_applied = false;
    let length = if ex.nullish(&original_tools)? { ex.heap.undefined()? } else { ex.get(&original_tools, "length")? };
    if ex.heap.truthy(&length)? {
        let tools = context.original_field(builder.0.as_ref(), "tools")?;
        let converted = schema::convert(
            ex,
            &tools,
            initial.schema,
            options.no_strict,
            strict_override,
            options.quarantine_port.as_deref(),
            builder.0.as_ref(),
        )?;
        strict_applied = converted.strict_applied;
        builder.define(&request, "tools", converted.tools)?;
    } else if ex.undefined(&context.original_field(builder.0.as_ref(), "tools")?)?
        && has_tool_history(&builder, &context.original_field(builder.0.as_ref(), "messages")?)?
    {
        builder.define(&request, "tools", ex.array([])?)?;
    }
    let mut choice = requested_choice.filter(|_| initial.supports_tool_choice);
    let forced_name = choice
        .as_ref()
        .and_then(|choice| choice.get("function"))
        .and_then(|function| function.get("name"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let contains_named = |tools: &ValueHandle, name: &str| -> Result<bool, ValueError> {
        Ok(ex.heap.kind(tools)? == ValueKind::Array
            && ex.heap.truthy(&ex.apply(tools, "some", &callbacks::MATCH_WIRE_TOOL, &[builder.text(name)?])?)?)
    };
    let mut tools = builder.get(&request, "tools")?;
    if forced_name.is_some() && !initial.supports_named_tool_choice {
        if let Some(name) = &forced_name
            && contains_named(&tools, name)?
        {
            tools = ex.apply(&tools, "filter", &callbacks::MATCH_WIRE_TOOL, &[builder.text(name.as_str())?])?;
            builder.define(&request, "tools", tools.clone())?;
        }
        choice = Some(serde_json::json!("required"));
    }
    if let Some(name) = &forced_name {
        // Fixed 1751–1755 evaluates some before its Kimi predicate. Preserve
        // those Native accesses. The nativeKimiK3Reasoning-specific rewrite
        // remains OPEN until its materialized-policy route is source-bound.
        let _kimi_target_present = contains_named(&tools, name)?;
    }
    let forced = |choice: &Option<serde_json::Value>| {
        choice.as_ref().is_some_and(|choice| choice == "required" || choice.is_object())
    };
    if forced(&choice) && !initial.supports_forced_tool_choice {
        choice = Some(serde_json::json!("auto"));
    }
    let empty_tools = ex.heap.kind(&tools)? != ValueKind::Array
        || matches!(ex.heap.snapshot(&ex.get(&tools, "length")?)?, ValueSnapshot::Number(length) if length == 0.0);
    if empty_tools && (choice.as_ref().is_some_and(|choice| choice == "none") || forced(&choice)) {
        choice = None;
    }
    if let Some(name) = &forced_name
        && !contains_named(&tools, name)?
    {
        choice = None;
    }
    if initial.drop_auto_for_reasoning && choice.as_ref().is_some_and(|choice| choice == "auto") {
        choice = None;
    }
    if let Some(choice) = choice {
        knobs["tool_choice"] = choice;
    }
    // Final policy uses exactly the surviving wire choice. The encoder only
    // creates derived controls; source history is projected with its compat.
    let compat = crate::reasoning::apply_chat_final_policy(
        &mut knobs,
        model,
        options.reasoning.as_ref(),
        options.reasoning_policy.as_ref(),
        &options.compat,
    )
    .map_err(|error| ValueError::TypeError(error.to_string()))?;
    let messages = history::project_messages(&builder, model, context, &compat, prefix_binding)?;
    builder.define(&request, "messages", messages)?;
    if let Some(max) = options.max_tokens {
        let key = match compat.max_tokens_field {
            super::MaxTokensField::MaxTokens => "max_tokens",
            super::MaxTokensField::MaxCompletionTokens => "max_completion_tokens",
        };
        knobs[key] = serde_json::json!(max);
    }
    for (key, value) in knobs.as_object().expect("Derived request knobs are an object") {
        builder.define(&request, key, builder.derived(value)?)?;
    }
    // JSON.stringify creates scalar JSON syntax even for non-scalar string
    // values (lone units are escaped), so this checked final conversion is safe.
    Ok(PreparedChatRequest {
        bytes: scalar(&builder.plain_json(&request)?)?.into_bytes(),
        compat,
        strict_applied,
        retry_on_grammar_error: initial.schema.retry_on_grammar_error,
    })
}

fn has_tool_history(builder: &GraphBuilder, messages: &ValueHandle) -> Result<bool, ValueError> {
    let ex = crate::graph_history::NativeExecution { heap: builder.0.heap(), runtime: builder.0.runtime() };
    let mut found = false;
    ex.for_of(messages, |message| {
        if ex.eq(message, "role", "toolResult")? {
            found = true;
            return Ok(false);
        }
        if ex.eq(message, "role", "assistant")?
            && ex.heap.truthy(&ex.apply(&ex.get(message, "content")?, "some", &callbacks::TOOL_BLOCK, &[])?)?
        {
            found = true;
            return Ok(false);
        }
        Ok(true)
    })?;
    Ok(found)
}

enum FrameError {
    Value(ValueError),
    Provider(ProviderError),
}
impl From<ValueError> for FrameError {
    fn from(error: ValueError) -> Self {
        Self::Value(error)
    }
}
impl From<ProviderError> for FrameError {
    fn from(error: ProviderError) -> Self {
        Self::Provider(error)
    }
}
enum PartialArguments {
    Text(WireString),
    Object(ValueHandle),
    /// Original JSON.stringify output; undefined is not an empty string.
    Healed(ValueHandle),
}
struct Block {
    value: ValueHandle,
    kind: &'static str,
    text: WireString,
    signature: Option<WireString>,
}

pub(crate) struct GraphChunkState {
    builder: GraphBuilder,
    root: ValueHandle,
    content: ValueHandle,
    blocks: Vec<Block>,
    current: Option<usize>,
    tool_by_index: HashMap<u64, usize>,
    pending: Vec<usize>,
    unkeyed: HashMap<usize, usize>,
    partial_args: HashMap<usize, PartialArguments>,
    last_parse: HashMap<usize, usize>,
    finished: bool,
    saw_done: bool,
    saw_frame: bool,
    accepted: Cell<bool>,
    captured_http_error: Option<super::CapturedHttpError>,
    saw_usage: bool,
    await_usage: bool,
    first_token: Option<Instant>,
    retry_blocked: bool,
    failure_evidence: Option<crate::retry_classification::ProviderFailureEvidence>,
    stop: StopReason,
    error_message: Option<String>,
    failures: ActualStreamFailureReceipt,
    healing: Option<markup::StreamMarkupHealing>,
    cumulative_reasoning: bool,
    reasoning_snapshots: HashMap<WireString, WireString>,
    suppress_healed_thinking: bool,
    healed_tool_call_emitted: bool,
    strip_deepseek: bool,
    deepseek_strip_buffer: WireString,
    empty_length_error: bool,
}
impl GraphChunkState {
    fn new(
        model: &Model,
        factory: Arc<GraphValueFactory>,
        failures: ActualStreamFailureReceipt,
    ) -> Result<Self, ValueError> {
        let builder = GraphBuilder(factory);
        let (root, content) = new_assistant(&builder, model)?;
        Ok(Self {
            builder,
            root,
            content,
            blocks: Vec::new(),
            current: None,
            tool_by_index: HashMap::new(),
            pending: Vec::new(),
            unkeyed: HashMap::new(),
            partial_args: HashMap::new(),
            last_parse: HashMap::new(),
            finished: false,
            saw_done: false,
            saw_frame: false,
            accepted: Cell::new(false),
            captured_http_error: None,
            saw_usage: false,
            await_usage: false,
            first_token: None,
            retry_blocked: false,
            failure_evidence: None,
            stop: StopReason::Stop,
            error_message: None,
            failures,
            healing: None,
            cumulative_reasoning: false,
            reasoning_snapshots: HashMap::new(),
            suppress_healed_thinking: false,
            healed_tool_call_emitted: false,
            strip_deepseek: false,
            deepseek_strip_buffer: "".into(),
            empty_length_error: false,
        })
    }
    fn configure_stream(&mut self, compat: &OpenAICompat) -> Result<(), ValueError> {
        self.healing = compat
            .stream_markup_healing_pattern
            .map(|pattern| markup::StreamMarkupHealing::new(self.builder.0.clone(), pattern))
            .transpose()?;
        self.cumulative_reasoning = compat.reasoning_deltas_may_be_cumulative;
        self.strip_deepseek = compat.strip_deepseek_special_tokens;
        self.empty_length_error = compat.empty_length_finish_is_context_error;
        Ok(())
    }
    fn graph_error(&mut self, error: ValueError) -> ProviderError {
        self.retry_blocked = true;
        self.failures.record(error);
        ProviderError::Config("Actual provider integration failed; original exception is in the Host receipt".into())
    }
    fn parse(&self, raw: &WireString) -> Result<ValueHandle, ValueError> {
        parse_streaming_json_units(raw, self.builder.0.as_ref()).map_err(|error| match error {
            FinalParseError::Runtime(error) => error,
            FinalParseError::Syntax(error) => ValueError::TypeError(error.to_string()),
        })
    }
    fn parse_healed(&self, raw: &ValueHandle) -> Result<ValueHandle, ValueError> {
        match self.builder.0.heap().snapshot(raw)? {
            ValueSnapshot::String(raw) => self.parse(&raw),
            // Fixed parseStreamingJson uses partialJson?.trimStart(): its
            // undefined/null branch allocates a fresh ordinary {} each time.
            ValueSnapshot::Undefined | ValueSnapshot::Null => self.builder.0.heap().object(),
            _ => Err(ValueError::TypeError("Healed tool arguments must be JSON.stringify output".into())),
        }
    }
    fn finish_block(&mut self, index: usize, events: &mut Vec<ActualAssistantEvent>) -> Result<(), ValueError> {
        if self.blocks[index].kind == "toolCall" {
            let Some(arguments) = self.partial_args.remove(&index) else { return Ok(()) };
            let arguments = match arguments {
                PartialArguments::Text(raw) => self.parse(&raw)?,
                PartialArguments::Healed(raw) => {
                    // finishToolCallBlock returns before deleting partialArgs
                    // or emitting End when its original value is undefined.
                    match self.builder.0.heap().snapshot(&raw)? {
                        ValueSnapshot::Undefined => return Ok(()),
                        ValueSnapshot::String(text) => self.parse(&text)?,
                        ValueSnapshot::Null => {
                            let json = self.builder.plain_json(&raw)?;
                            events.push(AssistantMessageEvent::ToolcallDelta {
                                content_index: self.builder.0.heap().number(index as f64)?,
                                delta: self.builder.text(json)?,
                                partial: self.root.clone(),
                            });
                            raw
                        }
                        _ => {
                            return Err(ValueError::TypeError(
                                "Healed tool arguments must be JSON.stringify output".into(),
                            ));
                        }
                    }
                }
                PartialArguments::Object(object) => {
                    let json = self.builder.plain_json(&object)?;
                    if !json.equals_ascii("{}") {
                        events.push(AssistantMessageEvent::ToolcallDelta {
                            content_index: self.builder.0.heap().number(index as f64)?,
                            delta: self.builder.text(json)?,
                            partial: self.root.clone(),
                        });
                    }
                    object
                }
            };
            let block = self.blocks[index].value.clone();
            self.builder.put(&block, "arguments", arguments)?;
            for key in ["partialArgs", "streamIndex"] {
                self.builder.0.heap().delete_own(&block, &PropertyKey::from(key), self.builder.0.runtime())?;
            }
            self.pending.retain(|pending| *pending != index);
            self.tool_by_index.retain(|_, value| *value != index);
            self.unkeyed.retain(|_, value| *value != index);
            self.last_parse.remove(&index);
            events.push(AssistantMessageEvent::ToolcallEnd {
                content_index: self.builder.0.heap().number(index as f64)?,
                tool_call: block,
                partial: self.root.clone(),
            });
        } else if self.blocks[index].kind == "text" {
            events.push(AssistantMessageEvent::TextEnd {
                content_index: self.builder.0.heap().number(index as f64)?,
                content: self.blocks[index].text.clone(),
                partial: self.root.clone(),
            });
        } else {
            events.push(AssistantMessageEvent::ThinkingEnd {
                content_index: self.builder.0.heap().number(index as f64)?,
                content: self.blocks[index].text.clone(),
                partial: self.root.clone(),
            });
        }
        Ok(())
    }
    fn close_blocks(&mut self, events: &mut Vec<ActualAssistantEvent>) -> Result<(), ValueError> {
        if let Some(index) = self.current.take() {
            if self.blocks[index].kind != "toolCall" {
                self.finish_block(index, events)?;
            }
        }
        for index in self.pending.clone() {
            self.finish_block(index, events)?;
        }
        Ok(())
    }
    fn append_channel(
        &mut self,
        kind: &'static str,
        text: WireString,
        signature: Option<WireString>,
        events: &mut Vec<ActualAssistantEvent>,
    ) -> Result<(), ValueError> {
        if text.is_empty() {
            return Ok(());
        }
        self.first_token.get_or_insert_with(Instant::now);
        // Healed thinking has no signature but remains in the thinking channel.
        // An absent signature can reuse any current thinking block; an explicit
        // signature must equal that block's signature, as appendThinking does.
        let same = self.current.is_some_and(|index| {
            self.blocks[index].kind == kind
                && (kind == "text" || signature.is_none() || self.blocks[index].signature == signature)
        });
        if !same {
            if let Some(index) = self.current {
                if self.blocks[index].kind != "toolCall" {
                    self.finish_block(index, events)?;
                }
            }
            let key = if kind == "text" { "text" } else { "thinking" };
            let block = self.builder.object([("type", self.builder.text(kind)?), (key, self.builder.text("")?)])?;
            if kind == "thinking" {
                let signature_value = match &signature {
                    Some(signature) => self.builder.text(signature.clone())?,
                    None => self.builder.0.heap().undefined()?,
                };
                self.builder.define(&block, "thinkingSignature", signature_value)?;
            }
            self.builder.push(&self.content, block.clone())?;
            let index = self.blocks.len();
            self.blocks.push(Block { value: block, kind, text: "".into(), signature });
            self.current = Some(index);
            events.push(if kind == "text" {
                AssistantMessageEvent::TextStart {
                    content_index: self.builder.0.heap().number(index as f64)?,
                    partial: self.root.clone(),
                }
            } else {
                AssistantMessageEvent::ThinkingStart {
                    content_index: self.builder.0.heap().number(index as f64)?,
                    partial: self.root.clone(),
                }
            });
        }
        let index = self.current.unwrap();
        self.blocks[index].text = concat(&self.blocks[index].text, &text);
        self.builder.put(
            &self.blocks[index].value,
            if kind == "text" { "text" } else { "thinking" },
            self.builder.text(self.blocks[index].text.clone())?,
        )?;
        events.push(if kind == "text" {
            AssistantMessageEvent::TextDelta {
                content_index: self.builder.0.heap().number(index as f64)?,
                delta: text,
                partial: self.root.clone(),
            }
        } else {
            AssistantMessageEvent::ThinkingDelta {
                content_index: self.builder.0.heap().number(index as f64)?,
                delta: text,
                partial: self.root.clone(),
            }
        });
        Ok(())
    }
    fn append_thinking(
        &mut self,
        text: WireString,
        signature: Option<WireString>,
        cumulative: bool,
        events: &mut Vec<ActualAssistantEvent>,
    ) -> Result<(), ValueError> {
        if text.is_empty() {
            return Ok(());
        }
        let emitted = if cumulative {
            let key = signature.clone().unwrap_or_else(|| "".into());
            let previous = self.reasoning_snapshots.get(&key).cloned().unwrap_or_else(|| "".into());
            let emitted = if text.units().starts_with(previous.units()) {
                WireString::from_units(text.units()[previous.len()..].to_vec())
            } else {
                text.clone()
            };
            self.reasoning_snapshots.insert(key, text);
            emitted
        } else {
            text
        };
        self.append_channel("thinking", emitted, signature, events)
    }
    fn flush_deepseek_buffer(
        &mut self,
        final_input: bool,
        events: &mut Vec<ActualAssistantEvent>,
    ) -> Result<(), ValueError> {
        if self.deepseek_strip_buffer.is_empty() {
            return Ok(());
        }
        let held = if final_input { 0 } else { markup::trailing_deepseek_partial(&self.deepseek_strip_buffer) };
        let visible = self.deepseek_strip_buffer.len() - held;
        let flushable = WireString::from_units(self.deepseek_strip_buffer.units()[..visible].to_vec());
        self.deepseek_strip_buffer = WireString::from_units(self.deepseek_strip_buffer.units()[visible..].to_vec());
        let stripped = markup::strip_deepseek_tokens(&flushable);
        if !stripped.is_empty() && (stripped == flushable || !trim_empty(&stripped)) {
            self.append_channel("text", stripped, None, events)?;
        }
        Ok(())
    }
    fn append_processed_text(
        &mut self,
        text: WireString,
        events: &mut Vec<ActualAssistantEvent>,
    ) -> Result<(), ValueError> {
        if text.is_empty() {
            return Ok(());
        }
        if self.strip_deepseek {
            self.deepseek_strip_buffer = concat(&self.deepseek_strip_buffer, &text);
            self.flush_deepseek_buffer(false, events)
        } else {
            self.append_channel("text", text, None, events)
        }
    }
    fn emit_healed_tool_call(
        &mut self,
        call: markup::HealedToolCall,
        events: &mut Vec<ActualAssistantEvent>,
    ) -> Result<(), ValueError> {
        // The healed call is finalized immediately, unlike structured calls
        // which remain pending through text/thinking transitions.
        if let Some(index) = self.current {
            self.finish_block(index, events)?;
        }
        let block = self.builder.object([
            ("type", self.builder.text("toolCall")?),
            ("id", self.builder.text(call.id)?),
            ("name", call.name),
            ("arguments", self.builder.0.heap().object()?),
            ("partialArgs", call.arguments.clone()),
        ])?;
        let raw = call.arguments;
        let arguments = self.parse_healed(&raw)?;
        self.builder.put(&block, "arguments", arguments)?;
        let index = self.blocks.len();
        // If a user-modified push throws, the new block is not in content and
        // the source's indexOf guard closes no block. Avoid retaining the old
        // already-finalized block or an index without a bookkeeping record.
        self.current = None;
        self.builder.push(&self.content, block.clone())?;
        self.blocks.push(Block { value: block, kind: "toolCall", text: "".into(), signature: None });
        self.current = Some(index);
        self.partial_args.insert(index, PartialArguments::Healed(raw.clone()));
        events.push(AssistantMessageEvent::ToolcallStart {
            content_index: self.builder.0.heap().number(index as f64)?,
            partial: self.root.clone(),
        });
        events.push(AssistantMessageEvent::ToolcallDelta {
            content_index: self.builder.0.heap().number(index as f64)?,
            delta: raw,
            partial: self.root.clone(),
        });
        // Ordinary strings are parsed again at finishCurrentBlock. The
        // source undefined guard emits no End and keeps the first {} instead.
        self.finish_block(index, events)?;
        self.current = None;
        self.healed_tool_call_emitted = true;
        Ok(())
    }
    fn emit_healing_events(
        &mut self,
        healing: Vec<markup::HealingEvent>,
        events: &mut Vec<ActualAssistantEvent>,
    ) -> Result<(), ValueError> {
        for event in healing {
            match event {
                markup::HealingEvent::Text(text) => self.append_processed_text(text, events)?,
                markup::HealingEvent::Thinking(text) if !self.suppress_healed_thinking => {
                    self.append_thinking(text, None, false, events)?
                }
                markup::HealingEvent::Thinking(_) => {}
                markup::HealingEvent::ToolCall(call) => self.emit_healed_tool_call(call, events)?,
            }
        }
        Ok(())
    }
    fn flush_healing(&mut self, events: &mut Vec<ActualAssistantEvent>) -> Result<(), ValueError> {
        if let Some(healing) = &mut self.healing {
            let mut emitted = healing.flush_events()?;
            emitted.extend(healing.drain_completed().into_iter().map(markup::HealingEvent::ToolCall));
            self.emit_healing_events(emitted, events)?;
            if self.healed_tool_call_emitted && self.stop == StopReason::Stop {
                self.stop = StopReason::ToolUse;
            }
        }
        if self.strip_deepseek {
            self.flush_deepseek_buffer(true, events)?;
        }
        Ok(())
    }
    fn tool_delta(
        &mut self,
        call: &ValueHandle,
        offset: usize,
        batch_size: usize,
        events: &mut Vec<ActualAssistantEvent>,
    ) -> Result<(), ValueError> {
        let index_value = self.builder.get(call, "index")?;
        let stream_index = match self.builder.0.heap().snapshot(&index_value)? {
            ValueSnapshot::Number(value) => Some(value),
            _ => None,
        };
        let id = self.builder.optional_string(call, "id")?.filter(|id| !id.is_empty());
        let function = self.builder.get(call, "function")?;
        let name = if self.builder.is_nullish(&function)? {
            None
        } else {
            self.builder.optional_string(&function, "name")?.filter(|name| !name.is_empty())
        };
        let unkeyed = batch_size > 1 && stream_index.is_none() && id.is_none();
        let mut target = stream_index.and_then(|index| self.tool_by_index.get(&number_key(index)).copied());
        if target.is_none() {
            if let Some(id) = &id {
                for &index in &self.pending {
                    if self.builder.optional_string(&self.blocks[index].value, "id")?.as_ref() == Some(id) {
                        target = Some(index);
                        break;
                    }
                }
            }
        }
        if target.is_none() && unkeyed {
            target = self.unkeyed.get(&offset).copied().filter(|index| self.partial_args.contains_key(index));
        }
        if target.is_none() && !unkeyed {
            if let Some(index) = self.current.filter(|index| self.partial_args.contains_key(index)) {
                if id.is_none()
                    || self.builder.optional_string(&self.blocks[index].value, "id")?.as_ref() == id.as_ref()
                {
                    target = Some(index);
                }
            }
        }
        let index = if let Some(index) = target {
            if self.current != Some(index) {
                if let Some(current) = self.current {
                    if self.blocks[current].kind != "toolCall" {
                        self.finish_block(current, events)?;
                    }
                }
            }
            self.current = Some(index);
            index
        } else {
            if let Some(current) = self.current {
                if self.blocks[current].kind != "toolCall" {
                    self.finish_block(current, events)?;
                }
            }
            let block = self.builder.object([
                ("type", self.builder.text("toolCall")?),
                ("id", self.builder.text(id.clone().unwrap_or_else(|| "".into()))?),
                ("name", self.builder.text(name.clone().unwrap_or_else(|| "".into()))?),
                ("arguments", self.builder.0.heap().object()?),
                ("partialArgs", self.builder.text("")?),
            ])?;
            self.builder.push(&self.content, block.clone())?;
            let index = self.blocks.len();
            self.blocks.push(Block { value: block, kind: "toolCall", text: "".into(), signature: None });
            self.pending.push(index);
            self.current = Some(index);
            self.partial_args.insert(index, PartialArguments::Text("".into()));
            if unkeyed {
                self.unkeyed.insert(offset, index);
            }
            events.push(AssistantMessageEvent::ToolcallStart {
                content_index: self.builder.0.heap().number(index as f64)?,
                partial: self.root.clone(),
            });
            index
        };
        let block = self.blocks[index].value.clone();
        if let Some(stream_index) = stream_index {
            self.tool_by_index.entry(number_key(stream_index)).or_insert(index);
            self.builder.put(&block, "streamIndex", self.builder.0.heap().number(stream_index)?)?;
        }
        if let Some(id) = id {
            self.builder.put(&block, "id", self.builder.text(id)?)?;
        }
        if let Some(name) = name {
            self.builder.put(&block, "name", self.builder.text(name)?)?;
        }
        let mut delta = self.builder.text("")?;
        if !self.builder.is_nullish(&function)? {
            let raw = self.builder.get(&function, "arguments")?;
            match self.builder.0.heap().snapshot(&raw)? {
                ValueSnapshot::String(fragment) if !fragment.is_empty() => {
                    let previous = match self.partial_args.remove(&index) {
                        Some(PartialArguments::Text(previous)) => previous,
                        _ => {
                            self.last_parse.remove(&index);
                            "".into()
                        }
                    };
                    let accumulated = concat(&previous, &fragment);
                    delta = raw.clone();
                    self.builder.put(&block, "partialArgs", self.builder.text(accumulated.clone())?)?;
                    let last = self.last_parse.get(&index).copied().unwrap_or(0);
                    if last == 0 || accumulated.len() >= last + (last / 2).max(256) {
                        let arguments = self.parse(&accumulated)?;
                        self.builder.put(&block, "arguments", arguments)?;
                        self.last_parse.insert(index, accumulated.len().max(1));
                    }
                    self.partial_args.insert(index, PartialArguments::Text(accumulated));
                }
                ValueSnapshot::Object => {
                    let previous = match self.partial_args.remove(&index) {
                        Some(PartialArguments::Object(previous)) => Some(previous),
                        _ => None,
                    };
                    let merged = merge_objects(&self.builder, previous.as_ref(), &raw)?;
                    self.builder.put(&block, "partialArgs", merged.clone())?;
                    self.builder.put(&block, "arguments", merged.clone())?;
                    self.partial_args.insert(index, PartialArguments::Object(merged));
                    self.last_parse.remove(&index);
                }
                _ => {}
            }
        }
        self.first_token.get_or_insert_with(Instant::now);
        events.push(AssistantMessageEvent::ToolcallDelta {
            content_index: self.builder.0.heap().number(index as f64)?,
            delta,
            partial: self.root.clone(),
        });
        Ok(())
    }
    fn handle_graph(
        &mut self,
        chunk: &ValueHandle,
        events: &mut Vec<ActualAssistantEvent>,
    ) -> Result<(bool, ChunkFlow), FrameError> {
        if self.builder.0.heap().kind(chunk)? != ValueKind::Object {
            return Ok((false, ChunkFlow::Continue));
        }
        self.saw_frame = true;
        // Scalar transport error classification is a checked observation of
        // the raw JSON envelope. It never reconstructs a canonical payload.
        let raw_error = self.builder.get(chunk, "error")?;
        let raw_message = self.builder.get(chunk, "message")?;
        let choices = self.builder.get(chunk, "choices")?;
        let legacy_error = !self.builder.is_nullish(&raw_error)?
            || !self.builder.is_nullish(&raw_message)? && self.builder.is_nullish(&choices)?;
        let mut probe_error = legacy_error;
        for key in ["status", "code", "type", "response"] {
            if !self.builder.is_nullish(&self.builder.get(chunk, key)?)? { probe_error = true; }
        }
        if probe_error {
            let json = scalar(&self.builder.plain_json(chunk)?)?;
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) {
                if let Some(error) = super::stream_error(&value) {
                    self.retry_blocked = error.retry_blocked;
                    self.failure_evidence = Some(error.evidence);
                    return Err(error.cause.into());
                }
            }
            if legacy_error {
                return Err(ProviderError::Stream("Provider returned a non-scalar actual error envelope".into()).into());
            }
        }
        for (source, target) in [("id", "responseId"), ("provider", "upstreamProvider")] {
            let old = self.builder.get(&self.root, target)?;
            if self.builder.is_nullish(&old)? {
                if let Some(value) = self.builder.optional_string(chunk, source)?.filter(|value| !value.is_empty()) {
                    self.builder.put(&self.root, target, self.builder.text(value)?)?;
                }
            }
        }
        let mut progress = false;
        let mut top_usage = false;
        let usage = self.builder.get(chunk, "usage")?;
        if self.builder.0.heap().kind(&usage)? == ValueKind::Object {
            self.apply_usage(&usage)?;
            progress = true;
            top_usage = true;
        }
        if self.builder.is_nullish(&choices)? {
            return Ok((
                progress,
                if self.finished && self.saw_usage { ChunkFlow::Break } else { ChunkFlow::Continue },
            ));
        }
        let choice = self.builder.0.heap().get(&choices, &PropertyKey::from("0"), self.builder.0.runtime())?;
        if self.builder.is_nullish(&choice)? {
            return Ok((
                progress,
                if self.finished && self.saw_usage { ChunkFlow::Break } else { ChunkFlow::Continue },
            ));
        }
        if !top_usage {
            let usage = self.builder.get(&choice, "usage")?;
            if self.builder.0.heap().kind(&usage)? == ValueKind::Object {
                self.apply_usage(&usage)?;
                progress = true;
            }
        }
        if let Some(reason) =
            self.builder.optional_string(&choice, "finish_reason")?.filter(|reason| !reason.is_empty())
        {
            let (stop, error) = super::map_stop_reason(Some(&scalar(&reason)?));
            self.stop = stop;
            self.error_message = error;
            self.finished = true;
            progress = true;
            self.builder.put(&self.root, "stopReason", self.builder.text(stop.as_str())?)?;
        }
        let delta = self.builder.get(&choice, "delta")?;
        if !self.builder.is_nullish(&delta)? {
            for field in super::REASONING_FIELDS {
                // Original typeof-string ignores non-string alias values.
                if let ValueSnapshot::String(text) =
                    self.builder.0.heap().snapshot(&self.builder.get(&delta, field)?)?
                {
                    if !text.is_empty() {
                        self.append_thinking(text, Some(field.into()), self.cumulative_reasoning, events)?;
                        self.suppress_healed_thinking = true;
                        progress = true;
                        break;
                    }
                }
            }
            let content = self.builder.get(&delta, "content")?;
            let text = wire_text(&self.builder, &content)?;
            if !text.is_empty() {
                progress = true;
                self.first_token.get_or_insert_with(Instant::now);
                let structured = self.builder.get(&delta, "tool_calls")?;
                let has_structured = self.builder.0.heap().kind(&structured)? == ValueKind::Array
                    && matches!(self.builder.0.heap().snapshot(&self.builder.get(&structured, "length")?)?, ValueSnapshot::Number(length) if length > 0.0);
                if let Some(healing) = &mut self.healing {
                    let emitted = if has_structured {
                        healing.feed_events_without_calls(&text)?
                    } else {
                        healing.feed_events(&text)?
                    };
                    self.emit_healing_events(emitted, events)?;
                } else {
                    self.append_processed_text(text, events)?;
                }
            }
            let calls = self.builder.get(&delta, "tool_calls")?;
            if !self.builder.is_nullish(&calls)? {
                let calls = self.builder.members(&calls)?;
                if !calls.is_empty() {
                    progress = true;
                }
                for (offset, call) in calls.iter().enumerate() {
                    self.tool_delta(call, offset, calls.len(), events)?;
                }
            }
            if self.builder.optional_string(&delta, "refusal")?.is_some_and(|value| !value.is_empty()) {
                progress = true;
            }
        }
        Ok((
            progress,
            if self.finished && self.saw_usage && !self.await_usage { ChunkFlow::Break } else { ChunkFlow::Continue },
        ))
    }
    fn apply_usage(&mut self, usage: &ValueHandle) -> Result<(), ValueError> {
        let json = scalar(&self.builder.plain_json(usage)?)?;
        let observed = serde_json::from_str(&json)
            .map_err(|_| ValueError::UnsupportedOperation("Actual usage counters require scalar JSON"))?;
        let usage = super::parse_chunk_usage(&observed);
        self.await_usage = !super::has_positive_cache_read(&observed);
        self.saw_usage = true;
        self.builder.put(&self.root, "usage", self.builder.derived(&usage)?)
    }
    fn settle(
        &mut self,
        started: Instant,
        result: Result<(), ProviderError>,
        events: &mut Vec<ActualAssistantEvent>,
    ) -> Result<ActualAssistantEvent, ValueError> {
        self.builder.put(
            &self.root,
            "duration",
            self.builder.0.heap().number(started.elapsed().as_millis() as f64)?,
        )?;
        if let Some(first) = self.first_token {
            self.builder.put(
                &self.root,
                "ttft",
                self.builder.0.heap().number(first.duration_since(started).as_millis() as f64)?,
            )?;
        }
        match result {
            Ok(()) => Ok(AssistantMessageEvent::Done { reason: self.stop, message: self.root.clone() }),
            Err(error) => {
                self.close_blocks(events)?;
                self.stop = error.stop_reason();
                self.builder.put(&self.root, "stopReason", self.builder.text(self.stop.as_str())?)?;
                self.builder.put(&self.root, "errorMessage", self.builder.text(error.to_string())?)?;
                if let Some(status) = error.status() {
                    self.builder.put(&self.root, "errorStatus", self.builder.0.heap().number(f64::from(status))?)?;
                }
                // Preserve transport-owned classification and explicit waits.
                let stop = text_property(&self.builder, &self.root, "stopReason")?;
                let stop = if stop.equals_ascii("aborted") {
                    StopReason::Aborted
                } else if stop.equals_ascii("error") {
                    StopReason::Error
                } else {
                    return Err(ValueError::TypeError("Actual error root has a nonfailure stopReason".into()));
                };
                let error_message = scalar(&text_property(&self.builder, &self.root, "errorMessage")?)?;
                let status = self.builder.get(&self.root, "errorStatus")?;
                let status = match self.builder.0.heap().snapshot(&status)? {
                    ValueSnapshot::Number(value)
                        if value >= 0.0 && value <= f64::from(u16::MAX) && value.fract() == 0.0 =>
                    {
                        Some(value as u16)
                    }
                    ValueSnapshot::Undefined | ValueSnapshot::Null => None,
                    _ => return Err(ValueError::TypeError("Actual errorStatus must be an HTTP status number".into())),
                };
                let evidence = crate::retry_classification::finalize_failure_facts(
                    stop,
                    Some(&error_message),
                    status,
                    self.failure_evidence.take(),
                    &error,
                    self.retry_blocked,
                    false,
                    super::API,
                );
                // Fixed openai-completions.ts catch publishes the finalized ID
                // before the terminal event. This producer already finalized its
                // owned scalar transport facts; the same canonical root receives
                // the ID, preserving the ordinary Native assignment operation.
                self.builder.put(&self.root, "errorId", self.builder.0.heap().number(f64::from(evidence.error_id))?)?;
                self.builder.put(&self.root, "failureEvidence", self.builder.derived(&evidence)?)?;
                Ok(AssistantMessageEvent::Error { reason: stop, error: self.root.clone() })
            }
        }
    }
}

fn safe_key(key: &WireString) -> bool {
    !["__proto__", "constructor", "prototype"].iter().any(|name| key.equals_ascii(name))
}
fn clone_argument(builder: &GraphBuilder, source: &ValueHandle) -> Result<ValueHandle, ValueError> {
    match builder.0.heap().snapshot(source)? {
        ValueSnapshot::Object => merge_objects(builder, None, source),
        ValueSnapshot::Array => {
            let target = builder.0.heap().array_with_length(0)?;
            for member in builder.members(source)? {
                builder.push(&target, clone_argument(builder, &member)?)?;
            }
            Ok(target)
        }
        _ => Ok(source.clone()),
    }
}
fn argument_equal(builder: &GraphBuilder, left: &ValueHandle, right: &ValueHandle) -> Result<bool, ValueError> {
    match (builder.0.heap().snapshot(left)?, builder.0.heap().snapshot(right)?) {
        (ValueSnapshot::Null, ValueSnapshot::Null) | (ValueSnapshot::Undefined, ValueSnapshot::Undefined) => Ok(true),
        (ValueSnapshot::Number(left), ValueSnapshot::Number(right)) => Ok(left == right),
        (ValueSnapshot::String(left), ValueSnapshot::String(right)) => Ok(left == right),
        (ValueSnapshot::Boolean(left), ValueSnapshot::Boolean(right)) => Ok(left == right),
        (ValueSnapshot::Array, ValueSnapshot::Array) => {
            let left = builder.members(left)?;
            let right = builder.members(right)?;
            if left.len() != right.len() {
                return Ok(false);
            }
            for (left, right) in left.iter().zip(&right) {
                if !argument_equal(builder, left, right)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        (ValueSnapshot::Object, ValueSnapshot::Object) => {
            let left_keys = builder
                .0
                .heap()
                .own_enumerable_string_keys(left, builder.0.runtime())?
                .into_iter()
                .filter(safe_key)
                .collect::<Vec<_>>();
            let right_keys = builder
                .0
                .heap()
                .own_enumerable_string_keys(right, builder.0.runtime())?
                .into_iter()
                .filter(safe_key)
                .collect::<Vec<_>>();
            if left_keys.len() != right_keys.len() {
                return Ok(false);
            }
            for key in left_keys {
                if !right_keys.contains(&key) {
                    return Ok(false);
                }
                let property = PropertyKey::String(key);
                let left = builder.0.heap().get(left, &property, builder.0.runtime())?;
                let right = builder.0.heap().get(right, &property, builder.0.runtime())?;
                if !argument_equal(builder, &left, &right)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}
fn array_prefix(builder: &GraphBuilder, value: &[ValueHandle], prefix: &[ValueHandle]) -> Result<bool, ValueError> {
    if value.len() < prefix.len() {
        return Ok(false);
    }
    for (left, right) in value.iter().zip(prefix) {
        if !argument_equal(builder, left, right)? {
            return Ok(false);
        }
    }
    Ok(true)
}
fn merge_arguments(
    builder: &GraphBuilder,
    previous: &ValueHandle,
    fragment: &ValueHandle,
) -> Result<ValueHandle, ValueError> {
    match (builder.0.heap().snapshot(previous)?, builder.0.heap().snapshot(fragment)?) {
        (ValueSnapshot::String(previous), ValueSnapshot::String(fragment)) => builder
            .text(if fragment.units().starts_with(previous.units()) { fragment } else { concat(&previous, &fragment) }),
        (ValueSnapshot::Object, ValueSnapshot::Object) => merge_objects(builder, Some(previous), fragment),
        (ValueSnapshot::Array, ValueSnapshot::Array) => {
            let previous = builder.members(previous)?;
            let fragment = builder.members(fragment)?;
            let members = if array_prefix(builder, &fragment, &previous)? {
                fragment
            } else if array_prefix(builder, &previous, &fragment)? {
                previous
            } else {
                previous.into_iter().chain(fragment).collect()
            };
            let result = builder.0.heap().array_with_length(0)?;
            for member in members {
                builder.push(&result, clone_argument(builder, &member)?)?;
            }
            Ok(result)
        }
        _ => clone_argument(builder, fragment),
    }
}
fn merge_objects(
    builder: &GraphBuilder,
    previous: Option<&ValueHandle>,
    fragment: &ValueHandle,
) -> Result<ValueHandle, ValueError> {
    let output = builder.0.heap().object()?;
    if let Some(previous) = previous {
        for key in builder.0.heap().own_enumerable_string_keys(previous, builder.0.runtime())? {
            if safe_key(&key) {
                let member = builder.0.heap().get(previous, &PropertyKey::String(key.clone()), builder.0.runtime())?;
                builder.0.heap().assign(
                    &output,
                    PropertyKey::String(key),
                    clone_argument(builder, &member)?,
                    builder.0.runtime(),
                )?;
            }
        }
    }
    for key in builder.0.heap().own_enumerable_string_keys(fragment, builder.0.runtime())? {
        if !safe_key(&key) {
            continue;
        }
        let property = PropertyKey::String(key);
        let incoming = builder.0.heap().get(fragment, &property, builder.0.runtime())?;
        let previous = builder.0.heap().get(&output, &property, builder.0.runtime())?;
        let value = if builder.0.heap().kind(&previous)? == ValueKind::Undefined {
            clone_argument(builder, &incoming)?
        } else {
            merge_arguments(builder, &previous, &incoming)?
        };
        builder.0.heap().assign(&output, property, value, builder.0.runtime())?;
    }
    Ok(output)
}

impl CompletionsTransportState for GraphChunkState {
    type Event = ActualAssistantEvent;
    fn retry_state(&mut self) -> PostRetryState<'_> {
        PostRetryState { retry_blocked: &mut self.retry_blocked, failure_evidence: &mut self.failure_evidence }
    }
    fn set_failure_evidence(&mut self, evidence: crate::retry_classification::ProviderFailureEvidence) {
        self.failure_evidence = Some(evidence);
    }
    fn capture_http_error(&mut self, captured: Option<super::CapturedHttpError>) {
        self.captured_http_error = captured;
    }
    fn block_retry(&mut self) {
        self.retry_blocked = true;
    }
    fn start_event(&self) -> Self::Event {
        self.accepted.set(true);
        AssistantMessageEvent::Start { partial: self.root.clone() }
    }
    fn finished(&self) -> bool {
        self.finished
    }
    fn mark_done(&mut self) {
        self.saw_done = true;
    }
    fn raw_frame(&mut self, source: &str, events: &mut Vec<Self::Event>) -> Result<(bool, ChunkFlow), ProviderError> {
        let wire = WireValue::parse(source)
            .map_err(|error| ProviderError::Stream(format!("Malformed OpenAI completions stream frame ({error})")))?;
        let chunk = self.builder.import_wire(wire).map_err(|error| self.graph_error(error))?;
        self.handle_graph(&chunk, events).map_err(|error| match error {
            FrameError::Provider(error) => error,
            FrameError::Value(error) => self.graph_error(error),
        })
    }
    fn finish_events(&mut self, events: &mut Vec<Self::Event>) -> Result<(), ProviderError> {
        // Flush held markup/token tails before deciding whether transport EOF
        // is incomplete, and before the normal finalization sweep.
        self.flush_healing(events).map_err(|error| self.graph_error(error))?;
        if !self.finished && !self.saw_done {
            if !self.blocks.is_empty() {
                return Err(ProviderError::Incomplete(super::INCOMPLETE_STREAM_MESSAGE.into()));
            }
            if !self.saw_frame {
                return Err(ProviderError::Incomplete(super::EMPTY_STREAM_MESSAGE.into()));
            }
        }
        self.close_blocks(events).map_err(|error| self.graph_error(error))?;
        if self.stop == StopReason::Stop && self.blocks.iter().any(|block| block.kind == "toolCall") {
            self.stop = StopReason::ToolUse;
        }
        if self.empty_length_error
            && self.stop == StopReason::Length
            && !crate::replay_safe_retry::actual::visible(&self.builder, &self.root)
                .map_err(|error| self.graph_error(error))?
        {
            self.stop = StopReason::Error;
            self.error_message = Some("Model returned no content: prompt filled the context window; raise Ollama num_ctx or shorten the prompt.".into());
        }
        let stop = self.builder.text(self.stop.as_str()).map_err(|error| self.graph_error(error))?;
        self.builder.put(&self.root, "stopReason", stop).map_err(|error| self.graph_error(error))?;
        if self.stop == StopReason::Error {
            return Err(ProviderError::Stream(
                self.error_message.clone().unwrap_or_else(|| "Provider returned an error stop reason".into()),
            ));
        }
        self.builder
            .0
            .heap()
            .delete_own(&self.root, &PropertyKey::from("errorMessage"), self.builder.0.runtime())
            .map_err(|error| self.graph_error(error))?;
        Ok(())
    }
}

pub fn stream(
    client: reqwest::Client,
    model: Model,
    context: GraphProviderContext,
    factory: Arc<GraphValueFactory>,
    options: StreamOptions,
) -> Result<ActualAssistantStream, ValueError> {
    context.check_lineage(&factory)?;
    let cancel = options.cancel.clone();
    // Each original replay attempt observes its actual source values again at
    // the native request position; no pre-exported request snapshot is reused.
    crate::replay_safe_retry::actual::with_replay_safe_actual_stream_retry(
        model.clone(),
        factory.clone(),
        cancel,
        options.accept_empty_response,
        move |attempt_cancel| {
            let mut attempt_options = options.clone();
            attempt_options.cancel = attempt_cancel;
            stream_once(client.clone(), model.clone(), context.clone(), factory.clone(), attempt_options)
        },
    )
}
fn stream_once(
    client: reqwest::Client,
    model: Model,
    context: GraphProviderContext,
    factory: Arc<GraphValueFactory>,
    options: StreamOptions,
) -> crate::replay_safe_retry::actual::ActualAttemptStream {
    let (sink, events, failures) = ActualAssistantStream::channel();
    let error = Arc::new(Mutex::new(None));
    let attempt_error = error.clone();
    tokio::spawn(async move {
        let _settlement = failures.settlement_guard();
        let mut state = match GraphChunkState::new(&model, factory.clone(), failures.clone()) {
            Ok(state) => state,
            Err(error) => {
                failures.record(error);
                return;
            }
        };
        let started = Instant::now();
        let result = if options.cancel.is_cancelled() {
            Err(ProviderError::Aborted)
        } else if model.base_url.trim_end_matches('/').is_empty() {
            Err(ProviderError::Config("OpenAI request setup did not resolve a base URL".into()))
        } else {
            let mut disabled =
                options.provider_session_state.as_ref().is_some_and(|session| session.strict_tools_disabled(&model));
            let mut retried = false;
            loop {
                let prepared = match build_request_and_compat(
                    &model,
                    &context,
                    &options,
                    factory.clone(),
                    disabled.then_some(super::ToolStrictMode::None),
                ) {
                    Ok(prepared) => prepared,
                    Err(error) => break Err(state.graph_error(error)),
                };
                if let Err(error) = state.configure_stream(&prepared.compat) {
                    break Err(state.graph_error(error));
                }
                observe_prepared(&model, &options, &prepared.bytes);
                state.captured_http_error = None;
                let attempt = super::run_prepared(&client, &model, &options, &prepared.bytes, &mut state, &sink).await;
                let Err(cause) = &attempt else {
                    break attempt;
                };
                // Only an explicit pre-acceptance HTTP rejection is eligible.
                // A streamed error, callback failure or unknown transport
                // outcome must never replay accepted tool/model side effects.
                if retried
                    || state.accepted.get()
                    || state.saw_frame
                    || state.retry_blocked
                    || options.cancel.is_cancelled()
                    || !matches!(cause, ProviderError::Http { .. })
                {
                    break attempt;
                }
                let grammar_retry = prepared.retry_on_grammar_error
                    && !disabled
                    && grammar_rejection(cause, state.captured_http_error.as_ref());
                let retry = if grammar_retry {
                    true
                } else {
                    // Fixed retry context evaluates context.tools before its
                    // strict/status predicates, preserving this original getter.
                    let tools = match context.original_field(factory.as_ref(), "tools") {
                        Ok(tools) => tools,
                        Err(error) => break Err(state.graph_error(error)),
                    };
                    match strict_rejection(
                        &model,
                        cause,
                        state.captured_http_error.as_ref(),
                        prepared.strict_applied,
                        &tools,
                        crate::graph_history::NativeExecution { heap: factory.heap(), runtime: factory.runtime() },
                    ) {
                        Ok(retry) => retry,
                        Err(error) => break Err(state.graph_error(error)),
                    }
                };
                if !retry {
                    break attempt;
                }
                if let Some(session) = &options.provider_session_state {
                    session.disable_strict_tools(&model);
                }
                disabled = true;
                retried = true;
            }
        };
        if let Err(cause) = &result {
            *attempt_error.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Some(crate::replay_safe_retry::AttemptError {
                    cause: cause.clone(),
                    retry_blocked: state.retry_blocked,
                });
        }
        let mut closing = Vec::new();
        match state.settle(started, result, &mut closing) {
            Ok(terminal) => {
                for event in closing {
                    if !sink.push_or_cancel(event, &options.cancel).await {
                        break;
                    }
                }
                let _ = sink.push(terminal).await;
            }
            Err(error) => failures.record(error),
        }
    });
    crate::replay_safe_retry::actual::ActualAttemptStream { events, error }
}

fn observe_prepared(model: &Model, options: &StreamOptions, bytes: &[u8]) {
    if let Some(observer) = &options.request_text_observer {
        let sequence = super::NEXT_REQUEST_OBSERVATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let measurement = match serde_json::from_slice(bytes) {
            Ok(value) => super::measure_prepared_message_text(model, &value),
            Err(_) => super::PreparedMessageTextMeasurement {
                count: super::PreparedTextCount::UnsupportedShape,
                coverage_complete: false,
                text_fields: 0,
                text_bytes: 0,
                has_tool_definitions: false,
                has_tool_calls: false,
                has_images: false,
            },
        };
        observer.report(super::PreparedRequestTextObservation { sequence, measurement });
    }
}
fn rejection_message(cause: &ProviderError, captured: Option<&super::CapturedHttpError>) -> String {
    let mut message = cause.to_string();
    if let Some(captured) = captured
        && !captured.body_text.trim().is_empty()
    {
        message.push('\n');
        message.push_str(&captured.body_text);
    }
    message
}
fn grammar_rejection(cause: &ProviderError, captured: Option<&super::CapturedHttpError>) -> bool {
    if cause.status() != Some(400) {
        return false;
    }
    let message = rejection_message(cause, captured).to_ascii_lowercase();
    message.contains("invalid_request_error") && message.contains("compiled grammar") && message.contains("too large")
}
static STRICT_REJECTION: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
    r"(?i)wrong_api_format|mixed values for 'strict'|tool[s]?\b.*strict|\bstrict\b.*tool|tool parameters? schema|invalid schema for function|structured[_ -]?outputs?\b[^\n]*(?:not (?:supported|available|enabled)|unsupported)|(?:not support|unsupported)[^\n]*structured[_ -]?outputs?\b"
).expect("fixed OMP strict retry pattern")
});
static OPENROUTER_GENERIC: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)^(?:400\s+)?Provider returned error$").expect("fixed OMP generic rejection pattern")
});
fn strict_rejection(
    model: &Model,
    cause: &ProviderError,
    captured: Option<&super::CapturedHttpError>,
    applied: bool,
    tools: &ValueHandle,
    ex: crate::graph_history::NativeExecution<'_>,
) -> Result<bool, ValueError> {
    if !ex.heap.truthy(tools)? || ex.number_is(&ex.get(tools, "length")?, 0.0)? || !applied {
        return Ok(false);
    }
    if !matches!(cause.status(), Some(400 | 422)) {
        return Ok(false);
    }
    if STRICT_REJECTION.is_match(&rejection_message(cause, captured)) {
        return Ok(true);
    }
    if model.provider != "openrouter" || !OPENROUTER_GENERIC.is_match(cause.to_string().trim()) {
        return Ok(false);
    }
    if let Some(raw) = captured
        .and_then(|captured| captured.body_json.as_ref())
        .and_then(|body| body.get("error"))
        .and_then(|error| error.get("metadata"))
        .and_then(|metadata| metadata.get("raw"))
    {
        if raw.as_str().map_or(!raw.is_null(), |text| !text.trim().is_empty()) {
            return Ok(false);
        }
    }
    Ok(true)
}
