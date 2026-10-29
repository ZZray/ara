//! Fixed OMP596f2da flags.ts dependencies of oneshot-retry (MIT).
//! Uses the already evaluated flags module and original imported class identities.
use super::*;
mod provider;

#[derive(Clone)]
pub struct NativeRetryClassifierImports {
    pub aws_credentials_error: ValueHandle,
    pub anthropic_connection_timeout_error: ValueHandle,
    pub anthropic_connection_error: ValueHandle,
    pub provider_http_error: ValueHandle,
    pub abort_error: ValueHandle,
}
pub struct NativeRetryErrorNamespace {
    factory: Weak<GraphValueFactory>,
    pub namespace: ValueHandle,
}
impl NativeErrorFunctionOwner {
    pub fn prepare_retry_namespace(
        &self,
        factory: &Arc<GraphValueFactory>,
        imports: NativeRetryClassifierImports,
        runtime: &dyn ValueRuntime,
    ) -> Result<NativeRetryErrorNamespace> {
        self.check_factory(factory)?;
        let heap = factory.heap();
        let mut captures = self.captures.to_vec();
        for imported in [
            imports.aws_credentials_error,
            imports.anthropic_connection_timeout_error,
            imports.anthropic_connection_error,
            imports.provider_http_error,
        ] {
            heap.adopt(&imported)?;
            captures.push(imported);
        }
        let namespace = heap.object()?;
        let flags = heap.object()?;
        for (name, bits) in FLAGS {
            heap.define_own_data(
                &flags,
                (*name).into(),
                heap.number(f64::from(*bits))?,
                runtime,
            )?;
        }
        for (name, function) in [
            (
                "classify",
                heap.fresh_compiled_function(&CLASSIFY, &captures)?,
            ),
            (
                "status",
                heap.fresh_compiled_function(&STATUS, &self.captures)?,
            ),
            ("is", heap.fresh_compiled_function(&IS, &[])?),
            ("retriable", heap.fresh_compiled_function(&RETRIABLE, &[])?),
            (
                "isTransientStatus",
                heap.fresh_compiled_function(&TRANSIENT_STATUS, &[])?,
            ),
            ("classifyMessage", self.functions.classify_message.clone()),
            ("Flag", flags),
            ("AbortError", imports.abort_error),
            (
                "LLAMA_CPP_TOOL_CALL_PARSE_PATTERN",
                patterns::regexp(
                    Execution {
                        heap,
                        runtime,
                        captures: &self.captures,
                    },
                    "LLAMA_CPP_TOOL_CALL_PARSE_PATTERN",
                )?,
            ),
        ] {
            heap.define_own_data(&namespace, name.into(), function, runtime)?;
        }
        let provider = provider::prepare(heap, &captures, &namespace, runtime)?;
        heap.define_own_data(&namespace, "isProviderRetryableError".into(), provider, runtime)?;
        Ok(NativeRetryErrorNamespace {
            factory: Arc::downgrade(factory),
            namespace,
        })
    }
}
impl NativeRetryErrorNamespace {
    pub fn check_factory(&self, factory: &Arc<GraphValueFactory>) -> Result<()> {
        let original = self
            .factory
            .upgrade()
            .ok_or(ValueError::UnsupportedOperation(
                "Retry error namespace expired",
            ))?;
        if !Arc::ptr_eq(&original, factory) {
            return Err(ValueError::ForeignHeap);
        }
        factory.heap().adopt(&self.namespace)?;
        Ok(())
    }
}
const FLAGS: &[(&str, u32)] = &[
    ("Class", CLASS),
    ("ThinkingLoop", 0x0001_0000),
    ("Transient", TRANSIENT),
    ("Timeout", TIMEOUT),
    ("UsageLimit", USAGE_LIMIT),
    ("StaleResponsesItem", STALE),
    ("MalformedFunctionCall", MALFORMED),
    ("ProviderFinishError", PROVIDER_FINISH),
    ("EmptyResponse", EMPTY),
    ("ContentBlocked", CONTENT_BLOCKED),
    ("AccountPolicy", ACCOUNT_POLICY),
    ("ContextOverflow", OVERFLOW),
    ("AuthFailed", AUTH_FAILED),
    ("SilentAbort", 0x0200_0000),
    ("UserInterrupt", 0x0400_0000),
    ("Abort", 0x0800_0000),
    ("Grammar", GRAMMAR),
    ("FastModeUnsupported", FAST_MODE),
    ("OAuthExpiry", 0x4000_0000),
    ("PayloadRejected", PAYLOAD),
];
fn has(ex: Execution<'_>, value: &ValueHandle, key: &str) -> Result<bool> {
    ex.heap.has(value, &key.into(), ex.runtime)
}
fn instance(ex: Execution<'_>, value: &ValueHandle, constructor: &ValueHandle) -> Result<bool> {
    ex.heap.instance_of(value, constructor, ex.runtime)
}
fn cause_tokens(ex: Execution<'_>, error: &ValueHandle) -> Result<bool> {
    let seen = ex.heap.construct(&ex.global("Set")?, &[], ex.runtime)?;
    let mut link = error.clone();
    while !ex.nullish(&link)? {
        if !ex.typeof_object(&link)? {
            if ex.heap.kind(&link)? == ValueKind::String && token_overflow(ex, &link)? {
                return Ok(true);
            }
            break;
        }
        if ex.truthy(&ex.method(&seen, "has", std::slice::from_ref(&link))?)? {
            break;
        }
        ex.method(&seen, "add", std::slice::from_ref(&link))?;
        if has(ex, &link, "message")? {
            let message = ex.get(&link, "message")?;
            if ex.heap.kind(&message)? == ValueKind::String && token_overflow(ex, &message)? {
                return Ok(true);
            }
        }
        if has(ex, &link, "cause")? {
            link = ex.get(&link, "cause")?;
            continue;
        }
        break;
    }
    Ok(false)
}
fn classify_body(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Execution {
        heap,
        runtime,
        captures,
    };
    let error = argument(heap, args, 0)?;
    let api = argument(heap, args, 1)?;
    let mut kinds = 0_u32;
    let seen = heap.construct(&ex.global("Set")?, &[], runtime)?;
    let token_evidence = cause_tokens(ex, &error)?;
    let mut link = error.clone();
    while !ex.nullish(&link)? {
        if ex.typeof_object(&link)? {
            if ex.truthy(&ex.method(&seen, "has", std::slice::from_ref(&link))?)? {
                break;
            }
            ex.method(&seen, "add", std::slice::from_ref(&link))?;
            if has(ex, &link, "errorId")?
                && heap.kind(&ex.get(&link, "errorId")?)? == ValueKind::Number
            {
                kinds |= bits(ex, &ex.get(&link, "errorId")?)? & KIND_MASK;
            }
            if has(ex, &link, "code")?
                && heap.kind(&ex.get(&link, "code")?)? == ValueKind::String
                && ex.test("ACCOUNT_POLICY_PATTERN", &ex.get(&link, "code")?)?
            {
                kinds |= ACCOUNT_POLICY | CONTENT_BLOCKED;
            }
        }
        if instance(ex, &link, &captures[7])? {
            kinds |= AUTH_FAILED;
        } else if instance(ex, &link, &captures[8])? {
            kinds |= TIMEOUT | TRANSIENT;
        } else if instance(ex, &link, &captures[9])? {
            kinds |= TRANSIENT;
        } else if ex.typeof_object(&link)?
            && has(ex, &link, "name")?
            && ex.equals_text(&ex.get(&link, "name")?, "CodexWebSocketTransportError")?
        {
            kinds |= TRANSIENT;
        } else if instance(ex, &link, &ex.global("Error")?)?
            && ex.equals_text(&ex.get(&link, "name")?, "CodexProviderStreamError")?
            && has(ex, &link, "retryable")?
            && heap.object_is(&ex.get(&link, "retryable")?, &heap.boolean(true)?)?
        {
            kinds |= TRANSIENT;
        } else if instance(ex, &link, &captures[10])? {
            let mut link_kinds = 0;
            let status = ex.get(&link, "status")?;
            let code = ex.get(&link, "code")?;
            if ex.equals_text(&code, "usage_limit_reached")?
                || (ex.equals_text(&code, "insufficient_quota")?
                    && !rate_limit::dashscope(ex, &ex.get(&link, "message")?)?)
                || (ex.equals_number(&status, 402.0)?
                    && (ex.equals_text(&code, "payment_required")?
                        || ex.equals_text(&code, "deactivated_workspace")?
                        || rate_limit::billing_402(ex, &ex.get(&link, "message")?)?))
            {
                link_kinds |= USAGE_LIMIT;
            }
            if ex.equals_text(&code, "overloaded_error")?
                || ex.equals_text(&code, "rate_limit_error")?
            {
                link_kinds |= TRANSIENT;
            }
            if (ex.equals_number(&status, 401.0)? || ex.equals_number(&status, 403.0)?)
                && !(ex.equals_number(&status, 403.0)?
                    && rate_limit::reason(ex, &ex.get(&link, "message")?)?
                        == rate_limit::Reason::Concurrent)
            {
                link_kinds |= AUTH_FAILED;
            } else if ex.equals_number(&status, 429.0)? {
                if (link_kinds & USAGE_LIMIT) == 0 {
                    link_kinds |= TRANSIENT;
                }
            } else if ex.compare_number(&status, 500.0, ">=")? {
                link_kinds |= TRANSIENT;
            }
            kinds |= link_kinds;
        }
        let text = if instance(ex, &link, &ex.global("Error")?)? {
            ex.get(&link, "message")?
        } else if heap.kind(&link)? == ValueKind::String {
            link.clone()
        } else if ex.typeof_object(&link)?
            && has(ex, &link, "message")?
            && heap.kind(&ex.get(&link, "message")?)? == ValueKind::String
        {
            ex.get(&link, "message")?
        } else {
            heap.undefined()?
        };
        let status = status_internal(ex, &link, 0)?;
        let text_id = classify_text(
            ex,
            &text,
            &status,
            token_evidence,
            &api,
            &heap.undefined()?,
            &heap.undefined()?,
        )?;
        kinds |= bits(ex, &text_id)? & KIND_MASK;
        link = if ex.typeof_object(&link)? && has(ex, &link, "cause")? {
            ex.get(&link, "cause")?
        } else {
            heap.undefined()?
        };
    }
    if kinds != 0 {
        classified(ex, kinds)
    } else {
        coalesce(ex, status_internal(ex, &error, 0)?, || heap.number(0.0))
    }
}
static CLASSIFY: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da error/flags.ts classify + hasCauseTokenContextOverflowEvidence",
    name: "classify",
    length: 1,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 11,
    body: classify_body,
};
static STATUS: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da error/flags.ts status",
    name: "status",
    length: 1,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 7,
    body: |heap, captures, _, args, runtime| {
        status_internal(
            Execution {
                heap,
                runtime,
                captures,
            },
            &argument(heap, args, 0)?,
            0,
        )
    },
};
static IS: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da error/flags.ts is",
    name: "is",
    length: 2,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 0,
    body: |heap, captures, _, args, runtime| {
        let ex = Execution {
            heap,
            runtime,
            captures,
        };
        let id = coalesce(ex, argument(heap, args, 0)?, || heap.number(0.0))?;
        heap.boolean((bits(ex, &id)? & bits(ex, &argument(heap, args, 1)?)?) != 0)
    },
};
static RETRIABLE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da error/flags.ts retriable",
    name: "retriable",
    length: 2,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 0,
    body: |heap, captures, _, args, runtime| {
        let ex = Execution {
            heap,
            runtime,
            captures,
        };
        let id = argument(heap, args, 0)?;
        if is_flag(ex, &id, CONTENT_BLOCKED)? || is_flag(ex, &id, PAYLOAD)? {
            return heap.boolean(false);
        }
        let options = argument(heap, args, 1)?;
        if ex.truthy(&ex.optional_get(&options, "replayUnsafe")?)? {
            return heap.boolean(false);
        }
        if is_flag(ex, &id, MALFORMED)? {
            return heap.boolean(true);
        }
        heap.boolean(
            (bits(ex, &id)?
                & (TRANSIENT | USAGE_LIMIT | 0x0001_0000 | STALE | PROVIDER_FINISH | EMPTY))
                != 0,
        )
    },
};
static TRANSIENT_STATUS: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da error/retryable.ts isTransientStatus",
    name: "isTransientStatus",
    length: 1,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 0,
    body: |heap, captures, _, args, runtime| {
        let ex = Execution {
            heap,
            runtime,
            captures,
        };
        let status = argument(heap, args, 0)?;
        heap.boolean(
            !ex.undefined(&status)?
                && (ex.equals_number(&status, 408.0)?
                    || ex.equals_number(&status, 429.0)?
                    || ex.compare_number(&status, 500.0, ">=")?),
        )
    },
};
