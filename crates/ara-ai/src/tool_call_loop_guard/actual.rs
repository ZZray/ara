//! Original-value consumer of fixed OMP596f2da tool-call-loop-guard.ts.
//! MIT; Copyright (c) 2025 Mario Zechner; (c) 2025-2026 Can Bölük;
//! (c) 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
//! Prepare and finish invoke Graph callbacks. The shared detector commit does
//! not invoke them and can be owned by the Host's short state lock.

use super::{ARGUMENT_SUMMARY_LIMIT, RESULT_SUMMARY_LIMIT, ToolCallLoopGuard};
use crate::graph_values::GraphValueFactory;
use ara_rpc::{
    WireString,
    value_heap::{
        CompiledFunctionForm, CompiledFunctionSpec, PropertyKey, StringifyMode, ValueError, ValueHandle, ValueHeap,
        ValueKind, ValueRuntime, ValueSnapshot,
    },
};
use std::sync::Arc;

type Result<T> = std::result::Result<T, ValueError>;

#[derive(Clone, Copy)]
struct Exec<'a> {
    heap: &'a Arc<ValueHeap>,
    runtime: &'a dyn ValueRuntime,
}

impl<'a> Exec<'a> {
    fn factory(factory: &'a GraphValueFactory) -> Self {
        Self { heap: factory.heap(), runtime: factory.runtime() }
    }
    fn get(self, value: &ValueHandle, key: &str) -> Result<ValueHandle> {
        self.heap.get(value, &key.into(), self.runtime)
    }
    fn own(self, value: &ValueHandle, key: &str, member: ValueHandle) -> Result<()> {
        self.heap.define_own_data(value, key.into(), member, self.runtime)
    }
    fn method(self, value: &ValueHandle, key: &str, args: &[ValueHandle]) -> Result<ValueHandle> {
        let method = self.get(value, key)?;
        self.heap.call(&method, value, args, self.runtime)
    }
    fn apply(
        self,
        value: &ValueHandle,
        key: &str,
        spec: &'static CompiledFunctionSpec,
        captures: &[ValueHandle],
    ) -> Result<ValueHandle> {
        // Source Get precedes allocation/evaluation of the arrow argument.
        let method = self.get(value, key)?;
        let callback = self.heap.fresh_compiled_function(spec, captures)?;
        self.heap.call(&method, value, &[callback], self.runtime)
    }
    fn json(self, value: &ValueHandle) -> Result<Option<WireString>> {
        self.heap.stringify(value, StringifyMode::Plain, None, self.runtime)
    }
    fn json_value(self, value: &ValueHandle) -> Result<ValueHandle> {
        match self.json(value)? {
            Some(text) => self.heap.string(text),
            None => self.heap.undefined(),
        }
    }
    fn nullish(self, value: &ValueHandle) -> Result<bool> {
        Ok(matches!(self.heap.kind(value)?, ValueKind::Undefined | ValueKind::Null))
    }
    fn eq_text(self, value: &ValueHandle, text: &str) -> Result<bool> {
        Ok(matches!(self.heap.snapshot(value)?, ValueSnapshot::String(value) if value.equals_ascii(text)))
    }
    fn strict_equal(self, left: &ValueHandle, right: &ValueHandle) -> Result<bool> {
        match (self.heap.snapshot(left)?, self.heap.snapshot(right)?) {
            (ValueSnapshot::Number(left), ValueSnapshot::Number(right)) => Ok(left == right),
            _ => self.heap.object_is(left, right),
        }
    }
    fn key(self, value: &ValueHandle) -> Result<PropertyKey> {
        let primitive = self.heap.to_primitive(value, true, self.runtime)?;
        match self.heap.snapshot(&primitive)? {
            ValueSnapshot::Symbol { key, .. } => Ok(PropertyKey::Symbol(key)),
            _ => Ok(PropertyKey::String(self.heap.to_string(&primitive, self.runtime)?)),
        }
    }
    fn implicit_string(self, value: &ValueHandle) -> Result<WireString> {
        if self.heap.kind(value)? == ValueKind::Symbol {
            return Err(ValueError::TypeError("Cannot implicitly convert Symbol to string".into()));
        }
        self.heap.to_string(value, self.runtime)
    }
    fn for_of(self, value: &ValueHandle, mut body: impl FnMut(&ValueHandle) -> Result<()>) -> Result<()> {
        let mut iterator = self.heap.get_iterator(value, self.runtime)?;
        while let Some(member) = self.heap.iterator_next(&mut iterator, self.runtime)? {
            if let Err(primary) = body(&member) {
                return Err(match self.heap.iterator_close(&mut iterator, self.runtime) {
                    Ok(()) => primary,
                    Err(close) if primary.is_native_exception() && close.is_native_exception() => primary,
                    Err(close) => ValueError::IteratorClose { original: Box::new(primary), close: Box::new(close) },
                });
            }
        }
        Ok(())
    }
}

/// Created once per same-realm/settings guard, outside the Host state mutex.
/// The ordered source array still invokes Set.add for duplicate members.
pub fn create_exempt_set(factory: &GraphValueFactory, exempt_tools: &[String]) -> Result<ValueHandle> {
    let heap = factory.heap();
    let mut members = Vec::new();
    for name in exempt_tools {
        members.push(heap.string(name.as_str().into())?);
    }
    let source = heap.array(members)?;
    heap.new_set(&source, factory.runtime())
}

enum PreparedKind {
    Reset,
    Batch { hash: Option<WireString>, calls: ValueHandle, exempt_set: ValueHandle },
}

/// Roots remain original Graph edges. A method may have retained any arrow;
/// arrow captures never own a factory, resolver lease, or Host state mutex.
pub struct PreparedActualTurn {
    turn: ValueHandle,
    kind: PreparedKind,
}

/// Original recordTurn prefix through JSON.stringify(canonicalCalls), with no
/// state mutation. The Host must release its state mutex before invoking this.
pub fn prepare_turn(
    factory: &GraphValueFactory,
    turn: &ValueHandle,
    exempt_set: &ValueHandle,
) -> Result<PreparedActualTurn> {
    let ex = Exec::factory(factory);
    ex.heap.adopt(turn)?;
    ex.heap.adopt(exempt_set)?;
    let message = ex.get(turn, "message")?;
    let content = ex.get(&message, "content")?;
    let calls = ex.apply(&content, "filter", &IS_TOOL_CALL, &[])?;
    let length = ex.get(&calls, "length")?;
    if matches!(ex.heap.snapshot(&length)?, ValueSnapshot::Number(length) if length == 0.0) {
        return Ok(PreparedActualTurn { turn: turn.clone(), kind: PreparedKind::Reset });
    }
    let all_exempt = ex.apply(&calls, "every", &IS_EXEMPT, &[exempt_set.clone()])?;
    if ex.heap.truthy(&all_exempt)? {
        return Ok(PreparedActualTurn { turn: turn.clone(), kind: PreparedKind::Reset });
    }
    let canonical = ex.apply(&calls, "map", &CANONICAL_CALL, &[])?;
    let canonical = ex.method(&canonical, "sort", &[])?;
    let hash = ex.json(&canonical)?;
    Ok(PreparedActualTurn {
        turn: turn.clone(),
        kind: PreparedKind::Batch { hash, calls, exempt_set: exempt_set.clone() },
    })
}

impl ToolCallLoopGuard {
    /// Shared typed/Native lastHash/count; no Graph Get, Call or conversion.
    /// None is upstream undefined, including JSON.stringify(undefined).
    pub fn commit_actual_turn(&mut self, prepared: &PreparedActualTurn) -> Option<f64> {
        let PreparedKind::Batch { hash, .. } = &prepared.kind else {
            self.last_hash = None;
            self.count = 0.0;
            return None;
        };
        if &self.last_hash == hash {
            self.count += 1.0;
        } else {
            self.last_hash = hash.clone();
            self.count = 1.0;
        }
        (self.count == self.threshold).then_some(self.count)
    }
}

/// Run only after the Host commits the batch. Any original Throw propagates
/// without rolling back lastHash/count, as in fixed OMP recordTurn.
pub fn finish_turn(factory: &GraphValueFactory, prepared: &PreparedActualTurn, count: f64) -> Result<ValueHandle> {
    let ex = Exec::factory(factory);
    let PreparedKind::Batch { calls, exempt_set, .. } = &prepared.kind else {
        return Err(ValueError::WrongRuntimeReply("Tool-loop finish requires a committed batch"));
    };
    let report = ex.apply(calls, "find", &IS_NOT_EXEMPT, &[exempt_set.clone()])?;
    let report = if ex.nullish(&report)? { ex.get(calls, "0")? } else { report };
    let detection = ex.heap.object()?;
    ex.own(&detection, "kind", ex.heap.string("repeated_tool_call".into())?)?;
    ex.own(&detection, "toolName", ex.get(&report, "name")?)?;
    ex.own(&detection, "count", ex.heap.number(count)?)?;
    // Property evaluation order: turn.toolResults before reportCall.id.
    let results = ex.get(&prepared.turn, "toolResults")?;
    let id = ex.get(&report, "id")?;
    ex.own(&detection, "resultSummary", summarize_result(ex, &results, &id)?)?;
    // Arguments are read and canonicalized again after the result summary.
    let arguments = ex.get(&report, "arguments")?;
    let canonical = canonicalize(ex, &arguments)?;
    let json = ex.json_value(&canonical)?;
    ex.own(&detection, "argumentsSummary", summarize_text(ex, &json, ARGUMENT_SUMMARY_LIMIT)?)?;
    Ok(detection)
}

fn canonicalize(ex: Exec<'_>, value: &ValueHandle) -> Result<ValueHandle> {
    if ex.heap.kind(value)? == ValueKind::Array {
        return ex.apply(value, "map", &CANONICAL_ITEM, &[]);
    }
    if !ex.heap.truthy(value)? || ex.heap.kind(value)? != ValueKind::Object {
        return Ok(value.clone());
    }
    let output = ex.heap.object()?;
    let mut keys = Vec::new();
    for key in ex.heap.own_enumerable_string_keys(value, ex.runtime)? {
        keys.push(ex.heap.string(key)?);
    }
    let keys = ex.heap.array(keys)?;
    let sorted = ex.method(&keys, "sort", &[])?;
    ex.for_of(&sorted, |key| {
        if ex.eq_text(key, "i")? || ex.eq_text(key, "__intent")? {
            return Ok(());
        }
        // output[key] and input[key] each perform their own ToPropertyKey.
        let output_key = ex.key(key)?;
        let input_key = ex.key(key)?;
        let member = ex.heap.get(value, &input_key, ex.runtime)?;
        let canonical = canonicalize(ex, &member)?;
        // Ordinary assignment retains inherited __proto__ setter behavior.
        ex.heap.assign(&output, output_key, canonical, ex.runtime)
    })?;
    Ok(output)
}

fn greater_than_limit(ex: Exec<'_>, value: &ValueHandle, limit: usize) -> Result<bool> {
    let primitive = ex.heap.to_primitive(value, false, ex.runtime)?;
    if let ValueSnapshot::BigInt(value) = ex.heap.snapshot(&primitive)? {
        let digits = value.as_str();
        let limit = limit.to_string();
        return Ok(!digits.starts_with('-')
            && (digits.len() > limit.len() || (digits.len() == limit.len() && digits > limit.as_str())));
    }
    Ok(ex.heap.to_number(&primitive, ex.runtime)? > limit as f64)
}

fn summarize_text(ex: Exec<'_>, text: &ValueHandle, limit: usize) -> Result<ValueHandle> {
    let replace = ex.get(text, "replace")?;
    // This is a fresh regex literal per function call, after replace's Get.
    let whitespace = ex.heap.regexp(r"\s+".into(), "g")?;
    let space = ex.heap.string(" ".into())?;
    let replaced = ex.heap.call(&replace, text, &[whitespace, space], ex.runtime)?;
    let summary = ex.method(&replaced, "trim", &[])?;
    let length = ex.get(&summary, "length")?;
    if greater_than_limit(ex, &length, limit)? {
        let sliced = ex.method(&summary, "slice", &[ex.heap.number(0.0)?, ex.heap.number(limit as f64)?])?;
        let mut units = ex.implicit_string(&sliced)?.units().to_vec();
        units.push(0x2026);
        return ex.heap.string(WireString::from_units(units));
    }
    Ok(summary)
}

fn summarize_result(ex: Exec<'_>, results: &ValueHandle, id: &ValueHandle) -> Result<ValueHandle> {
    let result = ex.apply(results, "find", &RESULT_MATCHES, &[id.clone()])?;
    if !ex.heap.truthy(&result)? {
        return ex.heap.string("".into());
    }
    let text_parts = ex.heap.array(Vec::new())?;
    let content = ex.get(&result, "content")?;
    ex.for_of(&content, |block| {
        if ex.eq_text(&ex.get(block, "type")?, "text")? {
            let push = ex.get(&text_parts, "push")?;
            let text = ex.get(block, "text")?;
            ex.heap.call(&push, &text_parts, &[text], ex.runtime)?;
        }
        Ok(())
    })?;
    let joined = ex.method(&text_parts, "join", &[ex.heap.string("\n".into())?])?;
    summarize_text(ex, &joined, RESULT_SUMMARY_LIMIT)
}

fn argument(heap: &Arc<ValueHeap>, args: &[ValueHandle]) -> Result<ValueHandle> {
    args.first().cloned().map(Ok).unwrap_or_else(|| heap.undefined())
}
fn exempt_member(ex: Exec<'_>, set: &ValueHandle, call: &ValueHandle) -> Result<ValueHandle> {
    let has = ex.get(set, "has")?;
    let name = ex.get(call, "name")?;
    ex.heap.call(&has, set, &[name], ex.runtime)
}

macro_rules! arrow {
    ($spec:ident, $body:ident, $source:literal, $captures:expr, $action:expr) => {
        static $spec: CompiledFunctionSpec = CompiledFunctionSpec {
            source: $source,
            name: "",
            length: 1,
            form: CompiledFunctionForm::Arrow,
            capture_count: $captures,
            body: $body,
        };
        fn $body(
            heap: &Arc<ValueHeap>,
            captures: &[ValueHandle],
            _this: &ValueHandle,
            args: &[ValueHandle],
            runtime: &dyn ValueRuntime,
        ) -> Result<ValueHandle> {
            ($action)(Exec { heap, runtime }, captures, argument(heap, args)?)
        }
    };
}

arrow!(
    IS_TOOL_CALL,
    is_tool_call,
    "part => part.type === \"toolCall\"",
    0,
    |ex: Exec<'_>, _: &[ValueHandle], part: ValueHandle| ex
        .heap
        .boolean(ex.eq_text(&ex.get(&part, "type")?, "toolCall")?)
);
arrow!(
    IS_EXEMPT,
    is_exempt,
    "tc => this.#exemptTools.has(tc.name)",
    1,
    |ex: Exec<'_>, captures: &[ValueHandle], call: ValueHandle| exempt_member(ex, &captures[0], &call)
);
arrow!(
    IS_NOT_EXEMPT,
    is_not_exempt,
    "tc => !this.#exemptTools.has(tc.name)",
    1,
    |ex: Exec<'_>, captures: &[ValueHandle], call: ValueHandle| {
        let member = exempt_member(ex, &captures[0], &call)?;
        ex.heap.boolean(!ex.heap.truthy(&member)?)
    }
);
arrow!(
    CANONICAL_CALL,
    canonical_call,
    "tc => JSON.stringify([tc.name, canonicalizeToolCallValue(tc.arguments)])",
    0,
    |ex: Exec<'_>, _: &[ValueHandle], call: ValueHandle| {
        let name = ex.get(&call, "name")?;
        let arguments = ex.get(&call, "arguments")?;
        let canonical = canonicalize(ex, &arguments)?;
        ex.json_value(&ex.heap.array(vec![name, canonical])?)
    }
);
arrow!(
    CANONICAL_ITEM,
    canonical_item,
    "item => canonicalizeToolCallValue(item)",
    0,
    |ex: Exec<'_>, _: &[ValueHandle], item: ValueHandle| canonicalize(ex, &item)
);
arrow!(
    RESULT_MATCHES,
    result_matches,
    "candidate => candidate.toolCallId === toolCallId",
    1,
    |ex: Exec<'_>, captures: &[ValueHandle], result: ValueHandle| {
        let id = ex.get(&result, "toolCallId")?;
        ex.heap.boolean(ex.strict_equal(&id, &captures[0])?)
    }
);
