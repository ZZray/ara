//! Fixed OMP AbortError/StreamTimeoutError imports and native DOM abort reason.
//! MIT source: packages/ai/src/error/{abort,validation,flags}.ts at596f2da.
use super::signal::NativeRetryErrorImports;
use ara_ai::graph_values::GraphValueFactory;
use ara_rpc::value_heap::{
    CompiledFunctionForm, CompiledFunctionSpec, PropertyDescriptor, ValueError, ValueHandle,
    ValueKind, ValueRuntime,
};
use std::sync::Arc;
type Result<T> = std::result::Result<T, ValueError>;

pub(crate) fn prepare(
    factory: &Arc<GraphValueFactory>,
    error: ValueHandle,
) -> Result<NativeRetryErrorImports> {
    let heap = factory.heap();
    let abort_error = derived(factory, &ABORT_ERROR, &error)?;
    let stream_timeout_error = derived(factory, &STREAM_TIMEOUT_ERROR, &error)?;
    let dom_exception = derived(factory, &DOM_EXCEPTION, &error)?;
    let provider_response_error = derived(factory, &PROVIDER_RESPONSE_ERROR, &error)?;
    let aws_credentials_error = derived(factory, &AWS_CREDENTIALS_ERROR, &error)?;
    let anthropic_connection_timeout_error =
        derived(factory, &ANTHROPIC_CONNECTION_TIMEOUT_ERROR, &error)?;
    let anthropic_connection_error = derived(factory, &ANTHROPIC_CONNECTION_ERROR, &error)?;
    let provider_http_error = derived(factory, &PROVIDER_HTTP_ERROR, &error)?;
    // DOMException.name defaults to Error. Every instance receives the authored
    // name/message on construction; this default matches absent message lookup.
    let prototype = heap.get(&dom_exception, &"prototype".into(), factory.runtime())?;
    heap.define_own(
        &prototype,
        "name".into(),
        PropertyDescriptor::Data {
            value: heap.string("Error".into())?,
            writable: true,
            enumerable: false,
            configurable: true,
        },
        factory.runtime(),
    )?;
    Ok(NativeRetryErrorImports {
        error,
        abort_error,
        dom_exception,
        stream_timeout_error,
        provider_response_error,
        aws_credentials_error,
        anthropic_connection_timeout_error,
        anthropic_connection_error,
        provider_http_error,
    })
}

fn arg(
    heap: &Arc<ara_rpc::value_heap::ValueHeap>,
    args: &[ValueHandle],
    index: usize,
) -> Result<ValueHandle> {
    args.get(index)
        .cloned()
        .map(Ok)
        .unwrap_or_else(|| heap.undefined())
}
fn optional(
    heap: &Arc<ara_rpc::value_heap::ValueHeap>,
    target: &ValueHandle,
    name: &str,
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    if matches!(heap.kind(target)?, ValueKind::Null | ValueKind::Undefined) {
        heap.undefined()
    } else {
        heap.get(target, &name.into(), runtime)
    }
}
fn cause_options(
    heap: &Arc<ara_rpc::value_heap::ValueHeap>,
    options: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    if heap.kind(&optional(heap, options, "cause", runtime)?)? == ValueKind::Undefined {
        return heap.undefined();
    }
    let passed = heap.object()?;
    heap.define_own_data(
        &passed,
        "cause".into(),
        optional(heap, options, "cause", runtime)?,
        runtime,
    )?;
    Ok(passed)
}
fn named_error(
    heap: &Arc<ara_rpc::value_heap::ValueHeap>,
    imports: &[ValueHandle],
    new_target: &ValueHandle,
    message: ValueHandle,
    options: ValueHandle,
    name: &str,
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let instance =
        heap.construct_with_new_target(&imports[0], &[message, options], new_target, runtime)?;
    heap.assign(&instance, "name".into(), heap.string(name.into())?, runtime)?;
    Ok(instance)
}
static AWS_CREDENTIALS_ERROR: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da error/aws.ts AwsCredentialsError",
    name: "AwsCredentialsError",
    length: 2,
    capture_count: 1,
    form: CompiledFunctionForm::DerivedConstructor(|heap, imports, _, target, args, runtime| {
        let message = arg(heap, args, 0)?;
        let passed = cause_options(heap, &arg(heap, args, 2)?, runtime)?;
        let instance = named_error(
            heap,
            imports,
            target,
            message,
            passed,
            "AwsCredentialsError",
            runtime,
        )?;
        heap.assign(&instance, "kind".into(), arg(heap, args, 1)?, runtime)?;
        Ok(instance)
    }),
    body: |_, _, _, _, _| {
        Err(ValueError::TypeError(
            "Class constructor AwsCredentialsError cannot be invoked without new".into(),
        ))
    },
};
static ANTHROPIC_CONNECTION_TIMEOUT_ERROR: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da error/classes.ts AnthropicConnectionTimeoutError",
    name: "AnthropicConnectionTimeoutError",
    length: 0,
    capture_count: 1,
    form: CompiledFunctionForm::DerivedConstructor(|heap, imports, _, target, _, runtime| {
        let instance = heap.construct_with_new_target(
            &imports[0],
            &[heap.string("Request timed out.".into())?],
            target,
            runtime,
        )?;
        heap.assign(
            &instance,
            "name".into(),
            heap.string("AnthropicConnectionTimeoutError".into())?,
            runtime,
        )?;
        Ok(instance)
    }),
    body: |_, _, _, _, _| {
        Err(ValueError::TypeError(
            "Class constructor AnthropicConnectionTimeoutError cannot be invoked without new"
                .into(),
        ))
    },
};
static ANTHROPIC_CONNECTION_ERROR: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da error/classes.ts AnthropicConnectionError",
    name: "AnthropicConnectionError",
    length: 1,
    capture_count: 1,
    form: CompiledFunctionForm::DerivedConstructor(|heap, imports, _, target, args, runtime| {
        let options = heap.object()?;
        heap.define_own_data(&options, "cause".into(), arg(heap, args, 0)?, runtime)?;
        named_error(
            heap,
            imports,
            target,
            heap.string("Connection error.".into())?,
            options,
            "AnthropicConnectionError",
            runtime,
        )
    }),
    body: |_, _, _, _, _| {
        Err(ValueError::TypeError(
            "Class constructor AnthropicConnectionError cannot be invoked without new".into(),
        ))
    },
};
static PROVIDER_HTTP_ERROR: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da error/classes.ts ProviderHttpError",
    name: "ProviderHttpError",
    length: 2,
    capture_count: 1,
    form: CompiledFunctionForm::DerivedConstructor(|heap, imports, _, target, args, runtime| {
        let message = arg(heap, args, 0)?;
        let status = arg(heap, args, 1)?;
        let options = arg(heap, args, 2)?;
        let passed = cause_options(heap, &options, runtime)?;
        let instance = named_error(
            heap,
            imports,
            target,
            message,
            passed,
            "ProviderHttpError",
            runtime,
        )?;
        heap.assign(&instance, "status".into(), status, runtime)?;
        heap.assign(
            &instance,
            "headers".into(),
            optional(heap, &options, "headers", runtime)?,
            runtime,
        )?;
        heap.assign(
            &instance,
            "code".into(),
            optional(heap, &options, "code", runtime)?,
            runtime,
        )?;
        Ok(instance)
    }),
    body: |_, _, _, _, _| {
        Err(ValueError::TypeError(
            "Class constructor ProviderHttpError cannot be invoked without new".into(),
        ))
    },
};
fn derived(
    factory: &Arc<GraphValueFactory>,
    spec: &'static CompiledFunctionSpec,
    error: &ValueHandle,
) -> Result<ValueHandle> {
    let heap = factory.heap();
    let class = heap.fresh_compiled_function(spec, std::slice::from_ref(error))?;
    heap.set_prototype(&class, Some(error), factory.runtime())?;
    let prototype = heap.get(&class, &"prototype".into(), factory.runtime())?;
    let parent = heap.get(error, &"prototype".into(), factory.runtime())?;
    heap.set_prototype(&prototype, Some(&parent), factory.runtime())?;
    Ok(class)
}
fn source_error(
    heap: &Arc<ara_rpc::value_heap::ValueHeap>,
    imports: &[ValueHandle],
    new_target: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
    name: &str,
    default_message: &str,
    flags: u32,
) -> Result<ValueHandle> {
    let message = args.first().cloned().unwrap_or(heap.undefined()?);
    let message = if heap.kind(&message)? == ValueKind::Undefined {
        heap.string(default_message.into())?
    } else {
        message
    };
    let options = args.get(1).cloned().unwrap_or(heap.undefined()?);
    // Source options?.cause is read once for the condition and once again for
    // the literal value. Preserve accessor order and the original cause.
    let cause = if matches!(heap.kind(&options)?, ValueKind::Null | ValueKind::Undefined) {
        heap.undefined()?
    } else {
        heap.get(&options, &"cause".into(), runtime)?
    };
    let passed = if heap.kind(&cause)? == ValueKind::Undefined {
        heap.undefined()?
    } else {
        let passed = heap.object()?;
        heap.define_own_data(
            &passed,
            "cause".into(),
            heap.get(&options, &"cause".into(), runtime)?,
            runtime,
        )?;
        passed
    };
    let instance =
        heap.construct_with_new_target(&imports[0], &[message, passed], new_target, runtime)?;
    heap.assign(&instance, "name".into(), heap.string(name.into())?, runtime)?;
    // Exact attach(create(...)): Class bit, signed ECMAScript bitwise result;
    // nonenumerable, nonwritable, configurable own property.
    heap.define_own(
        &instance,
        "errorId".into(),
        PropertyDescriptor::Data {
            value: heap.number(f64::from((flags | 0x1000) as i32))?,
            writable: false,
            enumerable: false,
            configurable: true,
        },
        runtime,
    )?;
    Ok(instance)
}
static ABORT_ERROR: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da error/abort.ts AbortError + flags.ts attach/create",
    name: "AbortError",
    length: 0,
    capture_count: 1,
    form: CompiledFunctionForm::DerivedConstructor(
        |heap, imports, _, new_target, args, runtime| {
            source_error(
                heap,
                imports,
                new_target,
                args,
                runtime,
                "AbortError",
                "Request was aborted",
                0x0800_0000,
            )
        },
    ),
    body: |_, _, _, _, _| {
        Err(ValueError::TypeError(
            "Class constructor AbortError cannot be invoked without new".into(),
        ))
    },
};
static STREAM_TIMEOUT_ERROR: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da error/validation.ts StreamTimeoutError + flags.ts attach/create",
    name: "StreamTimeoutError",
    length: 0,
    capture_count: 1,
    form: CompiledFunctionForm::DerivedConstructor(
        |heap, imports, _, new_target, args, runtime| {
            source_error(
                heap,
                imports,
                new_target,
                args,
                runtime,
                "StreamTimeoutError",
                "Request timed out.",
                0x0006_0000,
            )
        },
    ),
    body: |_, _, _, _, _| {
        Err(ValueError::TypeError(
            "Class constructor StreamTimeoutError cannot be invoked without new".into(),
        ))
    },
};
static DOM_EXCEPTION: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Native DOMException(message,name) reason import for OMP596f2da AbortController.abort",
    name: "DOMException",
    length: 0,
    capture_count: 1,
    form: CompiledFunctionForm::DerivedConstructor(
        |heap, imports, _, new_target, args, runtime| {
            let message = args.first().cloned().unwrap_or(heap.string("".into())?);
            let name = args.get(1).cloned().unwrap_or(heap.string("Error".into())?);
            let message = if heap.kind(&message)? == ValueKind::Undefined {
                heap.string("".into())?
            } else {
                heap.string(heap.to_string(&message, runtime)?)?
            };
            let name = if heap.kind(&name)? == ValueKind::Undefined {
                heap.string("Error".into())?
            } else {
                heap.string(heap.to_string(&name, runtime)?)?
            };
            let instance =
                heap.construct_with_new_target(&imports[0], &[message], new_target, runtime)?;
            heap.define_own(
                &instance,
                "name".into(),
                PropertyDescriptor::Data {
                    value: name.clone(),
                    writable: false,
                    enumerable: true,
                    configurable: true,
                },
                runtime,
            )?;
            let code = match heap.snapshot(&name)? {
                ara_rpc::value_heap::ValueSnapshot::String(name)
                    if name.equals_ascii("AbortError") =>
                {
                    20.0
                }
                ara_rpc::value_heap::ValueSnapshot::String(name)
                    if name.equals_ascii("TimeoutError") =>
                {
                    23.0
                }
                _ => 0.0,
            };
            heap.define_own(
                &instance,
                "code".into(),
                PropertyDescriptor::Data {
                    value: heap.number(code)?,
                    writable: false,
                    enumerable: true,
                    configurable: true,
                },
                runtime,
            )?;
            Ok(instance)
        },
    ),
    body: |_, _, _, _, _| {
        Err(ValueError::TypeError(
            "Class constructor DOMException cannot be invoked without new".into(),
        ))
    },
};

static PROVIDER_RESPONSE_ERROR: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da error/provider.ts ProviderResponseError constructor",
    name: "ProviderResponseError",
    length: 1,
    capture_count: 1,
    form: CompiledFunctionForm::DerivedConstructor(
        |heap, imports, _, new_target, args, runtime| {
            let message = args.first().cloned().unwrap_or(heap.undefined()?);
            let options = args.get(1).cloned().unwrap_or(heap.undefined()?);
            let options = if heap.kind(&options)? == ValueKind::Undefined {
                heap.object()?
            } else {
                options
            };
            let cause = heap.get(&options, &"cause".into(), runtime)?;
            let passed = if heap.kind(&cause)? == ValueKind::Undefined {
                heap.undefined()?
            } else {
                let passed = heap.object()?;
                heap.define_own_data(
                    &passed,
                    "cause".into(),
                    heap.get(&options, &"cause".into(), runtime)?,
                    runtime,
                )?;
                passed
            };
            let instance = heap.construct_with_new_target(
                &imports[0],
                &[message, passed],
                new_target,
                runtime,
            )?;
            heap.assign(
                &instance,
                "name".into(),
                heap.string("ProviderResponseError".into())?,
                runtime,
            )?;
            heap.assign(
                &instance,
                "provider".into(),
                heap.get(&options, &"provider".into(), runtime)?,
                runtime,
            )?;
            let kind = heap.get(&options, &"kind".into(), runtime)?;
            let kind = if matches!(heap.kind(&kind)?, ValueKind::Null | ValueKind::Undefined) {
                heap.string("output".into())?
            } else {
                kind
            };
            heap.assign(&instance, "kind".into(), kind, runtime)?;
            let equals =
                |value: &ValueHandle, text: &str| heap.object_is(value, &heap.string(text.into())?);
            let flags = if equals(
                &heap.get(&instance, &"kind".into(), runtime)?,
                "content-blocked",
            )? {
                Some(0x8000)
            } else if equals(
                &heap.get(&instance, &"kind".into(), runtime)?,
                "empty-output",
            )? {
                Some(0x0002_2000)
            } else if equals(
                &heap.get(&instance, &"kind".into(), runtime)?,
                "incomplete-stream",
            )? || equals(&heap.get(&instance, &"kind".into(), runtime)?, "empty-body")?
            {
                Some(0x0002_0000)
            } else {
                None
            };
            if let Some(flags) = flags {
                heap.define_own(
                    &instance,
                    "errorId".into(),
                    PropertyDescriptor::Data {
                        value: heap.number(f64::from((flags | 0x1000) as i32))?,
                        writable: false,
                        enumerable: false,
                        configurable: true,
                    },
                    runtime,
                )?;
            }
            Ok(instance)
        },
    ),
    body: |_, _, _, _, _| {
        Err(ValueError::TypeError(
            "Class constructor ProviderResponseError cannot be invoked without new".into(),
        ))
    },
};
