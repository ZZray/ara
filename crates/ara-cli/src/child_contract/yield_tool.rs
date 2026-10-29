//! Fixed OMP b91db66e tools/yield.ts, MIT; THIRD_PARTY_NOTICES.md.
//! Copyright (c) 2025 Mario Zechner; (c) 2025-2026 Can Bölük; (c) 2026 Stencil Labs, Inc.
use super::{
    ChildContractConfig, ChildContractOwner, WorkPoolYieldItem,
    result::ChildRunObserver,
    schema::{self, get, members, normalize_graph, nullish, put, string, validate},
};
use crate::native_realm::NativeRealm;
use ara_agent::{
    ActualAfterToolCallContext, ActualBeforeToolCallContext, ActualProviderError, AgentMessage,
    AgentMessageBody, AgentTool, SessionValues,
};
use ara_ai::Tool;
use ara_rpc::{
    WireString,
    value_heap::{
        CompiledFunctionForm, CompiledFunctionSpec, PropertyDescriptor, ValueError, ValueHandle,
        ValueHeap, ValueKind, ValueRuntime, ValueSnapshot,
    },
};
use serde_json::json;
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum YieldStatus {
    Success,
    Aborted,
}
#[derive(Clone)]
pub struct YieldReceipt {
    pub result: ValueHandle,
    pub details: ValueHandle,
    pub data: Option<ValueHandle>,
    pub labels: Vec<WireString>,
    pub terminal: bool,
    pub complete: bool,
    pub status: YieldStatus,
    pub use_last_turn: bool,
    pub schema_overridden: bool,
}
pub struct NativeYieldBundle {
    pub owner: Arc<ChildContractOwner>,
    pub original: ValueHandle,
    observer: Mutex<Option<Weak<ChildRunObserver>>>,
    capabilities: Mutex<Vec<Weak<dyn AgentTool>>>,
    final_yield_forced: AtomicBool,
}
pub(crate) fn initial_state(
    values: &SessionValues,
    config: &ChildContractConfig,
) -> Result<ValueHandle, ValueError> {
    let h = values.heap();
    let r = values.runtime();
    let state = h.object()?;
    let built = schema::build(config.output_schema.as_ref());
    for (key, value) in [
        (
            "schema",
            built
                .json_schema
                .as_ref()
                .map(|s| h.import_serde(s))
                .transpose()?
                .unwrap_or(h.undefined()?),
        ),
        (
            "labelSchema",
            built
                .json_schema
                .as_ref()
                .map(|s| {
                    let declaration = schema::dereference(s);
                    let imported = h.import_serde(&declaration);
                    // Dispose the materialized tree on both import outcomes,
                    // before transpose can propagate a Graph import error.
                    schema::drop_declaration(declaration);
                    imported
                })
                .transpose()?
                .unwrap_or(h.undefined()?),
        ),
        ("normalParameters", h.import_serde(&built.parameters)?),
        ("normalizeData", h.boolean(built.strict)?),
        ("schemaStrict", h.boolean(built.strict)?),
        ("schemaFailures", h.number(0.0)?),
        ("emptyFailures", h.number(0.0)?),
        ("hasSections", h.boolean(false)?),
        ("turn", h.number(0.0)?),
        ("receipts", h.new_map(&h.undefined()?, r)?),
        ("submitted", h.array(Vec::new())?),
        ("batchKey", h.string("".into())?),
        ("lastText", h.undefined()?),
        ("reportText", h.undefined()?),
    ] {
        put(h, &state, key, value, r)?;
    }
    set_items(values, &state, &config.work_pool_items)?;
    Ok(state)
}
pub(crate) fn set_items(
    values: &SessionValues,
    state: &ValueHandle,
    items: &[WorkPoolYieldItem],
) -> Result<(), ValueError> {
    let h = values.heap();
    let r = values.runtime();
    let key = items
        .iter()
        .map(|i| format!("{}:{}", i.index, i.id))
        .collect::<Vec<_>>()
        .join("\0");
    let previous = get(h, state, "batchKey", r)?;
    if !matches!(h.snapshot(&previous)?,ValueSnapshot::String(s) if s==WireString::from(key.as_str()))
    {
        put(h, state, "submitted", h.array(Vec::new())?, r)?;
        put(h, state, "batchKey", h.string(key.into())?, r)?;
    }
    put(
        h,
        state,
        "items",
        h.import_serde(
            &serde_json::to_value(items)
                .map_err(|_| ValueError::TypeError("Invalid workpool declaration".into()))?,
        )?,
        r,
    )
}
fn boolean(
    h: &Arc<ValueHeap>,
    s: &ValueHandle,
    k: &str,
    r: &dyn ValueRuntime,
) -> Result<bool, ValueError> {
    h.truthy(&get(h, s, k, r)?)
}
fn number(
    h: &Arc<ValueHeap>,
    s: &ValueHandle,
    k: &str,
    r: &dyn ValueRuntime,
) -> Result<f64, ValueError> {
    match h.snapshot(&get(h, s, k, r)?)? {
        ValueSnapshot::Number(v) => Ok(v),
        _ => Err(ValueError::TypeError("Invalid yield counter".into())),
    }
}
fn increment(
    h: &Arc<ValueHeap>,
    s: &ValueHandle,
    k: &str,
    r: &dyn ValueRuntime,
) -> Result<usize, ValueError> {
    let next = number(h, s, k, r)? + 1.0;
    put(h, s, k, h.number(next)?, r)?;
    Ok(next as usize)
}
fn reset(
    h: &Arc<ValueHeap>,
    s: &ValueHandle,
    k: &str,
    r: &dyn ValueRuntime,
) -> Result<(), ValueError> {
    put(h, s, k, h.number(0.0)?, r)
}
fn nonempty(
    h: &Arc<ValueHeap>,
    value: &ValueHandle,
    r: &dyn ValueRuntime,
) -> Result<bool, ValueError> {
    if nullish(h, value)? {
        return Ok(false);
    }
    let trim = get(h, value, "trim", r)?;
    let trimmed = h.call(&trim, value, &[], r)?;
    Ok(!string(h, &trimmed)?.units().is_empty())
}
fn text_content(
    h: &Arc<ValueHeap>,
    message: &ValueHandle,
    r: &dyn ValueRuntime,
) -> Result<WireString, ValueError> {
    let content = get(h, message, "content", r)?;
    if h.kind(&content)? != ValueKind::Array {
        return Ok("".into());
    }
    let mut out = Vec::<u16>::new();
    let mut first = true;
    for block in members(h, &content, r)? {
        let ty = get(h, &block, "type", r)?;
        if matches!(h.snapshot(&ty)?,ValueSnapshot::String(t) if t.equals_ascii("text")) {
            let text = string(h, &get(h, &block, "text", r)?)?;
            if !first {
                out.push(10)
            }
            first = false;
            out.extend_from_slice(text.units());
        }
    }
    Ok(WireString::from_units(out))
}
fn graph_message(
    values: &SessionValues,
    m: &AgentMessage,
) -> Result<ValueHandle, ActualProviderError> {
    match m.body_snapshot() {
        AgentMessageBody::Graph(v) => {
            values.check_handle(&v)?;
            Ok(v)
        }
        body => Ok(values.heap().import_serde(
            &body
                .json()
                .map_err(|e| ActualProviderError::Callback(e.to_string()))?,
        )?),
    }
}
fn report_text(
    values: &SessionValues,
    messages: &[AgentMessage],
    call_id: &WireString,
) -> Result<Option<WireString>, ActualProviderError> {
    let Some(last) = messages.last() else {
        return Ok(None);
    };
    let h = values.heap();
    let r = values.runtime();
    let last = graph_message(values, last)?;
    let stop = get(h, &last, "stopReason", r)?;
    if matches!(h.snapshot(&stop)?,ValueSnapshot::String(s) if ["error","aborted","length"].iter().any(|x|s.equals_ascii(x)))
    {
        return Ok(None);
    }
    let content = get(h, &last, "content", r)?;
    if h.kind(&content)? != ValueKind::Array {
        return Ok(None);
    }
    let calls = members(h, &content, r)?
        .into_iter()
        .filter_map(
            |b| match get(h, &b, "type", r).and_then(|x| h.snapshot(&x)) {
                Ok(ValueSnapshot::String(s)) if s.equals_ascii("toolCall") => Some(b),
                _ => None,
            },
        )
        .collect::<Vec<_>>();
    if calls.len() != 1
        || string(h, &get(h, &calls[0], "name", r)?)? != "yield".into()
        || string(h, &get(h, &calls[0], "id", r)?)? != *call_id
    {
        return Ok(None);
    }
    let text = text_content(h, &last, r)?;
    if nonempty(h, &h.string(text.clone())?, r)? {
        return Ok(Some(text));
    }
    for previous in messages[..messages.len() - 1].iter().rev() {
        let p = graph_message(values, previous)?;
        let role = string(h, &get(h, &p, "role", r)?)?;
        if role.equals_ascii("developer") {
            let c = get(h, &p, "content", r)?;
            let blocks = if h.kind(&c)? == ValueKind::String {
                vec![c]
            } else if h.kind(&c)? == ValueKind::Array {
                members(h, &c, r)?
                    .iter()
                    .map(|b| get(h, b, "text", r))
                    .collect::<Result<Vec<_>, _>>()?
            } else {
                Vec::new()
            };
            if !blocks.is_empty()
                && blocks.iter().all(|b| {
                    string(h, b).is_ok_and(|s| {
                        s.units()
                            .starts_with(&"<system-reminder>".encode_utf16().collect::<Vec<_>>())
                    })
                })
            {
                continue;
            }
        }
        if !role.equals_ascii("assistant")
            || !matches!(h.snapshot(&get(h,&p,"stopReason",r)?)?,ValueSnapshot::String(s) if s.equals_ascii("stop"))
        {
            return Ok(None);
        }
        let c = get(h, &p, "content", r)?;
        if h.kind(&c)? == ValueKind::Array {
            for b in members(h, &c, r)? {
                if matches!(h.snapshot(&get(h,&b,"type",r)?)?,ValueSnapshot::String(s) if s.equals_ascii("toolCall"))
                {
                    return Ok(None);
                }
            }
        }
        let t = text_content(h, &p, r)?;
        return Ok(nonempty(h, &h.string(t.clone())?, r)?.then_some(t));
    }
    Ok(None)
}
impl NativeYieldBundle {
    pub fn prepare(
        owner: Arc<ChildContractOwner>,
        realm: &NativeRealm,
    ) -> Result<Arc<Self>, ActualProviderError> {
        if !Arc::ptr_eq(owner.values(), &realm.values) {
            return Err(ActualProviderError::Protocol(
                "Yield owner belongs to another Native Session",
            ));
        }
        let v = owner.values();
        let h = v.heap();
        let r = v.runtime();
        let original = h.object()?;
        let promise = get(h, &realm.globals.environment, "Promise", r)?;
        let execute = h.fresh_compiled_function(
            &EXECUTE,
            &[
                owner.graph.clone(),
                promise.clone(),
                get(h, &promise, "resolve", r)?,
                get(h, &promise, "reject", r)?,
                realm.globals.errors.error.clone(),
            ],
        )?;
        for (k, value) in [
            ("name", h.string("yield".into())?),
            ("label", h.string("Submit Result".into())?),
            ("approval", h.string("read".into())?),
            ("intent", h.string("omit".into())?),
            ("lenientArgValidation", h.boolean(true)?),
            ("execute", execute),
        ] {
            put(h, &original, k, value, r)?
        }
        let built = schema::build(owner.config().output_schema.as_ref());
        let normal = super::prompt::yield_description(built.json_schema.is_some(), false)
            .map_err(|e| ActualProviderError::Callback(e.0))?;
        let pool = super::prompt::yield_description(built.json_schema.is_some(), true)
            .map_err(|e| ActualProviderError::Callback(e.0))?;
        for (k, spec, captures) in [
            ("parameters", &PARAMETERS, vec![owner.graph.clone()]),
            ("strict", &STRICT, vec![owner.graph.clone()]),
            (
                "description",
                &DESCRIPTION,
                vec![
                    owner.graph.clone(),
                    h.string(normal.into())?,
                    h.string(pool.into())?,
                ],
            ),
        ] {
            h.define_own(
                &original,
                k.into(),
                PropertyDescriptor::Accessor {
                    get: Some(
                        match h.snapshot(&h.fresh_compiled_function(spec, &captures)?)? {
                            ValueSnapshot::Function(target) => target,
                            _ => return Err(ValueError::InvalidNode.into()),
                        },
                    ),
                    set: None,
                    enumerable: true,
                    configurable: true,
                },
                r,
            )?;
        }
        Ok(Arc::new(Self {
            owner,
            original,
            observer: Mutex::new(None),
            capabilities: Mutex::new(Vec::new()),
            final_yield_forced: AtomicBool::new(false),
        }))
    }
    pub fn definition(&self) -> Result<Tool, ActualProviderError> {
        let mut built = schema::build(self.owner.config().output_schema.as_ref());
        Ok(Tool {
            name: "yield".into(),
            description: super::prompt::yield_description(
                built.json_schema.is_some(),
                !self.owner.desired_snapshot().items.is_empty(),
            )
            .map_err(|e| ActualProviderError::Callback(e.0))?,
            parameters: std::mem::take(&mut built.parameters),
            strict: Some(built.strict),
            ..Default::default()
        })
    }
    pub(crate) fn bind_capability(&self, c: Arc<dyn AgentTool>) {
        let mut a = self.capabilities.lock().unwrap_or_else(|e| e.into_inner());
        a.retain(|v| v.strong_count() > 0);
        a.push(Arc::downgrade(&c));
    }
    pub fn capability(&self) -> Option<Arc<dyn AgentTool>> {
        self.capabilities
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .last()
            .and_then(Weak::upgrade)
    }
    pub fn is_capability(&self, capability: &Arc<dyn AgentTool>) -> bool {
        self.capabilities
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter_map(Weak::upgrade)
            .any(|c| Arc::ptr_eq(&c, capability))
    }
    pub fn mark_final_yield_forced(&self, forced: bool) {
        self.final_yield_forced.store(forced, Ordering::Release);
    }
    pub fn begin_turn(&self, turn_id: u64) -> Result<Arc<ChildRunObserver>, ActualProviderError> {
        self.mark_final_yield_forced(false);
        if turn_id > 9_007_199_254_740_991 {
            return Err(ActualProviderError::Protocol(
                "Child turn id exceeds exact numeric range",
            ));
        }
        let v = self.owner.values();
        let h = v.heap();
        let r = v.runtime();
        for k in ["schemaFailures", "emptyFailures"] {
            reset(h, &self.owner.graph, k, r)?
        }
        put(h, &self.owner.graph, "hasSections", h.boolean(false)?, r)?;
        put(h, &self.owner.graph, "turn", h.number(turn_id as f64)?, r)?;
        put(
            h,
            &self.owner.graph,
            "receipts",
            h.new_map(&h.undefined()?, r)?,
            r,
        )?;
        let observer = ChildRunObserver::new(self.owner.clone(), turn_id);
        *self.observer.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::downgrade(&observer));
        Ok(observer)
    }
    pub fn before_tool_dispatch(
        &self,
        c: &ActualBeforeToolCallContext<'_>,
    ) -> Result<(), ActualProviderError> {
        if !c.tool.source.same_identity(&self.original) {
            return Ok(());
        }
        if !self.is_capability(&c.tool.capability) {
            return Err(ActualProviderError::Protocol(
                "Yield source capability association changed",
            ));
        }
        let observer = self
            .observer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .and_then(Weak::upgrade)
            .ok_or(ActualProviderError::Protocol(
                "Yield has no actual turn observer",
            ))?;
        let h = c.values.heap();
        let r = c.values.runtime();
        let id = string(h, &get(h, c.tool_call, "id", r)?)?;
        observer.admit(id.clone());
        let last = c.context.messages.iter().rev().find_map(|m| {
            graph_message(c.values, m).ok().and_then(|g| {
                match get(h, &g, "role", r).and_then(|x| h.snapshot(&x)) {
                    Ok(ValueSnapshot::String(s)) if s.equals_ascii("assistant") => Some(g),
                    _ => None,
                }
            })
        });
        let text = last.as_ref().map(|m| text_content(h, m, r)).transpose()?;
        put(
            h,
            &self.owner.graph,
            "lastText",
            text.map(|s| h.string(s))
                .transpose()?
                .unwrap_or(h.undefined()?),
            r,
        )?;
        put(
            h,
            &self.owner.graph,
            "reportText",
            report_text(c.values, c.context.messages, &id)?
                .map(|s| h.string(s))
                .transpose()?
                .unwrap_or(h.undefined()?),
            r,
        )?;
        Ok(())
    }
    pub fn after_tool_call(
        &self,
        c: &ActualAfterToolCallContext<'_>,
    ) -> Result<Option<YieldReceipt>, ActualProviderError> {
        let v = self.owner.values();
        if !std::ptr::eq(v.as_ref(), c.values) {
            return Err(ActualProviderError::Protocol(
                "Yield result belongs to another Native Session",
            ));
        }
        let h = v.heap();
        let r = v.runtime();
        let id = get(h, c.tool_call, "id", r)?;
        let receipts = get(h, &self.owner.graph, "receipts", r)?;
        let lookup = get(h, &receipts, "get", r)?;
        let original = h.call(&lookup, &receipts, &[id.clone()], r)?;
        if nullish(h, &original)? {
            return Ok(None);
        }
        // Core coerce_actual rebuilds the outer result/content, retaining the exact
        // original details handle. Correlate that source receipt, not a typed mirror.
        if !get(h, &original, "details", r)?.same_identity(&get(h, c.result, "details", r)?) {
            return Ok(None);
        }
        let observer = self
            .observer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .and_then(Weak::upgrade)
            .ok_or(ActualProviderError::Protocol(
                "Yield observer expired after execution",
            ))?;
        if c.is_error {
            return Ok(None);
        }
        let mut receipt = read_receipt(h, c.result, r)?;
        receipt.terminal |= self.final_yield_forced.load(Ordering::Acquire);
        observer.observe(string(h, &id)?, receipt.clone())?;
        if receipt.terminal {
            c.terminal.request_terminal_result();
        }
        Ok(Some(receipt))
    }
}
pub(crate) fn read_receipt(
    h: &Arc<ValueHeap>,
    result: &ValueHandle,
    r: &dyn ValueRuntime,
) -> Result<YieldReceipt, ValueError> {
    let details = get(h, result, "details", r)?;
    let status = string(h, &get(h, &details, "status", r)?)?;
    let status = if status.equals_ascii("success") {
        YieldStatus::Success
    } else if status.equals_ascii("aborted") {
        YieldStatus::Aborted
    } else {
        return Err(ValueError::TypeError("Invalid yield status".into()));
    };
    let ty = get(h, &details, "type", r)?;
    let incremental = h.kind(&ty)? == ValueKind::Array && h.array_length(&ty, r)? > 0;
    let labels = if incremental {
        members(h, &ty, r)?
            .iter()
            .map(|v| string(h, v))
            .collect::<Result<Vec<_>, _>>()?
    } else {
        Vec::new()
    };
    let complete = boolean(h, &details, "complete", r)?;
    let data = get(h, &details, "data", r)?;
    Ok(YieldReceipt {
        result: result.clone(),
        details: details.clone(),
        data: (!schema::undefined(h, &data)?).then_some(data),
        labels,
        terminal: complete || status == YieldStatus::Aborted || !incremental,
        complete,
        status,
        use_last_turn: boolean(h, &details, "useLastTurn", r)?,
        schema_overridden: boolean(h, &details, "schemaOverridden", r)?,
    })
}
static PARAMETERS: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "b91 yield.ts dynamic parameters",
    name: "get parameters",
    length: 0,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 1,
    body: |h, c, _, _, r| {
        let items = get(h, &c[0], "items", r)?;
        if h.array_length(&items, r)? == 0 {
            return get(h, &c[0], "normalParameters", r);
        }
        let params=h.import_serde(&json!({"type":"object","additionalProperties":false,"description":"submit one workpool item outcome","properties":{"key":{"description":"1-based workpool item number"},"data":{"description":"Self-contained outcome and evidence for this item"},"error":{"type":"string","description":"Failure reason for this item"}},"required":["key"]}))?;
        let mut keys = Vec::new();
        for item in members(h, &items, r)? {
            keys.push(get(h, &item, "index", r)?)
        }
        let props = get(h, &params, "properties", r)?;
        let key = get(h, &props, "key", r)?;
        put(h, &key, "enum", h.array(keys)?, r)?;
        Ok(params)
    },
};
static STRICT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "b91 yield.ts dynamic strict",
    name: "get strict",
    length: 0,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 1,
    body: |h, c, _, _, r| {
        h.boolean(
            h.array_length(&get(h, &c[0], "items", r)?, r)? == 0
                && boolean(h, &c[0], "schemaStrict", r)?,
        )
    },
};
static DESCRIPTION: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "b91 yield.ts dynamic description",
    name: "get description",
    length: 0,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 3,
    body: |h, c, _, _, r| {
        Ok(if h.array_length(&get(h, &c[0], "items", r)?, r)? > 0 {
            c[2].clone()
        } else {
            c[1].clone()
        })
    },
};
static EXECUTE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "b91 yield.ts original async execute",
    name: "execute",
    length: 2,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 5,
    body: |h, c, _, a, r| {
        let outcome = (|| {
            let args = a.get(1).cloned().unwrap_or(h.undefined()?);
            let result = execute_graph(h, &c[0], &args, r)?;
            let id = a.first().cloned().unwrap_or(h.undefined()?);
            let receipts = get(h, &c[0], "receipts", r)?;
            let set = get(h, &receipts, "set", r)?;
            h.call(&set, &receipts, &[id, result.clone()], r)?;
            Ok(result)
        })();
        match outcome {
            Ok(result) => h.call(&c[2], &c[1], &[result], r),
            Err(ValueError::Thrown(error)) => h.call(&c[3], &c[1], &[error], r),
            Err(error) => {
                let object = h.construct(&c[4], &[h.string(error.to_string().into())?], r)?;
                h.call(&c[3], &c[1], &[object], r)
            }
        }
    },
};
fn failure<T>(message: impl Into<String>) -> Result<T, ValueError> {
    Err(ValueError::TypeError(message.into().into()))
}
fn response(
    h: &Arc<ValueHeap>,
    text: impl Into<WireString>,
    details: ValueHandle,
    r: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let result = h.object()?;
    let block = h.object()?;
    put(h, &block, "type", h.string("text".into())?, r)?;
    put(h, &block, "text", h.string(text.into())?, r)?;
    put(h, &result, "content", h.array(vec![block])?, r)?;
    put(h, &result, "details", details, r)?;
    Ok(result)
}
fn abort_empty(
    h: &Arc<ValueHeap>,
    s: &ValueHandle,
    ty: &ValueHandle,
    count: usize,
    r: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    reset(h, s, "emptyFailures", r)?;
    let error = format!(
        "yield result stayed empty after {count} consecutive attempt(s); aborting child instead of retrying forever. Submit success as {{\"data\":<your output>}} or failure as {{\"error\":\"message\"}}."
    );
    let details = h.object()?;
    put(h, &details, "status", h.string("aborted".into())?, r)?;
    put(h, &details, "error", h.string(error.clone().into())?, r)?;
    put(h, &details, "type", ty.clone(), r)?;
    response(h, format!("Task aborted: {error}"), details, r)
}
pub(super) fn execute_graph(
    h: &Arc<ValueHeap>,
    s: &ValueHandle,
    args: &ValueHandle,
    r: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    if h.kind(args)? != ValueKind::Object {
        return failure("yield arguments must be an object");
    }
    let raw_data = get(h, args, "data", r)?;
    let mut data = if h.kind(&raw_data)? == ValueKind::Null {
        h.undefined()?
    } else {
        raw_data
    };
    let error = get(h, args, "error", r)?;
    let error = if nullish(h, &error)?
        || matches!(h.snapshot(&error)?,ValueSnapshot::String(s) if s.units().is_empty())
    {
        None
    } else if h.kind(&error)? == ValueKind::String {
        Some(string(h, &error)?)
    } else {
        return failure("error must be a string");
    };
    let items = members(h, &get(h, s, "items", r)?, r)?;
    let mut pool_id = None;
    let mut pool_index = None;
    let ty = if !items.is_empty() {
        let key = get(h, args, "key", r)?;
        let index = match h.snapshot(&key)? {
            ValueSnapshot::Number(n) if n.is_finite() && n.fract() == 0.0 => n,
            _ => return failure("key must be a current workpool item number"),
        };
        let mut found = None;
        for item in &items {
            if matches!(h.snapshot(&get(h,item,"index",r)?)?,ValueSnapshot::Number(n) if n==index) {
                found = Some(item.clone());
                break;
            }
        }
        let item = found.ok_or_else(|| {
            ValueError::TypeError("key must be a current workpool item number".into())
        })?;
        let id = get(h, &item, "id", r)?;
        let id_text = string(h, &id)?;
        for old in members(h, &get(h, s, "submitted", r)?, r)? {
            if string(h, &old)? == id_text {
                return failure(format!("workpool item {index} was already submitted"));
            }
        }
        if schema::undefined(h, &data)? == error.is_none() {
            return failure("workpool yield requires exactly one of data or error");
        }
        pool_id = Some(id.clone());
        pool_index = Some(index);
        h.array(vec![id])?
    } else {
        let raw = get(h, args, "type", r)?;
        if nullish(h, &raw)? {
            h.undefined()?
        } else if h.kind(&raw)? == ValueKind::String {
            raw
        } else if h.kind(&raw)? == ValueKind::Array
            && h.array_length(&raw, r)? > 0
            && members(h, &raw, r)?
                .iter()
                .all(|x| h.kind(x).is_ok_and(|k| k == ValueKind::String))
        {
            raw
        } else {
            return failure("type must be a string or non-empty array of strings");
        }
    };
    let incremental = h.kind(&ty)? == ValueKind::Array && h.array_length(&ty, r)? > 0;
    let mut use_last =
        error.is_none() && schema::undefined(h, &data)? && !schema::undefined(h, &ty)?;
    if error.is_some() && !schema::undefined(h, &data)? {
        return failure("yield cannot contain both data and error");
    }
    if error.is_none() && schema::undefined(h, &data)? && schema::undefined(h, &ty)? {
        let count = increment(h, s, "emptyFailures", r)?;
        if count > 3 {
            return abort_empty(h, s, &ty, count, r);
        }
        return failure(format!(
            "yield must contain either data or error. Empty untyped result retries remaining before abort: {}.",
            3 - count
        ));
    }
    let schema = get(h, s, "schema", r)?;
    let label_schema = get(h, s, "labelSchema", r)?;
    if error.is_none() && incremental && pool_id.is_none() && !nullish(h, &schema)? {
        for label in members(h, &ty, r)? {
            if !schema::known_label(h, &label_schema, &string(h, &label)?, r)? {
                return failure(
                    "Unknown incremental yield section label; resubmit with one of the schema's labels",
                );
            }
        }
    }
    if error.is_none()
        && use_last
        && !incremental
        && !nullish(h, &schema)?
        && !boolean(h, s, "hasSections", r)?
    {
        return failure(
            "This task requires structured output matching the declared schema; submit the full object in data",
        );
    }
    if error.is_none() && use_last && (incremental || !boolean(h, s, "hasSections", r)?) {
        let text = get(
            h,
            s,
            if !incremental {
                "reportText"
            } else {
                "lastText"
            },
            r,
        )?;
        if !nonempty(h, &text, r)? {
            let count = increment(h, s, "emptyFailures", r)?;
            if count > 3 {
                return abort_empty(h, s, &ty, count, r);
            }
            return failure(
                "yield used the last assistant turn as the result, but that turn contains no text (thinking only). Put your result in data",
            );
        }
        if !incremental {
            data = text;
            use_last = false;
        }
    }
    let labels = if incremental {
        members(h, &ty, r)?
    } else {
        Vec::new()
    };
    let normalize = |value: &ValueHandle| -> Result<ValueHandle, ValueError> {
        if pool_id.is_some() || !boolean(h, s, "normalizeData", r)? {
            return Ok(value.clone());
        }
        if labels.len() == 1 {
            let section = schema::section_schema(h, &label_schema, &string(h, &labels[0])?, r)?;
            normalize_graph(h, &section, value, r)
        } else if !incremental {
            normalize_graph(h, &label_schema, value, r)
        } else {
            Ok(value.clone())
        }
    };
    let check = |value: &ValueHandle| -> Result<Option<String>, ValueError> {
        if pool_id.is_some() {
            return Ok(None);
        }
        if incremental {
            for label in &labels {
                let part = schema::section_schema(h, &label_schema, &string(h, label)?, r)?;
                if let Some(e) = validate(h, &part, value, r)? {
                    return Ok(Some(e));
                }
            }
            Ok(None)
        } else {
            validate(h, &schema, value, r)
        }
    };
    let mut overridden = false;
    let mut failed_count = 0;
    if error.is_none() && !schema::undefined(h, &data)? {
        data = normalize(&data)?
    }
    if error.is_none() && !use_last {
        let mut mismatch = check(&data)?;
        if mismatch.is_some() && h.kind(&data)? == ValueKind::String {
            if let Ok(parsed) = h.parse_strict_units(&string(h, &data)?) {
                if h.kind(&parsed)? != ValueKind::Null {
                    let normalized = normalize(&parsed)?;
                    if check(&normalized)?.is_none() {
                        data = normalized;
                        mismatch = None;
                    }
                }
            }
        }
        if let Some(message) = mismatch {
            failed_count = increment(h, s, "schemaFailures", r)?;
            if failed_count <= 3 {
                return failure(format!(
                    "Output does not match schema: {message}. Call yield again with the corrected shape — {} retry attempt(s) remain before the schema constraint is dropped.",
                    3 - failed_count
                ));
            }
            reset(h, s, "schemaFailures", r)?;
            overridden = true;
        } else {
            reset(h, s, "schemaFailures", r)?
        }
    }
    reset(h, s, "emptyFailures", r)?;
    if error.is_none() && incremental {
        put(h, s, "hasSections", h.boolean(true)?, r)?
    }
    let mut complete = false;
    let mut remaining = Vec::new();
    if error.is_none() {
        if let Some(id) = pool_id {
            let old = get(h, s, "submitted", r)?;
            let mut submitted = members(h, &old, r)?;
            submitted.push(id);
            put(h, s, "submitted", h.array(submitted.clone())?, r)?;
            for item in &items {
                let key = string(h, &get(h, item, "id", r)?)?;
                if !submitted
                    .iter()
                    .any(|s| string(h, s).is_ok_and(|s| s == key))
                {
                    remaining.push(match h.snapshot(&get(h, item, "index", r)?)? {
                        ValueSnapshot::Number(n) => n.to_string(),
                        _ => "?".into(),
                    });
                }
            }
            complete = remaining.is_empty();
        }
    }
    let details = h.object()?;
    put(h, &details, "data", data, r)?;
    put(
        h,
        &details,
        "status",
        h.string(
            if error.is_some() {
                "aborted"
            } else {
                "success"
            }
            .into(),
        )?,
        r,
    )?;
    if let Some(error) = &error {
        put(h, &details, "error", h.string(error.clone())?, r)?
    }
    put(h, &details, "type", ty, r)?;
    if use_last {
        put(h, &details, "useLastTurn", h.boolean(true)?, r)?
    }
    if complete {
        put(h, &details, "complete", h.boolean(true)?, r)?
    }
    if overridden {
        put(h, &details, "schemaOverridden", h.boolean(true)?, r)?
    }
    let text: WireString = if let Some(error) = error {
        let mut units = "Task aborted: ".encode_utf16().collect::<Vec<_>>();
        units.extend_from_slice(error.units());
        WireString::from_units(units)
    } else if let Some(index) = pool_index {
        if complete {
            format!("Item {index} submitted. All workpool items are complete; ending this turn.")
                .into()
        } else {
            format!(
                "Item {index} submitted. Remaining item(s): {}.",
                remaining.join(", ")
            )
            .into()
        }
    } else if overridden {
        format!("Result submitted (schema validation overridden after {failed_count} failed attempt(s)).").into()
    } else {
        "Result submitted.".into()
    };
    response(h, text, details, r)
}
