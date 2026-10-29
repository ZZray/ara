//! Fixed OMP596f2da array callbacks. MIT; see THIRD_PARTY_NOTICES.md.
//! Each body borrows its invocation runtime. Captures are original heap edges,
//! never a factory/runtime lease; an overridden method may retain the callback.
use crate::actual_provider::GraphBuilder;
use ara_rpc::{
    WireString,
    value_heap::{
        CompiledFunctionForm, CompiledFunctionSpec, ValueError, ValueHandle, ValueHeap, ValueKind, ValueRuntime,
        ValueSnapshot,
    },
};
use std::sync::Arc;
type Result<T> = std::result::Result<T, ValueError>;

#[derive(Clone, Copy)]
pub(super) struct Exec<'a> {
    pub heap: &'a Arc<ValueHeap>,
    pub runtime: &'a dyn ValueRuntime,
}
impl<'a> Exec<'a> {
    pub fn get(self, root: &ValueHandle, key: &str) -> Result<ValueHandle> {
        self.heap.get(root, &key.into(), self.runtime)
    }
    pub fn own(self, root: &ValueHandle, key: &str, value: ValueHandle) -> Result<()> {
        self.heap.define_own_data(root, key.into(), value, self.runtime)
    }
    pub fn object(self, fields: impl IntoIterator<Item = (&'static str, ValueHandle)>) -> Result<ValueHandle> {
        let value = self.heap.object()?;
        for (key, member) in fields {
            self.own(&value, key, member)?;
        }
        Ok(value)
    }
    pub fn text(self, text: impl Into<WireString>) -> Result<ValueHandle> {
        self.heap.string(text.into())
    }
    pub fn string(self, value: &ValueHandle) -> Result<WireString> {
        match self.heap.snapshot(value)? {
            ValueSnapshot::String(text) => Ok(text),
            _ => Err(ValueError::TypeError("Fixed Responses string field".into())),
        }
    }
    pub fn eq(self, root: &ValueHandle, key: &str, expected: &str) -> Result<bool> {
        Ok(
            matches!(self.heap.snapshot(&self.get(root,key)?)?,ValueSnapshot::String(text) if text.equals_ascii(expected)),
        )
    }
    pub fn nullish(self, value: &ValueHandle) -> Result<bool> {
        Ok(matches!(self.heap.kind(value)?, ValueKind::Undefined | ValueKind::Null))
    }
    pub fn method(self, target: &ValueHandle, key: &str, args: &[ValueHandle]) -> Result<ValueHandle> {
        let method = self.get(target, key)?;
        self.heap.call(&method, target, args, self.runtime)
    }
    pub fn members(self, target: &ValueHandle) -> Result<Vec<ValueHandle>> {
        let mut iterator = self.heap.get_iterator(target, self.runtime)?;
        let mut output = Vec::new();
        loop {
            match self.heap.iterator_next(&mut iterator, self.runtime) {
                Ok(Some(value)) => output.push(value),
                Ok(None) => return Ok(output),
                Err(primary) => {
                    let _ = self.heap.iterator_close(&mut iterator, self.runtime);
                    return Err(primary);
                }
            }
        }
    }
    pub fn array(self, items: impl IntoIterator<Item = ValueHandle>) -> Result<ValueHandle> {
        self.heap.array(items.into_iter().collect())
    }
    pub fn for_of(self, source: &ValueHandle, mut body: impl FnMut(&ValueHandle) -> Result<bool>) -> Result<()> {
        let mut iterator = self.heap.get_iterator(source, self.runtime)?;
        while let Some(value) = self.heap.iterator_next(&mut iterator, self.runtime)? {
            match body(&value) {
                Ok(true) => {}
                Ok(false) => {
                    self.heap.iterator_close(&mut iterator, self.runtime)?;
                    break;
                }
                Err(primary) => {
                    return Err(match self.heap.iterator_close(&mut iterator, self.runtime) {
                        Ok(()) => primary,
                        Err(close) if primary.is_native_exception() && close.is_native_exception() => primary,
                        Err(close) => ValueError::IteratorClose { original: Box::new(primary), close: Box::new(close) },
                    });
                }
            }
        }
        Ok(())
    }
    pub fn apply(
        self,
        target: &ValueHandle,
        method: &str,
        spec: &'static CompiledFunctionSpec,
        captures: &[ValueHandle],
    ) -> Result<ValueHandle> {
        // Get precedes evaluation/publication of the source arrow argument.
        let function = self.get(target, method)?;
        let callback = self.heap.fresh_compiled_function(spec, captures)?;
        self.heap.call(&function, target, &[callback], self.runtime)
    }
    pub fn apply_module(
        self,
        target: &ValueHandle,
        method: &str,
        module: &'static str,
        binding: &'static str,
        spec: &'static CompiledFunctionSpec,
    ) -> Result<ValueHandle> {
        let function = self.get(target, method)?;
        let callback = self.heap.module_compiled_function(module, binding, spec)?;
        self.heap.call(&function, target, &[callback], self.runtime)
    }
}
pub(super) fn execution(b: &GraphBuilder) -> Exec<'_> {
    Exec { heap: b.0.heap(), runtime: b.0.runtime() }
}
pub(crate) fn apply(
    b: &GraphBuilder,
    target: &ValueHandle,
    method: &str,
    spec: &'static CompiledFunctionSpec,
    captures: &[ValueHandle],
) -> Result<ValueHandle> {
    execution(b).apply(target, method, spec, captures)
}
pub(super) fn first(heap: &Arc<ValueHeap>, args: &[ValueHandle]) -> Result<ValueHandle> {
    match args.first() {
        Some(value) => Ok(value.clone()),
        None => heap.undefined(),
    }
}
pub(super) fn strict_equal(heap: &Arc<ValueHeap>, left: &ValueHandle, right: &ValueHandle) -> Result<bool> {
    match (heap.snapshot(left)?, heap.snapshot(right)?) {
        (ValueSnapshot::Number(left), ValueSnapshot::Number(right)) => Ok(left == right),
        _ => heap.object_is(left, right),
    }
}
pub(super) fn trim_nonempty(ex: Exec<'_>, value: &ValueHandle) -> Result<bool> {
    let trimmed = ex.method(value, "trim", &[])?;
    let length = ex.get(&trimmed, "length")?;
    Ok(ex.heap.to_number(&length, ex.runtime)? > 0.0)
}
pub(super) fn trim_is_zero(ex: Exec<'_>, value: &ValueHandle) -> Result<bool> {
    let trimmed = ex.method(value, "trim", &[])?;
    Ok(matches!(ex.heap.snapshot(&ex.get(&trimmed,"length")?)?,ValueSnapshot::Number(length) if length==0.0))
}
pub(super) fn well_formed(ex: Exec<'_>, value: &ValueHandle) -> Result<ValueHandle> {
    ex.method(value, "toWellFormed", &[])
}
pub(super) fn system_prompts(ex: Exec<'_>, source: &ValueHandle, redact: bool) -> Result<ValueHandle> {
    if ex.nullish(source)? {
        return ex.array([]);
    }
    let prompts = if ex.heap.kind(source)? == ValueKind::Array {
        source.clone()
    } else if ex.heap.kind(source)? == ValueKind::String {
        ex.array([source.clone()])?
    } else {
        ex.array([])?
    };
    let normalized = ex.apply(&prompts, "map", &SYSTEM_PROMPT, &[ex.heap.boolean(redact)?])?;
    ex.apply(&normalized, "filter", &NONEMPTY_PROMPT, &[])
}
pub(super) fn parse_reasoning(ex: Exec<'_>, signature: &ValueHandle) -> Result<Option<ValueHandle>> {
    if !ex.heap.truthy(signature)? {
        return Ok(None);
    }
    // Fixed parseResponseReasoningReplayItem catches parsing AND subsequent
    // Native Has/Get failures inside its try, and returns undefined.
    let attempt: Result<Option<ValueHandle>> = (|| {
        let text = ex.heap.to_string(signature, ex.runtime)?;
        let parsed = ex.heap.parse_strict_units(&text)?;
        if !ex.heap.truthy(&parsed)? || !matches!(ex.heap.kind(&parsed)?, ValueKind::Object | ValueKind::Array) {
            return Ok(None);
        }
        if !ex.heap.has(&parsed, &"type".into(), ex.runtime)? || !ex.eq(&parsed, "type", "reasoning")? {
            return Ok(None);
        }
        if !ex.heap.has(&parsed, &"id".into(), ex.runtime)?
            || ex.heap.kind(&ex.get(&parsed, "id")?)? != ValueKind::String
        {
            return Ok(None);
        }
        Ok(Some(parsed))
    })();
    Ok(attempt.unwrap_or(None))
}
pub(super) fn image(ex: Exec<'_>, item: &ValueHandle, original: bool) -> Result<ValueHandle> {
    let supplied = ex.get(item, "detail")?;
    let detail = if ex.nullish(&supplied)? {
        ex.text("auto")?
    } else if !original
        && matches!(ex.heap.snapshot(&supplied)?,ValueSnapshot::String(value) if value.equals_ascii("original"))
    {
        ex.text("auto")?
    } else {
        supplied
    };
    let output = ex.object([("type", ex.text("input_image")?), ("detail", detail)])?;
    let file = ex.get(item, "providerFile")?;
    if !ex.nullish(&file)? && ex.eq(&file, "provider", "openai")? && ex.heap.truthy(&ex.get(&file, "id")?)? {
        ex.own(&output, "file_id", ex.get(&file, "id")?)?;
    } else {
        let supplied = ex.get(item, "url")?;
        let url = if ex.nullish(&supplied)? {
            let mime = ex.heap.to_string(&ex.get(item, "mimeType")?, ex.runtime)?;
            let data = ex.heap.to_string(&ex.get(item, "data")?, ex.runtime)?;
            ex.text(crate::actual_provider::concat(
                &crate::actual_provider::concat(
                    &crate::actual_provider::concat(&"data:".into(), &mime),
                    &";base64,".into(),
                ),
                &data,
            ))?
        } else {
            supplied
        };
        ex.own(&output, "image_url", url)?;
    }
    Ok(output)
}

macro_rules! spec {
    ($name:ident,$source:literal,$body:ident,$captures:literal) => {
        pub(crate) static $name: CompiledFunctionSpec = CompiledFunctionSpec {
            source: $source,
            name: "",
            length: 1,
            form: CompiledFunctionForm::Arrow,
            capture_count: $captures,
            body: $body,
        };
    };
}
fn text_block(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Exec { heap, runtime };
    heap.boolean(ex.eq(&first(heap, args)?, "type", "text")?)
}
fn image_block(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Exec { heap, runtime };
    heap.boolean(ex.eq(&first(heap, args)?, "type", "image")?)
}
fn tool_call(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Exec { heap, runtime };
    heap.boolean(ex.eq(&first(heap, args)?, "type", "toolCall")?)
}
fn non_reasoning(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Exec { heap, runtime };
    let item = first(heap, args)?;
    heap.boolean(ex.nullish(&item)? || !ex.eq(&item, "type", "reasoning")?)
}
fn compaction(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Exec { heap, runtime };
    let item = first(heap, args)?;
    if !heap.truthy(&item)? || !matches!(heap.kind(&item)?, ValueKind::Object | ValueKind::Array) {
        return heap.boolean(false);
    }
    heap.boolean(ex.eq(&item, "type", "compaction")? || ex.eq(&item, "type", "compaction_summary")?)
}
fn replayable(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Exec { heap, runtime };
    let block = first(heap, args)?;
    heap.boolean(
        ex.eq(&block, "type", "thinking")? && parse_reasoning(ex, &ex.get(&block, "thinkingSignature")?)?.is_some(),
    )
}
fn thinking_match(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Exec { heap, runtime };
    let block = first(heap, args)?;
    heap.boolean(
        ex.eq(&block, "type", "thinking")?
            && strict_equal(heap, &ex.get(&block, "itemId")?, &ex.get(&captures[0], "id")?)?,
    )
}
fn text_value(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    Exec { heap, runtime }.get(&first(heap, args)?, "text")
}
fn system_prompt(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Exec { heap, runtime };
    let raw = well_formed(ex, &first(heap, args)?)?;
    super::redaction::credentials(ex, &raw, heap.truthy(&captures[0])?)
}
fn nonempty_prompt(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    heap.boolean(trim_nonempty(Exec { heap, runtime }, &first(heap, args)?)?)
}
fn output_part(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Exec { heap, runtime };
    let block = first(heap, args)?;
    if ex.eq(&block, "type", "image")? {
        return image(ex, &block, heap.truthy(&captures[0])?);
    }
    let raw = well_formed(ex, &ex.get(&block, "text")?)?;
    let text = if heap.truthy(&captures[1])? {
        ex.text(super::history::escaped(&ex.string(&raw)?, true, false))?
    } else {
        raw
    };
    ex.object([("type", ex.text("input_text")?), ("text", text)])
}
fn codex_computer_block(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Exec { heap, runtime };
    let block = first(heap, args)?;
    if !ex.eq(&block, "type", "toolCall")? {
        return Ok(block);
    }
    let metadata = ex.get(&block, "providerMetadata")?;
    if ex.nullish(&metadata)? || !ex.eq(&metadata, "type", "computer")? {
        return Ok(block);
    }
    ex.own(&captures[0], "changed", heap.boolean(true)?)?;
    let call = heap.spread(&block, runtime)?;
    // The source reads providerMetadata again after spread. Its getter may
    // return another value; retaining the predicate's observation is invalid.
    let metadata = ex.get(&block, "providerMetadata")?;
    let actions = heap.structured_clone_json(&ex.get(&metadata, "actions")?, runtime)?;
    ex.own(&call, "arguments", ex.object([("actions", actions)])?)?;
    heap.delete_own(&call, &"providerMetadata".into(), runtime)?;
    Ok(call)
}
spec!(TEXT_BLOCK, "vision-guard.ts:14; openai-shared.ts:2332", text_block, 0);
spec!(IMAGE_BLOCK, "vision-guard.ts:15; openai-shared.ts:2335", image_block, 0);
spec!(TOOL_CALL, "openai-shared.ts:2162", tool_call, 0);
spec!(NON_REASONING, "openai-shared.ts:1941", non_reasoning, 0);
spec!(COMPACTION, "openai-shared.ts:1958", compaction, 0);
spec!(REPLAYABLE, "openai-shared.ts:2146", replayable, 0);
spec!(THINKING_MATCH, "openai-shared.ts:3225", thinking_match, 1);
spec!(TEXT_VALUE, "openai-shared.ts:2333", text_value, 0);
spec!(OUTPUT_PART, "openai-shared.ts:2353", output_part, 2);
spec!(SYSTEM_PROMPT, "utils.ts:32", system_prompt, 1);
spec!(NONEMPTY_PROMPT, "utils.ts:33", nonempty_prompt, 0);
spec!(CODEX_COMPUTER_BLOCK, "openai-codex-responses.ts:1321", codex_computer_block, 1);
