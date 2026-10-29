//! Draft additions to an existing coarse test entrance; no added test attribute.
//! Exercises the actual Rust compiled Function owner. Root supplies its real
//! same-realm globals and imported utility; this helper is not that bootstrap.

use ara_ai::{graph_values::GraphValueFactory, native_error::NativeErrorFunctionOwner};
use ara_rpc::{
    WireString,
    value_heap::{
        CallableTarget, CompiledFunctionForm, CompiledFunctionSpec, PropertyDescriptor, ValueError, ValueHandle,
        ValueHeap, ValueRuntime, ValueSnapshot,
    },
};
use std::sync::Arc;

pub struct NativeErrorObservation {
    pub family: &'static str,
    pub message: ValueHandle,
    pub terminal: ValueHandle,
}
struct Case {
    name: &'static str,
    text: &'static str,
    status: Option<f64>,
    api: &'static str,
    provider: &'static str,
    model: &'static str,
    required: u32,
    forbidden: u32,
}
const O: u32 = 0x0080_0000;
const P: u32 = 0x8000_0000;
const T: u32 = 0x0002_0000;
const U: u32 = 0x0008_0000;
const A: u32 = 0x0000_4000;
const B: u32 = 0x0000_8000;
const CASES: &[Case] = &[
    Case {
        name: "overflow",
        text: "prompt is too long",
        status: Some(400.0),
        api: "openai-completions",
        provider: "custom",
        model: "x",
        required: O,
        forbidden: P,
    },
    Case {
        name: "payload",
        text: "request_too_large",
        status: Some(413.0),
        api: "openai-completions",
        provider: "custom",
        model: "x",
        required: P,
        forbidden: O,
    },
    Case {
        name: "bare-413-dual",
        text: "413 (no body)",
        status: Some(413.0),
        api: "openai-completions",
        provider: "custom",
        model: "x",
        required: P | O,
        forbidden: 0,
    },
    Case {
        name: "malformed-tool",
        text: "malformed function call",
        status: Some(500.0),
        api: "openai-completions",
        provider: "custom",
        model: "x",
        required: 0x0020_0000 | T,
        forbidden: 0,
    },
    Case {
        name: "provider-finish",
        text: "Provider returned error finish_reason",
        status: None,
        api: "openai-completions",
        provider: "custom",
        model: "x",
        required: 0x0040_0000,
        forbidden: 0,
    },
    Case {
        name: "empty",
        text: "thought-only response without final output",
        status: None,
        api: "openai-completions",
        provider: "custom",
        model: "x",
        required: 0x2000 | T,
        forbidden: 0,
    },
    Case {
        name: "content-filter",
        text: "incomplete: content_filter",
        status: None,
        api: "openai-completions",
        provider: "custom",
        model: "x",
        required: B,
        forbidden: 0,
    },
    Case {
        name: "cyber-policy",
        text: "trusted access for cyber",
        status: None,
        api: "openai-completions",
        provider: "custom",
        model: "x",
        required: A | B,
        forbidden: 0,
    },
    Case {
        name: "codex-policy-identity",
        text: "The 'GPT-5' model is not supported when using Codex with a ChatGPT account.",
        status: None,
        api: "openai-codex-responses",
        provider: "openai-codex",
        model: "org/gpt-5",
        required: A | B,
        forbidden: 0,
    },
    Case {
        name: "codex-policy-other-model",
        text: "The 'GPT-5' model is not supported when using Codex with a ChatGPT account.",
        status: None,
        api: "openai-codex-responses",
        provider: "openai-codex",
        model: "gpt-4",
        required: 0,
        forbidden: A,
    },
    Case {
        name: "cursor-policy",
        text: "ERROR_RATE_LIMITED_CHANGEABLE: Named models unavailable",
        status: None,
        api: "openai-completions",
        provider: "cursor",
        model: "x",
        required: A | B,
        forbidden: 0,
    },
    Case {
        name: "auth",
        text: "unauthorized",
        status: Some(401.0),
        api: "openai-completions",
        provider: "custom",
        model: "x",
        required: 0x0100_0000,
        forbidden: 0,
    },
    Case {
        name: "quota",
        text: "quota exceeded",
        status: Some(429.0),
        api: "openai-completions",
        provider: "custom",
        model: "x",
        required: U,
        forbidden: 0,
    },
    Case {
        name: "timeout",
        text: "operation timed out",
        status: None,
        api: "openai-completions",
        provider: "custom",
        model: "x",
        required: T | 0x0004_0000,
        forbidden: 0,
    },
    Case {
        name: "socket",
        text: "The socket connection was closed unexpectedly",
        status: None,
        api: "openai-completions",
        provider: "custom",
        model: "x",
        required: T,
        forbidden: 0,
    },
    Case {
        name: "stream-envelope",
        text: "anthropic stream envelope error: before message_stop",
        status: None,
        api: "openai-completions",
        provider: "custom",
        model: "x",
        required: T,
        forbidden: 0,
    },
    Case {
        name: "concurrency-not-quota",
        text: "Online prediction concurrent requests quota exceeded",
        status: Some(429.0),
        api: "openai-completions",
        provider: "custom",
        model: "x",
        required: T,
        forbidden: U,
    },
    Case {
        name: "responses-stale",
        text: "Item with id 'abc' not found.",
        status: Some(400.0),
        api: "openai-responses",
        provider: "custom",
        model: "x",
        required: 0x0010_0000,
        forbidden: 0,
    },
    Case {
        name: "generation-nan",
        text: "Floating point NaN is detected in generation",
        status: Some(400.0),
        api: "openai-completions",
        provider: "custom",
        model: "x",
        required: T,
        forbidden: 0,
    },
    Case {
        name: "grammar",
        text: "invalid_request_error: compiled grammar too large",
        status: Some(400.0),
        api: "openai-completions",
        provider: "custom",
        model: "x",
        required: 0x1000_0000,
        forbidden: 0,
    },
    Case {
        name: "schema-complex",
        text: "invalid_request_error: schema compilation too complex",
        status: Some(400.0),
        api: "openai-completions",
        provider: "custom",
        model: "x",
        required: 0x1000_0000,
        forbidden: 0,
    },
    Case {
        name: "strict-field",
        text: "tools.0.custom.strict: extra inputs are not permitted",
        status: Some(400.0),
        api: "openai-completions",
        provider: "custom",
        model: "x",
        required: 0x1000_0000,
        forbidden: 0,
    },
    Case {
        name: "unsupported-structured",
        text: "structured_outputs not supported",
        status: Some(400.0),
        api: "openai-completions",
        provider: "custom",
        model: "x",
        required: 0x1000_0000,
        forbidden: 0,
    },
    Case {
        name: "fast-mode",
        text: "invalid_request_error: speed not supported",
        status: Some(400.0),
        api: "openai-completions",
        provider: "custom",
        model: "x",
        required: 0x2000_0000,
        forbidden: 0,
    },
    Case {
        name: "fast-entitlement",
        text: "rate_limit_error: fast mode unavailable",
        status: Some(429.0),
        api: "openai-completions",
        provider: "custom",
        model: "x",
        required: 0x2000_0000,
        forbidden: 0,
    },
    Case {
        name: "cn-quota",
        text: "429 已达到 5 小时的使用上限。限额将在明天重置",
        status: Some(429.0),
        api: "openai-completions",
        provider: "custom",
        model: "x",
        required: U,
        forbidden: 0,
    },
    Case {
        name: "cn-minute-throttle",
        text: "每分钟请求使用频率已达上限，请稍后重试",
        status: Some(429.0),
        api: "openai-completions",
        provider: "custom",
        model: "x",
        required: 0,
        forbidden: U,
    },
    Case {
        name: "dashscope-tpm",
        text: "You exceeded your current quota, please check your plan and billing details. https://help.aliyun.com/error-code#token-limit",
        status: Some(429.0),
        api: "openai-completions",
        provider: "custom",
        model: "x",
        required: 0,
        forbidden: U,
    },
    Case {
        name: "same-wording-account-quota",
        text: "You exceeded your current quota, please check your plan and billing details.",
        status: Some(429.0),
        api: "openai-completions",
        provider: "custom",
        model: "x",
        required: U,
        forbidden: 0,
    },
    Case {
        name: "account-403",
        text: "Reached overall message rate limit",
        status: Some(403.0),
        api: "openai-completions",
        provider: "custom",
        model: "x",
        required: U,
        forbidden: 0,
    },
    Case {
        name: "402-opaque",
        text: "HTTP 402 (no body)",
        status: Some(402.0),
        api: "openai-completions",
        provider: "custom",
        model: "x",
        required: U,
        forbidden: 0,
    },
    Case {
        name: "subscription-window",
        text: "subscription rate limit reached",
        status: Some(429.0),
        api: "openai-completions",
        provider: "custom",
        model: "x",
        required: U,
        forbidden: 0,
    },
    Case {
        name: "subscription-per-minute",
        text: "subscription rate limit per minute",
        status: Some(429.0),
        api: "openai-completions",
        provider: "custom",
        model: "x",
        required: 0,
        forbidden: U,
    },
    Case {
        name: "structured-google-quota",
        text: r#"HTTP 429 {"error":{"status":"RESOURCE_EXHAUSTED","details":[{"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"QUOTA_EXHAUSTED"}]}}"#,
        status: Some(429.0),
        api: "openai-completions",
        provider: "custom",
        model: "x",
        required: U,
        forbidden: 0,
    },
    Case {
        name: "structured-google-credits",
        text: r#"{"error":{"status":"RESOURCE_EXHAUSTED","details":[{"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"INSUFFICIENT_G1_CREDITS_BALANCE"}]}}"#,
        status: Some(429.0),
        api: "openai-completions",
        provider: "custom",
        model: "x",
        required: U,
        forbidden: 0,
    },
];

fn put(factory: &GraphValueFactory, object: &ValueHandle, key: &str, value: ValueHandle) -> Result<(), ValueError> {
    factory.heap().define_own_data(object, WireString::from(key).into(), value, factory.runtime())
}
fn output_bits(factory: &GraphValueFactory, id: &ValueHandle) -> Result<u32, ValueError> {
    let ValueSnapshot::Number(id) = factory.heap().snapshot(id)? else {
        panic!("classification must return a Number in this coarse family")
    };
    assert!(id.is_finite());
    Ok(id.trunc().rem_euclid(4_294_967_296.0) as u32)
}
fn classify_case(
    factory: &Arc<GraphValueFactory>,
    owner: &NativeErrorFunctionOwner,
    case: &Case,
) -> Result<NativeErrorObservation, ValueError> {
    let heap = factory.heap();
    let message = heap.object()?;
    for (key, value) in
        [("errorMessage", case.text), ("api", case.api), ("provider", case.provider), ("model", case.model)]
    {
        put(factory, &message, key, heap.string(value.into())?)?;
    }
    if let Some(status) = case.status {
        put(factory, &message, "errorStatus", heap.number(status)?)?;
    }
    let terminal = owner.classify_message(factory, &message, factory.runtime())?;
    let id = output_bits(factory, &terminal)?;
    assert_eq!(id & case.required, case.required, "{}", case.name);
    assert_eq!(id & case.forbidden, 0, "{}", case.name);
    let stored = heap.get(&message, &"errorId".into(), factory.runtime())?;
    assert!(heap.object_is(&stored, &terminal)?);
    if id & P != 0 {
        assert!(matches!(heap.snapshot(&terminal)?,ValueSnapshot::Number(n) if n < 0.0));
    }
    Ok(NativeErrorObservation { family: case.name, message, terminal })
}

/// Invoke from one existing coarse entrance after actual globals are admitted.
/// Returned receipts retain the original records and exact terminal handles.
pub fn observe_native_error_classification(
    factory: &Arc<GraphValueFactory>,
    owner: &NativeErrorFunctionOwner,
) -> Result<Vec<NativeErrorObservation>, ValueError> {
    let functions = owner.functions(factory)?;
    let again = owner.functions(factory)?;
    assert!(functions.classify_message.same_identity(&again.classify_message));
    assert!(functions.is_context_overflow.same_identity(&again.is_context_overflow));
    let other_identity = Arc::new(factory.as_ref().clone());
    assert!(matches!(
        owner.functions(&other_identity),
        Err(ValueError::UnsupportedOperation("Native error binding requires the original Session factory"))
    ));
    let mut observations =
        CASES.iter().map(|case| classify_case(factory, owner, case)).collect::<Result<Vec<_>, _>>()?;
    let heap = factory.heap();
    let stale = heap.object()?;
    put(factory, &stale, "errorId", heap.number(f64::from((P | 0x1000) as i32))?)?;
    put(factory, &stale, "errorStatus", heap.number(413.0)?)?;
    put(factory, &stale, "errorMessage", heap.string("requested tokens exceed context length".into())?)?;
    let id = owner.classify_message(factory, &stale, factory.runtime())?;
    assert_eq!(output_bits(factory, &id)? & (P | O), O);
    observations.push(NativeErrorObservation { family: "413-clears-old-payload", message: stale, terminal: id });

    let llama = heap.object()?;
    put(factory, &llama, "errorStatus", heap.number(500.0)?)?;
    put(factory, &llama, "errorMessage", heap.string("HTTP 500 failed to parse tool call arguments as json".into())?)?;
    let id = owner.classify_message(factory, &llama, factory.runtime())?;
    assert!(matches!(heap.snapshot(&id)?,ValueSnapshot::Number(n) if n == 500.0));
    observations.push(NativeErrorObservation {
        family: "deterministic-parse-no-transient",
        message: llama,
        terminal: id,
    });

    let modulo = heap.object()?;
    put(factory, &modulo, "errorId", heap.number(4_294_967_296.0 + f64::from(O | 0x1000))?)?;
    let id = owner.classify_message(factory, &modulo, factory.runtime())?;
    assert_eq!(output_bits(factory, &id)?, O | 0x1000);
    observations.push(NativeErrorObservation { family: "toint32-modulo", message: modulo, terminal: id });

    let surrogate = heap.object()?;
    let mut units = vec![0xd800];
    units.extend(" token limit exceeded".encode_utf16());
    let original = heap.string(WireString::from_units(units))?;
    put(factory, &surrogate, "errorMessage", original.clone())?;
    let id = owner.classify_message(factory, &surrogate, factory.runtime())?;
    assert_ne!(output_bits(factory, &id)? & O, 0);
    assert!(heap.object_is(&original, &heap.get(&surrogate, &"errorMessage".into(), factory.runtime())?)?);
    observations.push(NativeErrorObservation { family: "unpaired-utf16", message: surrogate, terminal: id });

    let usage = heap.object()?;
    put(factory, &usage, "input", heap.string("1".into())?)?;
    put(factory, &usage, "cacheRead", heap.number(2.0)?)?;
    put(factory, &usage, "cacheWrite", heap.number(3.0)?)?;
    let message = heap.object()?;
    put(factory, &message, "usage", usage)?;
    let id = owner.is_context_overflow(factory, &message, &heap.number(100.0)?, factory.runtime())?;
    assert!(matches!(heap.snapshot(&id)?, ValueSnapshot::Boolean(true))); // "123" > 100, not 1+2+3.
    observations.push(NativeErrorObservation { family: "usage-native-string-plus", message, terminal: id });

    Ok(observations)
}

/// Root prepares this owner in a separate isolated fixture realm with the
/// original helper deliberately unavailable. Never re-evaluate prepare on a
/// production/continuation realm merely to remove an imported binding.
pub fn observe_native_error_missing_retry_hint(
    factory: &Arc<GraphValueFactory>,
    missing: &NativeErrorFunctionOwner,
) -> Result<(), ValueError> {
    let heap = factory.heap();
    let message = heap.object()?;
    put(factory,&message,"errorMessage",heap.string(r#"{"error":{"status":"RESOURCE_EXHAUSTED","details":[{"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"RATE_LIMIT_EXCEEDED"}]}}"#.into())?)?;
    assert!(matches!(
        missing.classify_message(factory, &message, factory.runtime()),
        Err(ValueError::UnsupportedOperation("Original imported extractRetryHint is not bound"))
    ));
    assert!(matches!(
        heap.snapshot(&heap.get(&message, &"errorId".into(), factory.runtime())?)?,
        ValueSnapshot::Undefined
    ));
    Ok(())
}

fn callable_target(heap: &Arc<ValueHeap>, value: &ValueHandle) -> Result<CallableTarget, ValueError> {
    match heap.snapshot(value)? {
        ValueSnapshot::Function(target) => Ok(target),
        _ => Err(ValueError::InvalidNode),
    }
}
fn observed_get(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    _: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    heap.array_push(&captures[0], std::slice::from_ref(&captures[1]), runtime)?;
    Ok(captures[2].clone())
}
static OBSERVED_GET: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Native error coarse getter observation",
    name: "observeGet",
    length: 0,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 3,
    body: observed_get,
};
fn observed_set(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    heap.array_push(&captures[0], &[heap.string("set:errorId".into())?], runtime)?;
    let value = args.first().cloned().unwrap_or(heap.undefined()?);
    heap.define_own_data(&captures[1], "last".into(), value, runtime)?;
    if !matches!(heap.snapshot(&captures[2])?, ValueSnapshot::Undefined) {
        return Err(ValueError::Thrown(captures[2].clone()));
    }
    heap.undefined()
}
static OBSERVED_SET: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Native error coarse setter observation",
    name: "observeSet",
    length: 1,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 3,
    body: observed_set,
};
fn getter(
    factory: &GraphValueFactory,
    object: &ValueHandle,
    key: &str,
    events: &ValueHandle,
    value: ValueHandle,
    set: Option<CallableTarget>,
) -> Result<(), ValueError> {
    let heap = factory.heap();
    let function = heap
        .fresh_compiled_function(&OBSERVED_GET, &[events.clone(), heap.string(format!("get:{key}").into())?, value])?;
    heap.define_own(
        object,
        WireString::from(key).into(),
        PropertyDescriptor::Accessor {
            get: Some(callable_target(heap, &function)?),
            set,
            enumerable: true,
            configurable: true,
        },
        factory.runtime(),
    )
}
fn event_names(factory: &GraphValueFactory, events: &ValueHandle) -> Result<Vec<WireString>, ValueError> {
    let heap = factory.heap();
    (0..heap.array_length(events, factory.runtime())?)
        .map(|i| {
            match heap.snapshot(&heap.get(events, &WireString::from(i.to_string()).into(), factory.runtime())?)? {
                ValueSnapshot::String(name) => Ok(name),
                _ => Err(ValueError::InvalidNode),
            }
        })
        .collect()
}

/// Getter order, repeated usage Gets, source setter and original Throw remain
/// one family in the same existing coarse entrance.
pub fn observe_native_error_access_order(
    factory: &Arc<GraphValueFactory>,
    owner: &NativeErrorFunctionOwner,
) -> Result<(), ValueError> {
    let heap = factory.heap();
    for thrown in [false, true] {
        let events = heap.array(vec![])?;
        let state = heap.object()?;
        let marker = if thrown { heap.object()? } else { heap.undefined()? };
        let setter = heap.fresh_compiled_function(&OBSERVED_SET, &[events.clone(), state.clone(), marker.clone()])?;
        let message = heap.object()?;
        for key in ["errorId", "errorStatus", "errorClassificationMessage", "errorMessage", "api", "provider", "model"]
        {
            getter(
                factory,
                &message,
                key,
                &events,
                if key == "errorStatus" { heap.number(401.0)? } else { heap.undefined()? },
                if key == "errorId" { Some(callable_target(heap, &setter)?) } else { None },
            )?;
        }
        match owner.classify_message(factory, &message, factory.runtime()) {
            Err(ValueError::Thrown(original)) if thrown => assert!(original.same_identity(&marker)),
            Ok(id) if !thrown => assert!(heap.object_is(&id, &heap.get(&state, &"last".into(), factory.runtime())?)?),
            other => panic!("Unexpected Native setter observation {other:?}"),
        }
        assert_eq!(
            event_names(factory, &events)?,
            [
                "get:errorId",
                "get:errorStatus",
                "get:errorClassificationMessage",
                "get:errorMessage",
                "get:api",
                "get:provider",
                "get:model",
                "set:errorId"
            ]
            .map(WireString::from)
        );
    }
    let events = heap.array(vec![])?;
    let usage = heap.object()?;
    for (key, n) in [("input", 1.0), ("cacheRead", 2.0), ("cacheWrite", 3.0)] {
        getter(factory, &usage, key, &events, heap.number(n)?, None)?;
    }
    let message = heap.object()?;
    getter(factory, &message, "errorId", &events, heap.undefined()?, None)?;
    getter(factory, &message, "usage", &events, usage, None)?;
    let terminal = owner.is_context_overflow(factory, &message, &heap.number(5.0)?, factory.runtime())?;
    assert!(matches!(heap.snapshot(&terminal)?, ValueSnapshot::Boolean(true)));
    assert_eq!(
        event_names(factory, &events)?,
        ["get:errorId", "get:usage", "get:input", "get:usage", "get:cacheRead", "get:usage", "get:cacheWrite"]
            .map(WireString::from)
    );
    Ok(())
}

fn probe_getter(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    receiver: &ValueHandle,
    _: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    heap.call(&captures[0], receiver, &[], runtime)
}
static PROBE_GETTER: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Native error effective runtime probe getter",
    name: "probeGet",
    length: 0,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 1,
    body: probe_getter,
};
fn probe_driver(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let getter = heap.fresh_compiled_function(&PROBE_GETTER, std::slice::from_ref(&args[0]))?;
    heap.define_own(
        &captures[1],
        "errorStatus".into(),
        PropertyDescriptor::Accessor {
            get: Some(callable_target(heap, &getter)?),
            set: None,
            enumerable: true,
            configurable: true,
        },
        runtime,
    )?;
    heap.call(&captures[0], &heap.undefined()?, std::slice::from_ref(&captures[1]), runtime)
}
static PROBE_DRIVER: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Native error effective runtime nested driver",
    name: "probe",
    length: 1,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 2,
    body: probe_driver,
};
fn retain_pattern(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    heap.define_own_data(&captures[0], "callback".into(), args[0].clone(), runtime)?;
    heap.boolean(false)
}
static RETAIN_PATTERN: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Native error original some override retains predicate",
    name: "some",
    length: 1,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 1,
    body: retain_pattern,
};

/// The operation-local callback exists only in the effective wrapper runtime.
/// Replacing that runtime with the owner's original factory runtime fails.
/// A separate some override retains the *actual persistent compiled arrow*
/// and calls it after the classification frame has returned.
pub fn observe_native_error_effective_runtime_and_retained_arrow(
    factory: &Arc<GraphValueFactory>,
    owner: &NativeErrorFunctionOwner,
) -> Result<(), ValueError> {
    let heap = factory.heap();
    let functions = owner.functions(factory)?;
    let message = heap.object()?;
    let driver = heap.fresh_compiled_function(&PROBE_DRIVER, &[functions.classify_message, message])?;
    let mut calls = 0;
    let mut callback = |_: &ValueHandle, _: &[ValueHandle], _: &dyn ValueRuntime| {
        calls += 1;
        heap.number(401.0)
    };
    let id = heap.call_with_local_callback(&driver, &heap.undefined()?, &mut callback, &[], factory.runtime())?;
    assert_eq!(calls, 1);
    assert_ne!(output_bits(factory, &id)? & 0x0100_0000, 0);

    let state = heap.object()?;
    let replacement = heap.fresh_compiled_function(&RETAIN_PATTERN, std::slice::from_ref(&state))?;
    let prototype = heap.array_prototype();
    let original =
        heap.own_descriptor(&prototype, &"some".into(), factory.runtime())?.ok_or(ValueError::InvalidNode)?;
    heap.define_own_data(&prototype, "some".into(), replacement, factory.runtime())?;
    let result = (|| {
        let message = heap.object()?;
        put(factory, &message, "errorStatus", heap.number(400.0)?)?;
        put(factory, &message, "errorMessage", heap.string("hello".into())?)?;
        owner.classify_message(factory, &message, factory.runtime())?;
        let retained = heap.get(&state, &"callback".into(), factory.runtime())?;
        let regexp = heap.regexp("hello".into(), "")?;
        let matched = heap.call(&retained, &heap.undefined()?, &[regexp], factory.runtime())?;
        assert!(matches!(heap.snapshot(&matched)?, ValueSnapshot::Boolean(true)));
        Ok(())
    })();
    let restored = heap.define_own(&prototype, "some".into(), original, factory.runtime());
    match (result, restored) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(primary), Err(close)) => {
            Err(ValueError::IteratorClose { original: Box::new(primary), close: Box::new(close) })
        }
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
    }
}
