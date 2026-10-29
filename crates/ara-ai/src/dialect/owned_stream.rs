//! Fixed OMP596f2da7101178214aa27a753529d15e6b7ad91d owned-stream.ts.
//! MIT; Copyright (c) 2025 Mario Zechner; (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
//! Original same-lineage roots/arguments/symbols, shared actual scanners and
//! independent stream failure/settlement owners. No typed message conversion.

use super::{
    ActualInbandScanEvent, DeepSeekInbandUnitsScanner, Dialect, HermesInbandUnitsScanner, InbandNameValue,
    InbandScanEvent, Qwen3InbandUnitsScanner,
};
use crate::graph_history::NativeExecution;
use crate::graph_values::{GraphExecution, GraphValueFactory, cursor_exec_resolved_key, streaming_partial_json_key};
use crate::{ActualAssistantEvent, ActualAssistantStream, AssistantMessageEvent as Event, StopReason};
use ara_rpc::{
    WireString,
    value_heap::{
        CompiledFunctionForm, CompiledFunctionSpec, PropertyKey, ValueError, ValueHandle, ValueHeap, ValueKind,
        ValueRuntime, ValueSnapshot,
    },
};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

type Result<T> = std::result::Result<T, ValueError>;
fn execution(factory: &GraphValueFactory) -> NativeExecution<'_> {
    NativeExecution { heap: factory.heap(), runtime: factory.runtime() }
}
fn property(ex: NativeExecution<'_>, value: &ValueHandle) -> Result<PropertyKey> {
    let primitive = ex.heap.to_primitive(value, true, ex.runtime)?;
    match ex.heap.snapshot(&primitive)? {
        ValueSnapshot::Symbol { key, .. } => Ok(PropertyKey::Symbol(key)),
        _ => Ok(PropertyKey::String(ex.heap.to_string(&primitive, ex.runtime)?)),
    }
}
fn indexed(ex: NativeExecution<'_>, array: &ValueHandle, index: &ValueHandle) -> Result<ValueHandle> {
    ex.heap.get(array, &property(ex, index)?, ex.runtime)
}
fn undefined(ex: NativeExecution<'_>, value: &ValueHandle) -> Result<bool> {
    ex.undefined(value)
}
fn string_kind(ex: NativeExecution<'_>, value: &ValueHandle) -> Result<bool> {
    Ok(ex.heap.kind(value)? == ValueKind::String)
}
fn object_kind(ex: NativeExecution<'_>, value: &ValueHandle) -> Result<bool> {
    Ok(matches!(ex.heap.kind(value)?, ValueKind::Object | ValueKind::Array))
}
fn map_get(ex: NativeExecution<'_>, map: &ValueHandle, key: ValueHandle) -> Result<ValueHandle> {
    ex.method_lazy(map, "get", || Ok(vec![key]))
}
fn set_add(ex: NativeExecution<'_>, set: &ValueHandle, value: ValueHandle) -> Result<()> {
    ex.method_lazy(set, "add", || Ok(vec![value])).map(|_| ())
}
fn collect_types(ex: NativeExecution<'_>, schema: &ValueHandle, out: &ValueHandle, depth: usize) -> Result<()> {
    if depth > 8 || !ex.heap.truthy(schema)? || !object_kind(ex, schema)? || ex.heap.kind(schema)? == ValueKind::Array {
        return Ok(());
    }
    let kind = ex.get(schema, "type")?;
    if string_kind(ex, &kind)? {
        set_add(ex, out, kind.clone())?;
    } else if ex.heap.kind(&kind)? == ValueKind::Array {
        ex.for_of(&kind, |value| {
            if string_kind(ex, value)? {
                set_add(ex, out, value.clone())?;
            }
            Ok(true)
        })?;
    }
    if undefined(ex, &kind)? {
        let enumeration = ex.get(schema, "enum")?;
        if ex.heap.kind(&enumeration)? == ValueKind::Array {
            ex.for_of(&ex.get(schema, "enum")?, |value| {
                let name = match ex.heap.kind(value)? {
                    ValueKind::Null => "null",
                    ValueKind::Number | ValueKind::BigInt => "number",
                    ValueKind::Boolean => "boolean",
                    ValueKind::String => "string",
                    _ => "object",
                };
                set_add(ex, out, ex.text(name)?)?;
                Ok(true)
            })?;
        }
        if ex.heap.has(schema, &"const".into(), ex.runtime)? {
            let value = ex.get(schema, "const")?;
            let name = match ex.heap.kind(&value)? {
                ValueKind::Null => "null",
                ValueKind::Number | ValueKind::BigInt => "number",
                ValueKind::Boolean => "boolean",
                ValueKind::String => "string",
                _ => "object",
            };
            set_add(ex, out, ex.text(name)?)?;
        }
    }
    for key in ["anyOf", "oneOf", "allOf"] {
        let branch = ex.get(schema, key)?;
        if ex.heap.kind(&branch)? == ValueKind::Array {
            ex.for_of(&branch, |sub| {
                collect_types(ex, sub, out, depth + 1)?;
                Ok(true)
            })?;
        }
    }
    Ok(())
}
fn string_args_resolver(ex: NativeExecution<'_>, tools: &ValueHandle) -> Result<ValueHandle> {
    let shapes = ex.heap.new_map(&ex.heap.undefined()?, ex.runtime)?;
    ex.for_of(tools, |tool| {
        let schema = match crate::schema_wire::graph_tool_wire_schema_with_execution(
            GraphExecution::new(ex.heap, ex.runtime),
            tool,
        ) {
            Ok(schema) => schema,
            Err(error) if error.is_native_exception() => {
                let parameters = ex.get(tool, "parameters")?;
                if ex.heap.truthy(&parameters)? && ex.heap.kind(&parameters)? == ValueKind::Object {
                    parameters
                } else {
                    ex.heap.object()?
                }
            }
            Err(error) => return Err(error),
        };
        let props = ex.get(&schema, "properties")?;
        let properties = if ex.heap.truthy(&props)? && ex.heap.kind(&props)? == ValueKind::Object {
            props
        } else {
            ex.heap.object()?
        };
        let strings = ex.heap.new_set(&ex.heap.undefined()?, ex.runtime)?;
        let order = ex.heap.array_with_length(0)?;
        for key in ex.heap.for_in_string_keys(&properties, ex.runtime)? {
            let key = ex.text(key.clone())?;
            ex.method_lazy(&order, "push", || Ok(vec![key.clone()]))?;
            let types = ex.heap.new_set(&ex.heap.undefined()?, ex.runtime)?;
            collect_types(ex, &ex.heap.get(&properties, &property(ex, &key)?, ex.runtime)?, &types, 0)?;
            ex.method_lazy(&types, "delete", || Ok(vec![ex.text("null")?]))?;
            if ex.number_is(&ex.get(&types, "size")?, 1.0)?
                && ex.heap.truthy(&ex.method_lazy(&types, "has", || Ok(vec![ex.text("string")?]))?)?
            {
                set_add(ex, &strings, key)?;
            }
        }
        ex.method_lazy(&shapes, "set", || {
            let name = ex.get(tool, "name")?;
            let shape = ex.object([("stringArgs", strings), ("properties", properties), ("parameterOrder", order)])?;
            Ok(vec![name, shape])
        })?;
        Ok(true)
    })?;
    let empty = ex.heap.new_set(&ex.heap.undefined()?, ex.runtime)?;
    ex.heap.fresh_compiled_function(&RESOLVE_STRINGS, &[shapes, empty])
}
static RESOLVE_STRINGS: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da:coercion.ts:buildStringArgsResolver returned arrow",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 2,
    body: |heap, captures, _, args, runtime| {
        let ex = NativeExecution { heap, runtime };
        let name = args.first().cloned().map(Ok).unwrap_or_else(|| heap.undefined())?;
        let shape = map_get(ex, &captures[0], name)?;
        let strings = if ex.nullish(&shape)? { heap.undefined()? } else { ex.get(&shape, "stringArgs")? };
        if ex.nullish(&strings)? { Ok(captures[1].clone()) } else { Ok(strings) }
    },
};

enum Scanner {
    Deepseek(Box<DeepSeekInbandUnitsScanner<GraphValueFactory>>),
    Hermes(Box<HermesInbandUnitsScanner<GraphValueFactory>>),
    Qwen3(Box<Qwen3InbandUnitsScanner<GraphValueFactory>>),
}
impl Scanner {
    fn new(factory: Arc<GraphValueFactory>, dialect: Dialect) -> Result<Self> {
        Ok(match dialect {
            Dialect::Deepseek => {
                Self::Deepseek(Box::new(DeepSeekInbandUnitsScanner::with_parse_thinking(factory, true)?))
            }
            Dialect::Hermes => Self::Hermes(Box::new(HermesInbandUnitsScanner::with_parse_thinking(factory, true))),
            Dialect::Qwen3 => Self::Qwen3(Box::new(Qwen3InbandUnitsScanner::with_parse_thinking(factory, true))),
            _ => {
                return Err(ValueError::UnsupportedOperation(
                    "Owned Graph stream scanner for this fixed dialect is not connected",
                ));
            }
        })
    }
    fn feed(&mut self, text: &WireString) -> Result<Vec<ActualInbandScanEvent<ValueHandle>>> {
        match self {
            Self::Deepseek(scanner) => scanner.try_feed_units(text),
            Self::Hermes(scanner) => scanner.try_feed_units(text),
            Self::Qwen3(scanner) => scanner.try_feed_units(text),
        }
    }
    fn flush(&mut self) -> Result<Vec<ActualInbandScanEvent<ValueHandle>>> {
        match self {
            Self::Deepseek(scanner) => scanner.try_flush_units(),
            Self::Hermes(scanner) => scanner.try_flush_units(),
            Self::Qwen3(scanner) => scanner.try_flush_units(),
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum ToolChannel {
    Native,
    Inband,
}

struct Projector {
    factory: Arc<GraphValueFactory>,
    globals: ValueHandle,
    scanner: Scanner,
    partial: ValueHandle,
    text: Option<ValueHandle>,
    thinking: Option<ValueHandle>,
    tool_blocks: ValueHandle,
    native_blocks: ValueHandle,
    tool_channel: Option<ToolChannel>,
    fed_len: usize,
    stopped: bool,
    response_pending: WireString,
    response_tokens: Vec<WireString>,
    response_overlap: usize,
    events: Vec<ActualAssistantEvent>,
    emit_events: bool,
}
impl Projector {
    fn new(
        factory: Arc<GraphValueFactory>,
        globals: &ValueHandle,
        tools: &ValueHandle,
        dialect: Dialect,
        seed: &ValueHandle,
        emit_events: bool,
    ) -> Result<Self> {
        let ex = execution(&factory);
        // Original class field initializers precede the constructor body.
        let tool_blocks = ex.heap.new_map(&ex.heap.undefined()?, ex.runtime)?;
        let native_blocks = ex.heap.new_map(&ex.heap.undefined()?, ex.runtime)?;
        let _string_args = string_args_resolver(ex, tools)?;
        let scanner = Scanner::new(factory.clone(), dialect)?;
        let tokens = match dialect {
            Dialect::Deepseek => vec!["<｜tool▁outputs▁begin｜>", "<｜tool▁output▁begin｜>"],
            Dialect::Hermes | Dialect::Qwen3 => vec!["<tool_response>"],
            _ => unreachable!(),
        };
        let response_tokens = tokens.into_iter().map(WireString::from).collect::<Vec<_>>();
        let response_overlap =
            response_tokens.iter().map(|token| token.units().len().saturating_sub(1)).max().unwrap_or(0);
        let partial = ex.copy_lazy(seed, "content", || ex.heap.array_with_length(0))?;
        let mut projector = Self {
            factory,
            globals: globals.clone(),
            scanner,
            partial,
            text: None,
            thinking: None,
            tool_blocks,
            native_blocks,
            tool_channel: None,
            fed_len: 0,
            stopped: false,
            response_pending: "".into(),
            response_tokens,
            response_overlap,
            events: Vec::new(),
            emit_events,
        };
        projector.emit(Event::Start { partial: projector.partial.clone() });
        Ok(projector)
    }
    fn emit(&mut self, event: ActualAssistantEvent) {
        if self.emit_events {
            self.events.push(event);
        }
    }
    fn content_block(&self, ex: NativeExecution<'_>, entry: &ValueHandle) -> Result<ValueHandle> {
        indexed(ex, &ex.get(&self.partial, "content")?, &ex.get(entry, "index")?)
    }
    fn last_index(&self, ex: NativeExecution<'_>) -> Result<ValueHandle> {
        let length = ex.get(&ex.get(&self.partial, "content")?, "length")?;
        ex.heap.number(ex.heap.to_number(&length, ex.runtime)? - 1.0)
    }
    fn push_block(&self, ex: NativeExecution<'_>, block: impl FnOnce() -> Result<ValueHandle>) -> Result<()> {
        ex.method_lazy(&ex.get(&self.partial, "content")?, "push", || Ok(vec![block()?])).map(|_| ())
    }
    fn keep(&mut self, block: ValueHandle) -> Result<()> {
        self.close_text()?;
        self.close_thinking()?;
        let factory = self.factory.clone();
        let ex = execution(&factory);
        self.push_block(ex, || Ok(block.clone()))?;
        if self.emit_events && ex.eq(&block, "type", "image")? {
            self.emit(Event::ImageEnd {
                content_index: self.last_index(ex)?,
                content: block,
                partial: self.partial.clone(),
            });
        }
        Ok(())
    }
    fn has_named(ex: NativeExecution<'_>, source: Option<&ValueHandle>) -> Result<bool> {
        let Some(source) = source else {
            return Ok(false);
        };
        let trimmed = ex.method_lazy(&ex.get(source, "name")?, "trim", || Ok(Vec::new()))?;
        crate::schema_validator::graph_values_compare_with_execution(
            GraphExecution::new(ex.heap, ex.runtime),
            &ex.get(&trimmed, "length")?,
            &ex.heap.number(0.0)?,
            ">",
        )
    }
    fn copy_symbols(ex: NativeExecution<'_>, target: &ValueHandle, source: &ValueHandle, sync: bool) -> Result<()> {
        let key = streaming_partial_json_key(ex.heap)?;
        let partial = ex.heap.get(source, &key, ex.runtime)?;
        if !undefined(ex, &partial)? {
            ex.heap.assign(target, key.clone(), partial, ex.runtime)?;
        } else if sync && ex.heap.has_own(target, &key, ex.runtime)? {
            ex.heap.assign(target, key, ex.heap.undefined()?, ex.runtime)?;
        }
        let cursor = cursor_exec_resolved_key(ex.heap)?;
        if matches!(ex.heap.snapshot(&ex.heap.get(source, &cursor, ex.runtime)?)?, ValueSnapshot::Boolean(true)) {
            ex.heap.assign(target, cursor, ex.heap.boolean(true)?, ex.runtime)?;
        }
        Ok(())
    }
    fn clone_call(ex: NativeExecution<'_>, source: &ValueHandle) -> Result<ValueHandle> {
        let block = ex.heap.object()?;
        ex.own(&block, "type", ex.text("toolCall")?)?;
        for field in ["id", "name", "arguments"] {
            ex.own(&block, field, ex.get(source, field)?)?;
        }
        if !undefined(ex, &ex.get(source, "rawBlock")?)? {
            ex.own(&block, "rawBlock", ex.get(source, "rawBlock")?)?;
        }
        Self::copy_symbols(ex, &block, source, false)?;
        Ok(block)
    }
    fn sync_call(ex: NativeExecution<'_>, target: &ValueHandle, source: &ValueHandle) -> Result<()> {
        for field in ["id", "name", "arguments", "rawBlock"] {
            ex.put(target, field, ex.get(source, field)?)?;
        }
        Self::copy_symbols(ex, target, source, true)
    }
    fn native_start(&mut self, source_index: ValueHandle, source: Option<ValueHandle>) -> Result<()> {
        let factory = self.factory.clone();
        let ex = execution(&factory);
        if self.stopped || !Self::has_named(ex, source.as_ref())? || self.tool_channel == Some(ToolChannel::Inband) {
            return Ok(());
        }
        self.tool_channel = Some(ToolChannel::Native);
        self.close_text()?;
        self.close_thinking()?;
        let block = Self::clone_call(ex, source.as_ref().expect("checked named source"))?;
        self.push_block(ex, || Ok(block.clone()))?;
        let index = self.last_index(ex)?;
        ex.method_lazy(&self.native_blocks, "set", || {
            Ok(vec![source_index, ex.object([("index", index.clone()), ("block", block)])?])
        })?;
        self.emit(Event::ToolcallStart { content_index: index, partial: self.partial.clone() });
        Ok(())
    }
    fn native_delta(
        &mut self,
        source_index: ValueHandle,
        delta: ValueHandle,
        source: Option<ValueHandle>,
    ) -> Result<()> {
        if self.stopped {
            return Ok(());
        }
        let factory = self.factory.clone();
        let ex = execution(&factory);
        let mut entry = map_get(ex, &self.native_blocks, source_index.clone())?;
        if !ex.heap.truthy(&entry)?
            && Self::has_named(ex, source.as_ref())?
            && self.tool_channel != Some(ToolChannel::Inband)
        {
            self.native_start(source_index.clone(), source.clone())?;
            entry = map_get(ex, &self.native_blocks, source_index)?;
        }
        if !ex.heap.truthy(&entry)? {
            return Ok(());
        }
        if let Some(source) = source {
            Self::sync_call(ex, &ex.get(&entry, "block")?, &source)?;
        }
        self.emit(Event::ToolcallDelta {
            content_index: ex.get(&entry, "index")?,
            delta,
            partial: self.partial.clone(),
        });
        Ok(())
    }
    fn native_end(&mut self, source_index: ValueHandle, source: ValueHandle) -> Result<()> {
        if self.stopped {
            return Ok(());
        }
        let factory = self.factory.clone();
        let ex = execution(&factory);
        let entry = map_get(ex, &self.native_blocks, source_index.clone())?;
        if ex.heap.truthy(&entry)? {
            Self::sync_call(ex, &ex.get(&entry, "block")?, &source)?;
            self.emit(Event::ToolcallEnd {
                content_index: ex.get(&entry, "index")?,
                tool_call: ex.get(&entry, "block")?,
                partial: self.partial.clone(),
            });
            ex.method_lazy(&self.native_blocks, "delete", || Ok(vec![source_index]))?;
            return Ok(());
        }
        if !Self::has_named(ex, Some(&source))? || self.tool_channel == Some(ToolChannel::Inband) {
            return Ok(());
        }
        self.tool_channel = Some(ToolChannel::Native);
        self.close_text()?;
        self.close_thinking()?;
        let block = Self::clone_call(ex, &source)?;
        self.push_block(ex, || Ok(block.clone()))?;
        let index = self.last_index(ex)?;
        self.emit(Event::ToolcallStart { content_index: index.clone(), partial: self.partial.clone() });
        self.emit(Event::ToolcallEnd { content_index: index, tool_call: block, partial: self.partial.clone() });
        Ok(())
    }
    fn text(&mut self, delta: WireString) -> Result<bool> {
        if self.stopped {
            return Ok(true);
        }
        self.fed_len += delta.units().len();
        let mut combined = self.response_pending.units().to_vec();
        combined.extend_from_slice(delta.units());
        let response_index =
            self.response_tokens.iter().filter_map(|token| super::units::find_units(&combined, token.units())).min();
        if let Some(index) = response_index {
            self.response_pending = "".into();
            let events = self.scanner.feed(&WireString::from_units(combined[..index].to_vec()))?;
            self.apply(events)?;
            self.stopped = true;
            return Ok(true);
        }
        if combined.len() <= self.response_overlap {
            self.response_pending = WireString::from_units(combined);
            return Ok(false);
        }
        let length = combined.len() - self.response_overlap;
        self.response_pending = WireString::from_units(combined[length..].to_vec());
        let events = self.scanner.feed(&WireString::from_units(combined[..length].to_vec()))?;
        self.apply(events)?;
        Ok(false)
    }
    fn thinking_start(&mut self) -> Result<()> {
        if self.stopped {
            return Ok(());
        }
        self.close_text()?;
        if self.thinking.is_some() {
            return Ok(());
        }
        let factory = self.factory.clone();
        let ex = execution(&factory);
        let block = ex.object([("type", ex.text("thinking")?), ("thinking", ex.text("")?)])?;
        self.push_block(ex, || Ok(block))?;
        let index = self.last_index(ex)?;
        self.thinking = Some(ex.object([("index", index.clone()), ("text", ex.text("")?)])?);
        self.emit(Event::ThinkingStart { content_index: index, partial: self.partial.clone() });
        Ok(())
    }
    fn thinking_delta(&mut self, delta: WireString) -> Result<()> {
        if self.stopped {
            return Ok(());
        }
        if self.thinking.is_none() {
            self.thinking_start()?;
        }
        let Some(thinking) = self.thinking.clone() else {
            return Ok(());
        };
        let factory = self.factory.clone();
        let ex = execution(&factory);
        let block = self.content_block(ex, &thinking)?;
        let actual_delta = ex.text(delta.clone())?;
        ex.put(&block, "thinking", ex.heap.add(&ex.get(&block, "thinking")?, &actual_delta, ex.runtime)?)?;
        ex.put(&thinking, "text", ex.heap.add(&ex.get(&thinking, "text")?, &actual_delta, ex.runtime)?)?;
        self.emit(Event::ThinkingDelta {
            content_index: ex.get(&thinking, "index")?,
            delta,
            partial: self.partial.clone(),
        });
        Ok(())
    }
    fn emit_text(&mut self, text: WireString) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        self.close_thinking()?;
        let factory = self.factory.clone();
        let ex = execution(&factory);
        if self.text.is_none() {
            self.push_block(ex, || ex.object([("type", ex.text("text")?), ("text", ex.text("")?)]))?;
            let index = self.last_index(ex)?;
            self.text = Some(ex.object([("index", index.clone())])?);
            self.emit(Event::TextStart { content_index: index, partial: self.partial.clone() });
        }
        let entry = self.text.clone().expect("text initialized");
        let block = self.content_block(ex, &entry)?;
        ex.put(&block, "text", ex.heap.add(&ex.get(&block, "text")?, &ex.text(text.clone())?, ex.runtime)?)?;
        self.emit(Event::TextDelta {
            content_index: ex.get(&entry, "index")?,
            delta: text,
            partial: self.partial.clone(),
        });
        Ok(())
    }
    fn close_text(&mut self) -> Result<()> {
        let Some(entry) = self.text.clone() else {
            return Ok(());
        };
        let factory = self.factory.clone();
        let ex = execution(&factory);
        let block = self.content_block(ex, &entry)?;
        if self.emit_events {
            self.emit(Event::TextEnd {
                content_index: ex.get(&entry, "index")?,
                content: ex.string(&ex.get(&block, "text")?)?,
                partial: self.partial.clone(),
            });
        }
        self.text = None;
        Ok(())
    }
    fn close_thinking(&mut self) -> Result<()> {
        let Some(entry) = self.thinking.clone() else {
            return Ok(());
        };
        let factory = self.factory.clone();
        let ex = execution(&factory);
        let block = self.content_block(ex, &entry)?;
        if self.emit_events {
            self.emit(Event::ThinkingEnd {
                content_index: ex.get(&entry, "index")?,
                content: ex.string(&ex.get(&block, "thinking")?)?,
                partial: self.partial.clone(),
            });
        }
        self.thinking = None;
        Ok(())
    }
    fn name(ex: NativeExecution<'_>, name: InbandNameValue<ValueHandle>) -> Result<ValueHandle> {
        match name {
            InbandNameValue::Source(name) => ex.text(name),
            InbandNameValue::Actual(name) => ex.heap.adopt(&name),
        }
    }
    fn begin_tool(&mut self, id: WireString, name: InbandNameValue<ValueHandle>) -> Result<()> {
        if self.tool_channel == Some(ToolChannel::Native) {
            return Ok(());
        }
        self.tool_channel = Some(ToolChannel::Inband);
        self.close_text()?;
        self.close_thinking()?;
        let factory = self.factory.clone();
        let ex = execution(&factory);
        let key = ex.text(id)?;
        if ex.heap.truthy(&ex.method_lazy(&self.tool_blocks, "has", || Ok(vec![key.clone()]))?)? {
            return Ok(());
        }
        let block = ex.object([
            ("type", ex.text("toolCall")?),
            ("id", key.clone()),
            ("name", Self::name(ex, name)?),
            ("arguments", ex.heap.object()?),
        ])?;
        self.push_block(ex, || Ok(block.clone()))?;
        let index = self.last_index(ex)?;
        let entry = ex.object([("index", index.clone()), ("block", block), ("rawValue", ex.text("")?)])?;
        ex.method_lazy(&self.tool_blocks, "set", || Ok(vec![key, entry]))?;
        self.emit(Event::ToolcallStart { content_index: index, partial: self.partial.clone() });
        Ok(())
    }
    fn delta_tool(
        &mut self,
        id: WireString,
        name: InbandNameValue<ValueHandle>,
        key: WireString,
        delta: WireString,
    ) -> Result<()> {
        let factory = self.factory.clone();
        let ex = execution(&factory);
        let id_value = ex.text(id.clone())?;
        let mut entry = map_get(ex, &self.tool_blocks, id_value.clone())?;
        if !ex.heap.truthy(&entry)? {
            self.begin_tool(id, name)?;
            entry = map_get(ex, &self.tool_blocks, id_value)?;
        }
        if !ex.heap.truthy(&entry)? {
            return Ok(());
        }
        let key_value = ex.text(key)?;
        if !ex.strict(&ex.get(&entry, "currentKey")?, &key_value)? {
            ex.put(&entry, "currentKey", key_value.clone())?;
            let block = ex.get(&entry, "block")?;
            let args = ex.get(&block, "arguments")?;
            let value = indexed(ex, &args, &key_value)?;
            let initial = if string_kind(ex, &value)? {
                let string = ex.get(&self.globals, "String")?;
                let value = indexed(ex, &ex.get(&ex.get(&entry, "block")?, "arguments")?, &key_value)?;
                ex.heap.call(&string, &ex.heap.undefined()?, &[value], ex.runtime)?
            } else {
                ex.text("")?
            };
            ex.put(&entry, "rawValue", initial)?;
        }
        ex.put(&entry, "rawValue", ex.heap.add(&ex.get(&entry, "rawValue")?, &ex.text(delta.clone())?, ex.runtime)?)?;
        let args = ex.get(&ex.get(&entry, "block")?, "arguments")?;
        ex.heap.assign(&args, property(ex, &key_value)?, ex.get(&entry, "rawValue")?, ex.runtime)?;
        self.emit(Event::ToolcallDelta {
            content_index: ex.get(&entry, "index")?,
            delta: ex.text(delta)?,
            partial: self.partial.clone(),
        });
        Ok(())
    }
    fn end_tool(
        &mut self,
        id: WireString,
        name: InbandNameValue<ValueHandle>,
        arguments: ValueHandle,
        raw: Option<WireString>,
    ) -> Result<()> {
        let factory = self.factory.clone();
        let ex = execution(&factory);
        let key = ex.text(id.clone())?;
        let mut entry = map_get(ex, &self.tool_blocks, key.clone())?;
        if !ex.heap.truthy(&entry)? {
            self.begin_tool(id, name.clone())?;
            entry = map_get(ex, &self.tool_blocks, key.clone())?;
        }
        if !ex.heap.truthy(&entry)? {
            return Ok(());
        }
        ex.put(&ex.get(&entry, "block")?, "name", Self::name(ex, name)?)?;
        ex.put(&ex.get(&entry, "block")?, "arguments", arguments)?;
        if let Some(raw) = raw {
            ex.put(&ex.get(&entry, "block")?, "rawBlock", ex.text(raw)?)?;
        }
        self.emit(Event::ToolcallEnd {
            content_index: ex.get(&entry, "index")?,
            tool_call: ex.get(&entry, "block")?,
            partial: self.partial.clone(),
        });
        ex.method_lazy(&self.tool_blocks, "delete", || Ok(vec![key]))?;
        Ok(())
    }
    fn apply(&mut self, events: Vec<ActualInbandScanEvent<ValueHandle>>) -> Result<()> {
        for event in events {
            match event {
                InbandScanEvent::Text { text } => self.emit_text(text)?,
                InbandScanEvent::ThinkingStart => self.thinking_start()?,
                InbandScanEvent::ThinkingDelta { delta } => self.thinking_delta(delta)?,
                InbandScanEvent::ThinkingEnd { .. } => self.close_thinking()?,
                InbandScanEvent::ImpliedThinkingEnd => {}
                InbandScanEvent::ToolStart { id, name } => self.begin_tool(id, name)?,
                InbandScanEvent::ToolArgDelta { id, name, key, delta } => self.delta_tool(id, name, key, delta)?,
                InbandScanEvent::ToolEnd { id, name, arguments, raw_block } => {
                    self.end_tool(id, name, arguments, raw_block)?
                }
            }
        }
        Ok(())
    }
    fn finish(&mut self, message: &ValueHandle, emit_done: bool) -> Result<ValueHandle> {
        let factory = self.factory.clone();
        let ex = execution(&factory);
        let mut full = ex.text("")?;
        ex.for_of(&ex.get(message, "content")?, |block| {
            if ex.eq(block, "type", "text")? {
                full = ex.heap.add(&full, &ex.get(block, "text")?, ex.runtime)?;
            }
            Ok(true)
        })?;
        let full = ex.string(&full)?;
        if !self.stopped && full.units().len() > self.fed_len {
            self.text(WireString::from_units(full.units()[self.fed_len..].to_vec()))?;
        }
        if !self.stopped && !self.response_pending.is_empty() {
            let pending = self.response_pending.clone();
            let events = self.scanner.feed(&pending)?;
            self.apply(events)?;
            self.response_pending = "".into();
        }
        let events = self.scanner.flush()?;
        self.apply(events)?;
        self.close_text()?;
        self.close_thinking()?;
        let callback = ex.heap.fresh_compiled_function(&IS_TOOL_CALL, &[])?;
        let has_tools =
            ex.heap.truthy(&ex.method_lazy(&ex.get(&self.partial, "content")?, "some", || Ok(vec![callback]))?)?;
        let reason = if has_tools && !ex.eq(message, "stopReason", "length")? {
            StopReason::ToolUse
        } else if ex.eq(message, "stopReason", "length")? {
            StopReason::Length
        } else {
            StopReason::Stop
        };
        let final_message = ex.heap.spread(message, ex.runtime)?;
        ex.own(&final_message, "content", ex.get(&self.partial, "content")?)?;
        ex.own(
            &final_message,
            "stopReason",
            ex.text(match reason {
                StopReason::ToolUse => "toolUse",
                StopReason::Length => "length",
                _ => "stop",
            })?,
        )?;
        if emit_done {
            self.emit(Event::Done { reason, message: final_message.clone() });
        }
        Ok(final_message)
    }
}
static IS_TOOL_CALL: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da:owned-stream.ts:finish.content.some",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: |heap, _, _, args, runtime| {
        let block = args.first().cloned().map(Ok).unwrap_or_else(|| heap.undefined())?;
        heap.boolean(NativeExecution { heap, runtime }.eq(&block, "type", "toolCall")?)
    },
};

fn source_call(factory: &GraphValueFactory, partial: &ValueHandle, index: &ValueHandle) -> Result<Option<ValueHandle>> {
    let ex = execution(factory);
    let source = indexed(ex, &ex.get(partial, "content")?, index)?;
    if !ex.nullish(&source)? && ex.eq(&source, "type", "toolCall")? { Ok(Some(source)) } else { Ok(None) }
}
fn project_event(
    projector: &mut Option<Projector>,
    factory: &Arc<GraphValueFactory>,
    globals: &ValueHandle,
    tools: &ValueHandle,
    dialect: Dialect,
    event: ActualAssistantEvent,
    abort_on_fabrication: bool,
) -> Result<(Vec<ActualAssistantEvent>, bool, bool)> {
    let mut abort = false;
    let mut terminal = false;
    let mut direct = Vec::new();
    match event {
        Event::Start { partial } => {
            *projector = Some(Projector::new(factory.clone(), globals, tools, dialect, &partial, true)?)
        }
        Event::ThinkingStart { .. } => {
            if let Some(p) = projector {
                p.thinking_start()?;
            }
        }
        Event::ThinkingDelta { delta, .. } => {
            if let Some(p) = projector {
                p.thinking_delta(delta)?;
            }
        }
        Event::ThinkingEnd { .. } => {
            if let Some(p) = projector {
                p.close_thinking()?;
            }
        }
        Event::ImageEnd { content, .. } => {
            if let Some(p) = projector {
                p.keep(content)?;
            }
        }
        Event::TextDelta { delta, partial, .. } => {
            if let Some(p) = projector {
                if p.text(delta)? && abort_on_fabrication {
                    p.finish(&partial, true)?;
                    abort = true;
                    terminal = true;
                }
            }
        }
        Event::ToolcallStart { content_index, partial } => {
            let source = source_call(factory, &partial, &content_index)?;
            if let Some(p) = projector {
                p.native_start(content_index, source)?;
            }
        }
        Event::ToolcallDelta { content_index, delta, partial } => {
            let source = source_call(factory, &partial, &content_index)?;
            if let Some(p) = projector {
                p.native_delta(content_index, delta, source)?;
            }
        }
        Event::ToolcallEnd { content_index, tool_call, .. } => {
            if let Some(p) = projector {
                p.native_end(content_index, tool_call)?;
            }
        }
        Event::Done { message, .. } => {
            if projector.is_none() {
                *projector = Some(Projector::new(factory.clone(), globals, tools, dialect, &message, true)?);
            }
            projector.as_mut().expect("done creates projector").finish(&message, true)?;
            terminal = true;
        }
        Event::Error { reason, error } => {
            direct.push(Event::Error { reason, error });
            terminal = true;
        }
        Event::TextStart { .. } | Event::TextEnd { .. } => {}
    }
    let mut events = projector.as_mut().map(|p| std::mem::take(&mut p.events)).unwrap_or_default();
    events.extend(direct);
    Ok((events, abort, terminal))
}

/// Wrap the actual provider stream only after the Core's captured-tools gate.
/// Child abortion stops fabricated continuation without cancelling the Run.
/// Event closure never seals or erases the inherited provider's actual owner.
pub fn wrap_owned_stream(
    mut inner: ActualAssistantStream,
    factory: Arc<GraphValueFactory>,
    globals: ValueHandle,
    tools: ValueHandle,
    dialect: Dialect,
    provider_abort: CancellationToken,
    outer_cancel: CancellationToken,
    abort_on_fabrication: bool,
) -> Result<ActualAssistantStream> {
    if !matches!(dialect, Dialect::Deepseek | Dialect::Hermes | Dialect::Qwen3) {
        return Err(ValueError::UnsupportedOperation("Owned Graph stream for this fixed dialect is not connected"));
    }
    factory.heap().adopt(&tools)?;
    factory.heap().adopt(&globals)?;
    let (sink, stream, failures) = ActualAssistantStream::channel_with_close_drain();
    failures.inherit_stream(&inner);
    let guard = failures.owner_settlement_guard();
    tokio::spawn(async move {
        let _guard = guard;
        let mut projector = None;
        loop {
            let event = tokio::select! {
                biased;
                _ = sink.closed() => None,
                _ = outer_cancel.cancelled() => None,
                event = inner.recv() => event,
            };
            let Some(event) = event else {
                provider_abort.cancel();
                inner.close();
                // Ready events retain selected terminals without awaiting any
                // inherited producer. Late failures remain in its live receipt.
                while let Ok(event) = inner.try_recv() {
                    match project_event(
                        &mut projector,
                        &factory,
                        &globals,
                        &tools,
                        dialect,
                        event,
                        abort_on_fabrication,
                    ) {
                        Ok((events, _, terminal)) => {
                            for event in events {
                                failures.retain_terminal(&event);
                            }
                            if terminal {
                                break;
                            }
                        }
                        Err(error) => {
                            failures.record(error);
                            break;
                        }
                    }
                }
                return;
            };
            let (events, abort, terminal) =
                match project_event(&mut projector, &factory, &globals, &tools, dialect, event, abort_on_fabrication) {
                    Ok(result) => result,
                    Err(error) => {
                        failures.record(error);
                        provider_abort.cancel();
                        inner.close();
                        return;
                    }
                };
            // Keep the actual selected terminal even if cancellation or a full
            // event queue prevents its public delivery.
            for event in &events {
                failures.retain_terminal(event);
            }
            // Source pushes to an unbounded event owner before aborting. Our
            // bounded consumer must not delay the provider abort on queue IO.
            if abort {
                provider_abort.cancel();
                inner.close();
            }
            for event in events {
                if !sink.push_or_cancel(event, &outer_cancel).await {
                    provider_abort.cancel();
                    inner.close();
                    while let Ok(event) = inner.try_recv() {
                        match project_event(
                            &mut projector,
                            &factory,
                            &globals,
                            &tools,
                            dialect,
                            event,
                            abort_on_fabrication,
                        ) {
                            Ok((events, _, terminal)) => {
                                for event in events {
                                    failures.retain_terminal(&event);
                                }
                                if terminal {
                                    break;
                                }
                            }
                            Err(error) => {
                                failures.record(error);
                                break;
                            }
                        }
                    }
                    return;
                }
            }
            if terminal {
                return;
            }
        }
    });
    Ok(stream)
}

/// Nonstream source entry uses the identical projector without emitting events.
pub fn parse_owned_message(
    factory: Arc<GraphValueFactory>,
    globals: &ValueHandle,
    message: &ValueHandle,
    tools: &ValueHandle,
    dialect: Dialect,
) -> Result<ValueHandle> {
    if !matches!(dialect, Dialect::Deepseek | Dialect::Hermes | Dialect::Qwen3) {
        return Err(ValueError::UnsupportedOperation("Owned Graph message for this fixed dialect is not connected"));
    }
    let mut projector = Projector::new(factory.clone(), globals, tools, dialect, message, false)?;
    let ex = execution(&factory);
    ex.for_of(&ex.get(message, "content")?, |block| {
        if ex.eq(block, "type", "text")? {
            projector.text(ex.string(&ex.get(block, "text")?)?)?;
        } else {
            projector.keep(block.clone())?;
        }
        Ok(true)
    })?;
    projector.finish(message, false)
}

#[cfg(test)]
mod observations;
#[cfg(test)]
pub(super) fn observe_owned_stream_family() {
    observations::observe_owned_stream_family();
}
