//! Fixed OMP b91db66e packages/utils/src/fetch-retry.ts:3-271 (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
//! Actual header/body values and imported Number/Date/Math/string methods.
//! Ordered headers and merged body hints; original Graph operations are retained.
use crate::graph_values::{GraphExecution, GraphValueFactory};
use crate::schema_validator::graph_values_compare_with_execution;
use ara_rpc::value_heap::{
    CompiledFunctionForm, CompiledFunctionSpec, ValueError, ValueHandle, ValueHeap, ValueKind,
    ValueRuntime, ValueSnapshot,
};
use std::sync::{Arc, Weak};

type Result<T> = std::result::Result<T, ValueError>;
const MODULE: &str = "ara-ai.retry.native-retry-hint";
const PATTERNS: &[(&str, &str, &str)] = &[
    (
        "QUOTA_RESET_PATTERN",
        r"reset after (?:(\d+)h)?(?:(\d+)m)?(\d+(?:\.\d+)?)s",
        "i",
    ),
    (
        "PLEASE_RETRY_PATTERN",
        r"Please retry in ([0-9.]+)(ms|s)",
        "i",
    ),
    (
        "RETRY_DELAY_FIELD_PATTERN",
        r#""retryDelay":\s*"([0-9.]+)(ms|s)""#,
        "i",
    ),
    (
        "TRY_AGAIN_PATTERN",
        r"try again in\s+~?\s*([0-9.]+)\s*(ms|sec|s|minutes?|mins?|m|hours?|hrs?|h)\b",
        "i",
    ),
    (
        "WILL_RESET_IN_PATTERN",
        r"(?:will\s+)?resets?\s+in\s+~?\s*([0-9.]+)\s*(ms|sec|s|minutes?|mins?|m|hours?|hrs?|h|days?|d)\b",
        "i",
    ),
    (
        "WILL_RESET_AT_PATTERN",
        r"(?:will\s+)?reset at\s+([0-9]{4}-[0-9]{2}-[0-9]{2}[ T][0-9]{2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]+)?(?:Z|[+-][0-9]{2}:?[0-9]{2})?)",
        "i",
    ),
    (
        "CN_RESET_AT_PATTERN",
        r"将在\s*([0-9]{4}-[0-9]{2}-[0-9]{2}\s+[0-9]{2}:[0-9]{2}:[0-9]{2})\s*重置",
        "",
    ),
    (
        "RETRY_AFTER_MS_BODY_PATTERN",
        r"\bretry-after-ms\s*[:=]\s*([0-9]+)\b",
        "i",
    ),
    ("RESET_IN_HR_MIN_PATTERN", r"resets?\s+in\s+~?\s*(\d+(?:\.\d+)?)\s*hr\s*(\d+(?:\.\d+)?)\s*min\b", "i"),
];

pub struct NativeRetryHintModule {
    factory: Weak<GraphValueFactory>,
    pub extract_retry_hint: ValueHandle,
}
impl NativeRetryHintModule {
    pub fn prepare(factory: &Arc<GraphValueFactory>, globals: ValueHandle) -> Result<Self> {
        let heap = factory.heap();
        heap.adopt(&globals)?;
        let mut captures = vec![globals];
        for (name, pattern, flags) in PATTERNS {
            captures.push(heap.module_regexp(MODULE, name, pattern, flags)?);
        }
        let pattern = |name: &str| captures[1 + PATTERNS.iter().position(|(key, _, _)| *key == name).expect("fixed pattern")].clone();
        // RESET_AT_PATTERNS is a module constant, unlike the later array literal.
        let reset_patterns = heap.array(vec![pattern("WILL_RESET_AT_PATTERN"), pattern("CN_RESET_AT_PATTERN")])?;
        captures.push(reset_patterns);
        Ok(Self {
            factory: Arc::downgrade(factory),
            extract_retry_hint: heap.fresh_compiled_function(&EXTRACT, &captures)?,
        })
    }
    pub fn check_factory(&self, factory: &Arc<GraphValueFactory>) -> Result<()> {
        let original = self
            .factory
            .upgrade()
            .ok_or(ValueError::UnsupportedOperation(
                "Native retry-hint imports expired",
            ))?;
        if !Arc::ptr_eq(&original, factory) {
            return Err(ValueError::UnsupportedOperation(
                "Native retry-hint requires original Session factory",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct Ex<'a> {
    heap: &'a Arc<ValueHeap>,
    captures: &'a [ValueHandle],
    runtime: &'a dyn ValueRuntime,
}
impl<'a> Ex<'a> {
    fn get(self, value: &ValueHandle, name: &str) -> Result<ValueHandle> {
        self.heap.get(value, &name.into(), self.runtime)
    }
    fn global(self, name: &str) -> Result<ValueHandle> {
        self.get(&self.captures[0], name)
    }
    fn method(self, value: &ValueHandle, name: &str, args: &[ValueHandle]) -> Result<ValueHandle> {
        let method = self.get(value, name)?;
        self.heap.call(&method, value, args, self.runtime)
    }
    fn number(self, value: &ValueHandle) -> Result<ValueHandle> {
        self.heap.call(
            &self.global("Number")?,
            &self.heap.undefined()?,
            std::slice::from_ref(value),
            self.runtime,
        )
    }
    fn number_method(self, name: &str, args: &[ValueHandle]) -> Result<ValueHandle> {
        self.method(&self.global("Number")?, name, args)
    }
    fn global_call(
        self,
        object: &str,
        method: &str,
        arguments: impl FnOnce() -> Result<Vec<ValueHandle>>,
    ) -> Result<ValueHandle> {
        let receiver = self.global(object)?;
        let function = self.get(&receiver, method)?;
        self.heap
            .call(&function, &receiver, &arguments()?, self.runtime)
    }
    fn global_number(self, argument: impl FnOnce() -> Result<ValueHandle>) -> Result<ValueHandle> {
        let function = self.global("Number")?;
        self.heap.call(
            &function,
            &self.heap.undefined()?,
            &[argument()?],
            self.runtime,
        )
    }
    fn finite(self, value: &ValueHandle) -> Result<bool> {
        self.heap
            .truthy(&self.number_method("isFinite", std::slice::from_ref(value))?)
    }
    fn not_nan(self, value: &ValueHandle) -> Result<bool> {
        Ok(!self
            .heap
            .truthy(&self.number_method("isNaN", std::slice::from_ref(value))?)?)
    }
    fn nullish(self, value: &ValueHandle) -> Result<bool> {
        Ok(matches!(
            self.heap.kind(value)?,
            ValueKind::Undefined | ValueKind::Null
        ))
    }
    fn compare(self, left: &ValueHandle, right: &ValueHandle, op: &str) -> Result<bool> {
        graph_values_compare_with_execution(
            GraphExecution::new(self.heap, self.runtime),
            left,
            right,
            op,
        )
    }
    fn positive(self, value: &ValueHandle) -> Result<bool> {
        self.compare(value, &self.heap.number(0.0)?, ">")
    }
    fn multiply(self, value: &ValueHandle, factor: f64) -> Result<ValueHandle> {
        self.heap
            .number(self.heap.to_number(value, self.runtime)? * factor)
    }
    fn subtract(self, left: &ValueHandle, right: &ValueHandle) -> Result<ValueHandle> {
        // ToNumeric occurs left then right, including patched Date imports.
        let numeric = |value: &ValueHandle| -> Result<ValueHandle> {
            let value = self.heap.to_primitive(value, false, self.runtime)?;
            if self.heap.kind(&value)? == ValueKind::BigInt {
                Ok(value)
            } else {
                self.heap.number(self.heap.to_number(&value, self.runtime)?)
            }
        };
        let left = numeric(left)?;
        let right = numeric(right)?;
        match (self.heap.snapshot(&left)?, self.heap.snapshot(&right)?) {
            (ValueSnapshot::BigInt(_), ValueSnapshot::BigInt(right_value)) => {
                let text = right_value.as_str();
                let negative = if text == "0" {
                    "0".to_owned()
                } else if let Some(positive) = text.strip_prefix('-') {
                    positive.to_owned()
                } else {
                    format!("-{text}")
                };
                self.heap
                    .add(&left, &self.heap.bigint_decimal(&negative)?, self.runtime)
            }
            (ValueSnapshot::BigInt(_), _) | (_, ValueSnapshot::BigInt(_)) => Err(
                ValueError::TypeError("Cannot mix BigInt and other types".into()),
            ),
            (ValueSnapshot::Number(left), ValueSnapshot::Number(right)) => {
                self.heap.number(left - right)
            }
            _ => Err(ValueError::WrongRuntimeReply(
                "Retry hint ToNumeric did not produce numeric values",
            )),
        }
    }
    fn now(self) -> Result<ValueHandle> {
        self.method(&self.global("Date")?, "now", &[])
    }
    fn date_parse(self, value: &ValueHandle) -> Result<ValueHandle> {
        self.method(&self.global("Date")?, "parse", std::slice::from_ref(value))
    }
    fn pattern(self, name: &str) -> &'a ValueHandle {
        &self.captures[1 + PATTERNS
            .iter()
            .position(|(key, _, _)| *key == name)
            .expect("fixed pattern")]
    }
    fn exec(self, name: &str, value: &ValueHandle) -> Result<ValueHandle> {
        self.method(self.pattern(name), "exec", std::slice::from_ref(value))
    }
    fn matched(self, value: &ValueHandle) -> Result<bool> {
        if self.nullish(value)? {
            return Ok(false);
        }
        self.heap.truthy(&self.get(value, "1")?)
    }
    fn unit(self, value: &ValueHandle) -> Result<ValueHandle> {
        let unit = self.method(value, "toLowerCase", &[])?;
        for (names, ms) in [
            (&["ms"][..], 1.0),
            (&["s", "sec"][..], 1000.0),
            (&["m", "min", "mins", "minute", "minutes"][..], 60_000.0),
            (&["h", "hr", "hrs", "hour", "hours"][..], 3_600_000.0),
            (&["d", "day", "days"][..], 86_400_000.0),
        ] {
            for name in names {
                if self
                    .heap
                    .object_is(&unit, &self.heap.string((*name).into())?)?
                {
                    return self.heap.number(ms);
                }
            }
        }
        self.heap.undefined()
    }
    fn duration(self, matched: &ValueHandle) -> Result<ValueHandle> {
        let value =
            self.global_call("Number", "parseFloat", || Ok(vec![self.get(matched, "1")?]))?;
        if self.finite(&value)? && self.positive(&value)? {
            let unit = self.unit(&self.get(matched, "2")?)?;
            if self.heap.kind(&unit)? != ValueKind::Undefined {
                let number = self.heap.to_number(&value, self.runtime)?;
                return self
                    .heap
                    .number(number * self.heap.to_number(&unit, self.runtime)?);
            }
        }
        self.heap.undefined()
    }
    fn patterns(
        self,
        names: &[&str],
        mut body: impl FnMut(&ValueHandle) -> Result<Option<ValueHandle>>,
    ) -> Result<Option<ValueHandle>> {
        let array = if names.len() == 2 && names[0] == "WILL_RESET_AT_PATTERN" && names[1] == "CN_RESET_AT_PATTERN" {
            self.captures[1 + PATTERNS.len()].clone()
        } else {
            self.heap.array(names.iter().map(|name| self.pattern(name).clone()).collect())?
        };
        let mut iterator = self.heap.get_iterator(&array, self.runtime)?;
        while let Some(pattern) = self.heap.iterator_next(&mut iterator, self.runtime)? {
            match body(&pattern) {
                Ok(Some(value)) => {
                    self.heap.iterator_close(&mut iterator, self.runtime)?;
                    return Ok(Some(value));
                }
                Ok(None) => {}
                Err(error) => {
                    return Err(
                        match self.heap.iterator_close(&mut iterator, self.runtime) {
                            Ok(()) => error,
                            Err(close)
                                if error.is_native_exception() && close.is_native_exception() =>
                            {
                                error
                            }
                            Err(close) => ValueError::IteratorClose {
                                original: Box::new(error),
                                close: Box::new(close),
                            },
                        },
                    );
                }
            }
        }
        Ok(None)
    }
}

fn extract(ex: Ex<'_>, source: &ValueHandle, body: &ValueHandle, options: &ValueHandle) -> Result<ValueHandle> {
    let heap = ex.heap;
    let headers = if heap.instance_of(source, &ex.global("Headers")?, ex.runtime)? {
        source.clone()
    } else if ex.nullish(source)? {
        heap.undefined()?
    } else {
        let value = ex.get(source, "headers")?;
        if ex.nullish(&value)? {
            heap.undefined()?
        } else {
            value
        }
    };
    if heap.truthy(&headers)? {
        let header = |name: &str| ex.method(&headers, "get", &[heap.string(name.into())?]);
        let value = header("retry-after-ms")?;
        if heap.truthy(&value)? {
            let ms = ex.number(&value)?;
            if ex.finite(&ms)? && ex.compare(&ms, &heap.number(0.0)?, ">=")? {
                return Ok(ms);
            }
        }
        let value = header("retry-after")?;
        if heap.truthy(&value)? {
            let seconds = ex.number(&value)?;
            if ex.finite(&seconds)? {
                return ex.global_call("Math", "max", || {
                    Ok(vec![heap.number(0.0)?, ex.multiply(&seconds, 1000.0)?])
                });
            }
            let date = ex.date_parse(&value)?;
            if ex.not_nan(&date)? {
                return ex.global_call("Math", "max", || {
                    Ok(vec![heap.number(0.0)?, ex.subtract(&date, &ex.now()?)?])
                });
            }
        }
        let value = header("x-ratelimit-reset-ms")?;
        if heap.truthy(&value)? {
            let value = ex.number(&value)?;
            if ex.finite(&value)? && ex.positive(&value)? {
                let target = if ex.compare(&value, &heap.number(1e12)?, ">")? {
                    value.clone()
                } else if ex.compare(&value, &heap.number(1e9)?, ">")? {
                    ex.multiply(&value, 1000.0)?
                } else {
                    return Ok(value);
                };
                let delta = ex.subtract(&target, &ex.now()?)?;
                if ex.positive(&delta)? {
                    return Ok(delta);
                }
            }
        }
        let value = header("x-ratelimit-reset")?;
        if heap.truthy(&value)? {
            let seconds = ex.number_method("parseInt", &[value, heap.number(10.0)?])?;
            if ex.not_nan(&seconds)? {
                let delta = ex.subtract(&ex.multiply(&seconds, 1000.0)?, &ex.now()?)?;
                if ex.positive(&delta)? {
                    return Ok(delta);
                }
            }
        }
        let value = header("x-ratelimit-reset-after")?;
        if heap.truthy(&value)? {
            let seconds = ex.number(&value)?;
            if ex.finite(&seconds)? && ex.positive(&seconds)? {
                return ex.multiply(&seconds, 1000.0);
            }
        }
    }
    if !heap.truthy(body)? { return heap.undefined(); }
    let mut hints = BodyHints::default();
    let matched = ex.exec("QUOTA_RESET_PATTERN", body)?;
    if heap.truthy(&matched)? {
        let part = |key: &str| -> Result<ValueHandle> {
            if heap.truthy(&ex.get(&matched, key)?)? {
                ex.global_call("Number", "parseInt", || Ok(vec![ex.get(&matched, key)?, heap.number(10.0)?]))
            } else { heap.number(0.0) }
        };
        let hours = part("1")?;
        let minutes = part("2")?;
        let seconds = ex.global_call("Number", "parseFloat", || Ok(vec![ex.get(&matched, "3")?]))?;
        if ex.not_nan(&seconds)? {
            let total = heap.add(&ex.multiply(&hours, 60.0)?, &minutes, ex.runtime)?;
            let total = heap.add(&ex.multiply(&total, 60.0)?, &seconds, ex.runtime)?;
            let total = ex.multiply(&total, 1000.0)?;
            let total = if ex.positive(&total)? { total } else { heap.undefined()? };
            BodyHints::consider(ex, &mut hints.longest, &total)?;
        }
    }
    ex.patterns(&["WILL_RESET_AT_PATTERN", "CN_RESET_AT_PATTERN"], |pattern| {
        let matched = ex.method(pattern, "exec", std::slice::from_ref(body))?;
        if ex.matched(&matched)? {
            let normalized = ex.method(&ex.get(&matched, "1")?, "replace", &[heap.string(" ".into())?, heap.string("T".into())?])?;
            let offset_pattern = heap.regexp(r"(?:Z|[+-][0-9]{2}:?[0-9]{2})$".into(), "i")?;
            let has_offset = ex.method(&offset_pattern, "test", std::slice::from_ref(&normalized))?;
            // Read options even for an already-offset-bearing stamp, as source does.
            let configured = if ex.nullish(options)? { heap.undefined()? } else { ex.get(options, "naiveResetTimezoneOffset")? };
            let parsed = ex.global_call("Date", "parse", || {
                let value = if heap.truthy(&has_offset)? { normalized.clone() } else {
                    if heap.kind(&normalized)? == ValueKind::Symbol { return Err(ValueError::TypeError("Cannot convert a Symbol value to a string".into())); }
                    let mut units = heap.to_string(&normalized, ex.runtime)?.units().to_vec();
                    let offset = if ex.nullish(&configured)? { heap.string("Z".into())? } else { configured.clone() };
                    if heap.kind(&offset)? == ValueKind::Symbol { return Err(ValueError::TypeError("Cannot convert a Symbol value to a string".into())); }
                    units.extend_from_slice(heap.to_string(&offset, ex.runtime)?.units());
                    heap.string(ara_rpc::WireString::from_units(units))?
                };
                Ok(vec![value])
            })?;
            if ex.not_nan(&parsed)? && ex.compare(&parsed, &ex.now()?, ">")? {
                if heap.truthy(&has_offset)? || heap.kind(&configured)? != ValueKind::Undefined {
                    BodyHints::consider(ex, &mut hints.longest, &ex.subtract(&parsed, &ex.now()?)?)?;
                } else {
                    BodyHints::consider(ex, &mut hints.naive, &ex.subtract(&parsed, &ex.now()?)?)?;
                }
            }
        }
        Ok(None)
    })?;
    let matched = ex.exec("RESET_IN_HR_MIN_PATTERN", body)?;
    if ex.matched(&matched)? && heap.truthy(&ex.get(&matched, "2")?)? {
        let hours = ex.global_call("Number", "parseFloat", || Ok(vec![ex.get(&matched, "1")?]))?;
        let minutes = ex.global_call("Number", "parseFloat", || Ok(vec![ex.get(&matched, "2")?]))?;
        if ex.finite(&hours)? && ex.finite(&minutes)? && ex.compare(&hours, &heap.number(0.0)?, ">=")? && ex.positive(&minutes)? {
            let hours = ex.multiply(&ex.multiply(&hours, 60.0)?, 60_000.0)?;
            let minutes = ex.multiply(&minutes, 60_000.0)?;
            BodyHints::consider(ex, &mut hints.longest, &heap.add(&hours, &minutes, ex.runtime)?)?;
        }
    }
    let matched = ex.exec("WILL_RESET_IN_PATTERN", body)?;
    if ex.matched(&matched)? {
        BodyHints::consider(ex, &mut hints.longest, &ex.duration(&matched)?)?;
    }
    let matched = ex.exec("RETRY_AFTER_MS_BODY_PATTERN", body)?;
    if ex.matched(&matched)? {
        let ms = ex.global_number(|| ex.get(&matched, "1"))?;
        if ex.finite(&ms)? { hints.clamped(ex, &ms)?; }
    }
    ex.patterns(&["PLEASE_RETRY_PATTERN", "RETRY_DELAY_FIELD_PATTERN", "TRY_AGAIN_PATTERN"], |pattern| {
        let matched = ex.method(pattern, "exec", std::slice::from_ref(body))?;
        if ex.matched(&matched)? { BodyHints::consider(ex, &mut hints.longest, &ex.duration(&matched)?)?; }
        Ok(None)
    })?;
    // These three RegExp literals are fresh per invocation, rather than
    // module-stamped instances: patched exec/iterator observers retain identity.
    let pattern = heap.regexp(r"retry-after\s*[:=]\s*([^\s,;]+)".into(), "i")?;
    let matched = ex.method(&pattern, "exec", std::slice::from_ref(body))?;
    if heap.truthy(&matched)? {
        let value = ex.get(&matched, "1")?;
        let seconds = ex.number(&value)?;
        if ex.finite(&seconds)? { hints.clamped(ex, &ex.multiply(&seconds, 1000.0)?)?; }
        else {
            let date = ex.date_parse(&value)?;
            if ex.not_nan(&date)? { hints.clamped(ex, &ex.subtract(&date, &ex.now()?)?)?; }
        }
    }
    let pattern = heap.regexp(r"x-ratelimit-reset-ms\s*[:=]\s*(\d+)".into(), "i")?;
    let matched = ex.method(&pattern, "exec", std::slice::from_ref(body))?;
    if heap.truthy(&matched)? {
        let value = ex.global_number(|| ex.get(&matched, "1"))?;
        if ex.not_nan(&value)? {
            let value = if ex.compare(&value, &heap.number(1_000_000_000_000.0)?, ">")? { ex.subtract(&value, &ex.now()?)? } else { value };
            hints.clamped(ex, &value)?;
        }
    }
    let pattern = heap.regexp(r"x-ratelimit-reset\s*[:=]\s*(\d+)".into(), "i")?;
    let matched = ex.method(&pattern, "exec", std::slice::from_ref(body))?;
    if heap.truthy(&matched)? {
        let value = ex.global_number(|| ex.get(&matched, "1"))?;
        if ex.not_nan(&value)? {
            let value = if ex.compare(&value, &heap.number(1_000_000_000.0)?, ">")? { ex.subtract(&ex.multiply(&value, 1000.0)?, &ex.now()?)? } else { ex.multiply(&value, 1000.0)? };
            hints.clamped(ex, &value)?;
        }
    }
    if let Some(value) = hints.longest.or(hints.naive) { Ok(value) }
    else if hints.retry_now { heap.number(0.0) } else { heap.undefined() }
}

#[derive(Default)]
struct BodyHints {
    longest: Option<ValueHandle>,
    naive: Option<ValueHandle>,
    retry_now: bool,
}
impl BodyHints {
    fn consider(ex: Ex<'_>, slot: &mut Option<ValueHandle>, value: &ValueHandle) -> Result<()> {
        if ex.heap.kind(value)? != ValueKind::Undefined && ex.positive(value)?
            && match slot { None => true, Some(current) => ex.compare(value, current, ">")? }
        { *slot = Some(value.clone()); }
        Ok(())
    }
    fn clamped(&mut self, ex: Ex<'_>, value: &ValueHandle) -> Result<()> {
        if ex.heap.kind(value)? == ValueKind::Undefined { return Ok(()); }
        if ex.positive(value)? { Self::consider(ex, &mut self.longest, value)?; }
        else { self.retry_now = true; }
        Ok(())
    }
}

static EXTRACT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP b91db66e packages/utils/src/fetch-retry.ts:79-248 extractRetryHint",
    name: "extractRetryHint",
    length: 3,
    capture_count: 2 + PATTERNS.len(),
    form: CompiledFunctionForm::Ordinary,
    body: |heap, captures, _, args, runtime| {
        extract(
            Ex {
                heap,
                captures,
                runtime,
            },
            &args.first().cloned().unwrap_or(heap.undefined()?),
            &args.get(1).cloned().unwrap_or(heap.undefined()?),
            &args.get(2).cloned().unwrap_or(heap.undefined()?),
        )
    },
};

/// Original narrow catalog dependency. Rules and find callbacks stay Native;
/// no product configuration, machine identity, or Graph-to-serde bridge.
pub(crate) fn prepare_provider_extractor(factory: &Arc<GraphValueFactory>, extractor: ValueHandle) -> Result<ValueHandle> {
    let heap = factory.heap();
    heap.adopt(&extractor)?;
    let mut rules = Vec::new();
    for name in ["zai", "zhipu-coding-plan"] {
        let rule = heap.object()?;
        heap.define_own_data(&rule, "provider".into(), heap.string(name.into())?, &ara_rpc::value_heap::NoRuntime)?;
        heap.define_own_data(&rule, "offset".into(), heap.string("+08:00".into())?, &ara_rpc::value_heap::NoRuntime)?;
        rules.push(rule);
    }
    let behavior = heap.object()?;
    heap.define_own_data(&behavior, "retryResetTimezones".into(), heap.array(rules)?, &ara_rpc::value_heap::NoRuntime)?;
    let policy = heap.fresh_compiled_function(&RESET_POLICY, &[behavior])?;
    heap.fresh_compiled_function(&PROVIDER_EXTRACT, &[extractor, policy])
}
static RESET_POLICY_FIND: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP b91db66e packages/catalog/src/compat/behavior.ts:108 find callback",
    name: "", length: 1, capture_count: 1, form: CompiledFunctionForm::Arrow,
    body: |heap, captures, _, args, runtime| {
        let rule = args.first().cloned().unwrap_or(heap.undefined()?);
        let provider = heap.get(&rule, &"provider".into(), runtime)?;
        let equal = match (heap.snapshot(&provider)?, heap.snapshot(&captures[0])?) {
            (ValueSnapshot::Number(left), ValueSnapshot::Number(right)) => left == right,
            _ => heap.object_is(&provider, &captures[0])?,
        };
        heap.boolean(equal)
    },
};
static RESET_POLICY: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP b91db66e packages/catalog/src/compat/behavior.ts:107-109 retryResetTimezoneOffsetFor",
    name: "retryResetTimezoneOffsetFor", length: 1, capture_count: 1, form: CompiledFunctionForm::Ordinary,
    body: |heap, captures, _, args, runtime| {
        let provider = args.first().cloned().unwrap_or(heap.undefined()?);
        let rules = heap.get(&captures[0], &"retryResetTimezones".into(), runtime)?;
        let find = heap.get(&rules, &"find".into(), runtime)?;
        let callback = heap.fresh_compiled_function(&RESET_POLICY_FIND, &[provider])?;
        let rule = heap.call(&find, &rules, &[callback], runtime)?;
        if matches!(heap.kind(&rule)?, ValueKind::Null | ValueKind::Undefined) { heap.undefined() }
        else { heap.get(&rule, &"offset".into(), runtime) }
    },
};
static PROVIDER_EXTRACT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP b91db66e packages/ai/src/utils/retry-after.ts:7-12 extractProviderRetryHint",
    name: "extractProviderRetryHint", length: 2, capture_count: 2, form: CompiledFunctionForm::Ordinary,
    body: |heap, captures, _, args, runtime| {
        let provider = args.first().cloned().unwrap_or(heap.undefined()?);
        let message = args.get(1).cloned().unwrap_or(heap.undefined()?);
        let offset = if heap.kind(&provider)? == ValueKind::Undefined { heap.undefined()? }
        else { heap.call(&captures[1], &heap.undefined()?, &[provider], runtime)? };
        let options = heap.object()?;
        heap.define_own_data(&options, "naiveResetTimezoneOffset".into(), offset, runtime)?;
        heap.call(&captures[0], &heap.undefined()?, &[heap.undefined()?, message, options], runtime)
    },
};
