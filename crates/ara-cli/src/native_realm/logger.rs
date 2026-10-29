//! Fixed OMP596f2da packages/utils/src/logger.ts compiled module bodies.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
//! Candidate WIP: synchronous module plus external effect ports. Full `time`
//! binding requires separately reviewed concrete Promise/context services.

use super::NativeGlobals;
use ara_rpc::{
    WireString,
    value_heap::{
        CapabilityId, CompiledFunctionForm, CompiledFunctionSpec, NodeId, PropertyDescriptor, PropertyKey,
        RuntimeReply, RuntimeRequest, ValueError, ValueHandle, ValueHeap, ValueKind, ValueRuntime, ValueSnapshot,
    },
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

pub type Result<T> = std::result::Result<T, ValueError>;
pub(super) const LOGGER_PORT: CapabilityId = CapabilityId(0x4152415f4c4f4747);
const BASE_CAPTURES: usize = 7;

/// Concrete adapters belong to the Host. Never retain a graph/Session/runtime
/// lease in an adapter; errors preserve original throws or explicit infra.
pub trait LoggerFile: Send + Sync {
    /// Preserve Date/select/register before original template-string coercion.
    /// The Host borrows this graph value/runtime only for the synchronous call.
    fn write(&self, line: &ValueHandle, runtime: &dyn ValueRuntime) -> Result<()>;
    fn close(&self) -> Result<()>;
}
pub trait LoggerHost: Send + Sync {
    fn environment(&self, name: &str) -> Result<Option<WireString>>;
    fn cwd(&self, heap: &Arc<ValueHeap>, runtime: &dyn ValueRuntime) -> Result<WireString>;
    fn pid(&self) -> Result<f64>;
    fn eol(&self) -> Result<WireString>;
    fn performance_now(&self) -> Result<f64>;
    fn default_logs_directory(&self) -> Result<WireString>;
    /// Original ensure -> prune -> RotatingFileSink construction, with the
    /// current Host PID and explicit namespace/audit-path naming policy.
    fn make_file(
        &self,
        directory: &WireString,
        heap: &Arc<ValueHeap>,
        runtime: &dyn ValueRuntime,
    ) -> Result<Arc<dyn LoggerFile>>;
    fn write_sync(
        &self,
        fd: u32,
        text: &WireString,
        heap: &Arc<ValueHeap>,
        runtime: &dyn ValueRuntime,
    ) -> Result<usize>;
    fn console_error(&self, text: &WireString, heap: &Arc<ValueHeap>, runtime: &dyn ValueRuntime) -> Result<()>;
}

/// Only token/NodeId/internal service owners are retained. Reverse runtime
/// registration is Weak; the prepared Realm keeps this service strongly.
pub struct NativeLoggerDomain {
    heap_token: u64,
    host: Arc<dyn LoggerHost>,
    files: Mutex<HashMap<NodeId, Arc<dyn LoggerFile>>>,
}
impl NativeLoggerDomain {
    pub fn new(heap_token: u64, host: Arc<dyn LoggerHost>) -> Self {
        Self { heap_token, host, files: Mutex::new(HashMap::new()) }
    }
    pub(super) fn check_heap(&self, heap: &Arc<ValueHeap>) -> Result<()> {
        if heap.heap_token() == self.heap_token { Ok(()) } else { Err(ValueError::ForeignHeap) }
    }
    pub fn resolve(
        &self,
        request: &RuntimeRequest<'_>,
        effective_runtime: &dyn ValueRuntime,
    ) -> Option<Result<RuntimeReply>> {
        let RuntimeRequest::Call { capability, receiver, arguments } = request else { return None };
        if *capability != LOGGER_PORT {
            return None;
        }
        Some(self.invoke(receiver, arguments, effective_runtime).map(RuntimeReply::Value))
    }
    fn invoke(&self, receiver: &ValueHandle, args: &[ValueHandle], runtime: &dyn ValueRuntime) -> Result<ValueHandle> {
        if receiver.heap().heap_token() != self.heap_token {
            return Err(ValueError::ForeignHeap);
        }
        let heap = receiver.heap();
        let string = |index: usize| -> Result<WireString> {
            match heap.snapshot(args.get(index).ok_or(ValueError::InvalidNode)?)? {
                ValueSnapshot::String(value) => Ok(value),
                _ => Err(ValueError::UnsupportedOperation(
                    "Native logger Host requires the original primitive-string parameter domain",
                )),
            }
        };
        let operation = string(0)?.to_utf8().map_err(|_| ValueError::WrongRuntimeReply("Native logger operation"))?;
        match operation.as_str() {
            "environment" => {
                let name = string(1)?
                    .to_utf8()
                    .map_err(|_| ValueError::WrongRuntimeReply("Native logger environment name"))?;
                match self.host.environment(&name)? {
                    Some(value) => heap.string(value),
                    None => heap.undefined(),
                }
            }
            "cwd" => heap.string(self.host.cwd(heap, runtime)?),
            "pid" => heap.number(self.host.pid()?),
            "eol" => heap.string(self.host.eol()?),
            "performance" => heap.number(self.host.performance_now()?),
            "defaultDir" => heap.string(self.host.default_logs_directory()?),
            "makeFile" => {
                let directory = string(1)?;
                let target = args.get(2).ok_or(ValueError::InvalidNode)?;
                if target.heap().heap_token() != self.heap_token {
                    return Err(ValueError::ForeignHeap);
                }
                if !matches!(heap.snapshot(target)?, ValueSnapshot::Object) {
                    return Err(ValueError::WrongRuntimeReply("Native logger file receiver"));
                }
                // Admit the graph target before external effects. Host effects
                // still precede registration and cannot be silently replayed.
                let file = self.host.make_file(&directory, heap, runtime)?;
                let mut files = self.files.lock().map_err(|_| ValueError::PoisonedHeap)?;
                if files.contains_key(&target.node_id()) {
                    return Err(ValueError::UnsupportedOperation("Native logger file slot is already initialized"));
                }
                files.try_reserve(1).map_err(|_| ValueError::Allocation)?;
                files.insert(target.node_id(), file);
                Ok(target.clone())
            }
            "writeFile" | "closeFile" => {
                let file =
                    self.files
                        .lock()
                        .map_err(|_| ValueError::PoisonedHeap)?
                        .get(&receiver.node_id())
                        .cloned()
                        .ok_or_else(|| ValueError::TypeError("RotatingFileSink receiver has no native slot".into()))?;
                // Registry guard dropped before effects, including controlled
                // Host reentry. The actual adapter retains original write order.
                if operation == "writeFile" {
                    let line = args.get(1).ok_or(ValueError::InvalidNode)?;
                    if line.heap().heap_token() != self.heap_token {
                        return Err(ValueError::ForeignHeap);
                    }
                    file.write(line, runtime)?;
                } else {
                    file.close()?;
                }
                heap.undefined()
            }
            "writeSync" => {
                let fd = match heap.snapshot(args.get(1).ok_or(ValueError::InvalidNode)?)? {
                    ValueSnapshot::Number(value) if value == 1.0 || value == 2.0 => value as u32,
                    _ => return Err(ValueError::UnsupportedOperation("Native logger fd domain")),
                };
                let written = self.host.write_sync(fd, &string(2)?, heap, runtime)?;
                heap.number(written as f64)
            }
            "consoleError" => {
                self.host.console_error(&string(1)?, heap, runtime)?;
                heap.undefined()
            }
            _ => Err(ValueError::WrongRuntimeReply("Unknown Native logger operation")),
        }
    }
}

/// These handles are owned by the prepared bundle, not by reverse services.
/// `sync_module` has fourteen original exports; no substitute `time` is added.
pub struct NativeLoggerBindings {
    pub sync_module: ValueHandle,
    pub module_load_buffer: ValueHandle,
    pub drain_module_load_events: ValueHandle,
    captures: Vec<ValueHandle>,
}
impl NativeLoggerBindings {
    /// Explicit WIP preparation requires the actual context object (getStore/
    /// run) from the separately reviewed domain. It never installs a no-op ALS.
    pub fn prepare_sync(
        heap: &Arc<ValueHeap>,
        globals: &NativeGlobals,
        context: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> Result<Self> {
        heap.adopt(context)?;
        primitives::install(heap, &globals.environment, runtime)?;
        let state = heap.object()?;
        let port = heap.function(LOGGER_PORT)?;
        let mut captures = vec![state.clone(), globals.environment.clone(), port];
        for constructor in [
            &globals.errors.error,
            &globals.errors.type_error,
            &globals.errors.syntax_error,
            &globals.errors.range_error,
        ] {
            captures.push(heap.get(constructor, &"prototype".into(), runtime)?);
        }
        let c = C { heap, captures: &captures, runtime };
        let opts = c.object(&[("file", heap.boolean(true)?)])?;
        for (key, value) in [
            ("transportOpts", opts),
            ("activeTransports", c.undef()?),
            ("root", c.undef()?),
            ("recording", heap.boolean(false)?),
            ("spanStorage", context.clone()),
            ("logSinks", c.construct("Set", &[])?),
            ("formatTokenPattern", heap.regexp("%[scdjifoO%]".into(), "")?),
        ] {
            heap.define_own_data(&state, key.into(), value, runtime)?;
        }
        ports::install(&c)?;
        // Imported timing-buffer KEY is computed through the actual registry
        // binding once per module preparation and shared by both its functions.
        let symbol = c.env("Symbol")?;
        let key = c.method(&symbol, "for", &[c.text("omp.moduleLoadBuffer")?])?;
        heap.define_own_data(&state, "moduleBufferKey".into(), key, runtime)?;
        for (name, spec) in format::INTERNAL.iter().chain(timing::INTERNAL.iter()) {
            heap.define_own_data(&state, (*name).into(), heap.fresh_compiled_function(spec, &captures)?, runtime)?;
        }
        let module = heap.object()?;
        for (name, spec) in format::EXPORTS.iter().chain(timing::EXPORTS.iter()) {
            let prior = c.state(name)?;
            let original = if c.undefined(&prior)? {
                let function = heap.fresh_compiled_function(spec, &captures)?;
                heap.define_own_data(&state, (*name).into(), function.clone(), runtime)?;
                function
            } else {
                prior
            };
            heap.define_own_data(&module, (*name).into(), original, runtime)?;
        }
        let module_load_buffer = heap.fresh_compiled_function(&timing::MODULE_BUFFER, &captures)?;
        let drain_module_load_events = heap.fresh_compiled_function(&timing::DRAIN_BUFFER, &captures)?;
        heap.define_own_data(&state, "drainModuleLoadEvents".into(), drain_module_load_events.clone(), runtime)?;
        Ok(Self { sync_module: module, module_load_buffer, drain_module_load_events, captures })
    }
    /// The sole graph module cells/storage used by both the complete sync
    /// logger and the separately reviewed original time implementation.
    pub fn module_cells(&self) -> &ValueHandle {
        &self.captures[0]
    }
    pub fn span_storage(&self, runtime: &dyn ValueRuntime) -> Result<ValueHandle> {
        self.sync_module.heap().get(&self.captures[0], &"spanStorage".into(), runtime)
    }
    pub fn time_imports(
        &self,
        runtime: &dyn ValueRuntime,
    ) -> Result<crate::native_logger::time::NativeLoggerTimeImports> {
        let heap = self.sync_module.heap();
        Ok(crate::native_logger::time::NativeLoggerTimeImports {
            performance: heap.get(&self.captures[1], &"performance".into(), runtime)?,
            process: heap.get(&self.captures[1], &"process".into(), runtime)?,
            fs: heap.get(&self.captures[0], &"fs".into(), runtime)?,
        })
    }
    pub fn install_actual_time(
        &self,
        time: &crate::native_logger::time::NativeLoggerTimeModule,
        runtime: &dyn ValueRuntime,
    ) -> Result<()> {
        if !self.module_cells().same_identity(time.module_cells())
            || !self.span_storage(runtime)?.same_identity(time.span_storage())
        {
            return Err(ValueError::WrongRuntimeReply("Logger time must reuse the original module cells and storage"));
        }
        let heap = self.sync_module.heap();
        for (name, function) in
            [("time", &time.time), ("startTiming", &time.start_timing), ("startupMarker", &time.startup_marker)]
        {
            heap.adopt(function)?;
            heap.define_own_data(&self.captures[0], name.into(), function.clone(), runtime)?;
            heap.define_own_data(&self.sync_module, name.into(), function.clone(), runtime)?;
        }
        Ok(())
    }
    /// Called only with the actual separately reviewed original `time` spec.
    /// No absent/placeholder function is installed by synchronous preparation.
    pub fn install_original_time(&self, spec: &'static CompiledFunctionSpec, runtime: &dyn ValueRuntime) -> Result<()> {
        if spec.name != "time" || spec.capture_count != BASE_CAPTURES || spec.form != CompiledFunctionForm::Ordinary {
            return Err(ValueError::WrongRuntimeReply("Native logger original time definition"));
        }
        let heap = self.sync_module.heap();
        heap.define_own_data(
            &self.sync_module,
            "time".into(),
            heap.fresh_compiled_function(spec, &self.captures)?,
            runtime,
        )
    }
}

fn timing_key(key: &str) -> &str {
    match key {
        "gRootSpan" => "root",
        "gRecordTimings" => "recording",
        key => key,
    }
}

struct C<'a> {
    heap: &'a Arc<ValueHeap>,
    captures: &'a [ValueHandle],
    runtime: &'a dyn ValueRuntime,
}
impl C<'_> {
    fn undef(&self) -> Result<ValueHandle> {
        self.heap.undefined()
    }
    fn text(&self, text: &str) -> Result<ValueHandle> {
        self.heap.string(text.into())
    }
    fn number(&self, number: f64) -> Result<ValueHandle> {
        self.heap.number(number)
    }
    fn get(&self, object: &ValueHandle, key: &str) -> Result<ValueHandle> {
        self.heap.get(object, &key.into(), self.runtime)
    }
    fn set(&self, object: &ValueHandle, key: &str, value: ValueHandle) -> Result<()> {
        self.heap.assign(object, key.into(), value, self.runtime)
    }
    fn state(&self, key: &str) -> Result<ValueHandle> {
        self.get(&self.captures[0], timing_key(key))
    }
    fn put(&self, key: &str, value: ValueHandle) -> Result<()> {
        self.set(&self.captures[0], timing_key(key), value)
    }
    fn env(&self, key: &str) -> Result<ValueHandle> {
        self.get(&self.captures[1], key)
    }
    fn kind(&self, value: &ValueHandle) -> Result<ValueKind> {
        self.heap.kind(value)
    }
    fn truth(&self, value: &ValueHandle) -> Result<bool> {
        self.heap.truthy(value)
    }
    fn undefined(&self, value: &ValueHandle) -> Result<bool> {
        Ok(self.kind(value)? == ValueKind::Undefined)
    }
    fn nullish(&self, value: &ValueHandle) -> Result<bool> {
        Ok(matches!(self.kind(value)?, ValueKind::Undefined | ValueKind::Null))
    }
    fn string(&self, value: &ValueHandle) -> Result<WireString> {
        if self.kind(value)? == ValueKind::Symbol {
            return Err(ValueError::TypeError("Cannot convert a Symbol value to a string".into()));
        }
        self.heap.to_string(value, self.runtime)
    }
    fn num(&self, value: &ValueHandle) -> Result<f64> {
        self.heap.to_number(value, self.runtime)
    }
    fn arg(&self, args: &[ValueHandle], index: usize) -> Result<ValueHandle> {
        args.get(index).cloned().map(Ok).unwrap_or_else(|| self.undef())
    }
    fn object(&self, fields: &[(&str, ValueHandle)]) -> Result<ValueHandle> {
        let object = self.heap.object()?;
        for (key, value) in fields {
            self.heap.define_own_data(&object, (*key).into(), value.clone(), self.runtime)?;
        }
        Ok(object)
    }
    fn call(&self, function: &ValueHandle, args: &[ValueHandle]) -> Result<ValueHandle> {
        self.heap.call(function, &self.undef()?, args, self.runtime)
    }
    fn method(&self, object: &ValueHandle, name: &str, args: &[ValueHandle]) -> Result<ValueHandle> {
        let method = self.get(object, name)?;
        self.heap.call(&method, object, args, self.runtime)
    }
    fn helper(&self, name: &str, args: &[ValueHandle]) -> Result<ValueHandle> {
        self.call(&self.state(name)?, args)
    }
    fn port(&self, receiver: &ValueHandle, name: &str, args: &[ValueHandle]) -> Result<ValueHandle> {
        let mut supplied = vec![self.text(name)?];
        supplied.extend_from_slice(args);
        self.heap.call(&self.captures[2], receiver, &supplied, self.runtime)
    }
    fn now(&self) -> Result<ValueHandle> {
        self.method(&self.env("performance")?, "now", &[])
    }
    fn construct(&self, name: &str, args: &[ValueHandle]) -> Result<ValueHandle> {
        self.heap.construct(&self.env(name)?, args, self.runtime)
    }
    fn env_value(&self, name: &str) -> Result<ValueHandle> {
        self.get(&self.get(&self.env("process")?, "env")?, name)
    }
    fn array(&self, values: Vec<ValueHandle>) -> Result<ValueHandle> {
        self.heap.array(values)
    }
    fn push(&self, array: &ValueHandle, value: ValueHandle) -> Result<()> {
        self.method(array, "push", &[value])?;
        Ok(())
    }
    fn list(&self, array: &ValueHandle) -> Result<Vec<ValueHandle>> {
        let mut record = self.heap.get_iterator(array, self.runtime)?;
        let mut values = Vec::new();
        while let Some(value) = self.heap.iterator_next(&mut record, self.runtime)? {
            values.push(value);
        }
        Ok(values)
    }
    fn each(&self, iterable: &ValueHandle, mut body: impl FnMut(ValueHandle) -> Result<()>) -> Result<()> {
        let mut record = self.heap.get_iterator(iterable, self.runtime)?;
        while let Some(value) = self.heap.iterator_next(&mut record, self.runtime)? {
            if let Err(error) = body(value) {
                return Err(match self.heap.iterator_close(&mut record, self.runtime) {
                    Ok(()) => error,
                    Err(close) if error.is_native_exception() && close.is_native_exception() => error,
                    Err(close) => ValueError::IteratorClose { original: Box::new(error), close: Box::new(close) },
                });
            }
        }
        Ok(())
    }
    fn join_text(&self, values: &[WireString]) -> WireString {
        let mut units = Vec::new();
        for value in values {
            units.extend_from_slice(value.units());
        }
        WireString::from_units(units)
    }
    fn interpolate(&self, prefix: &str, value: &ValueHandle, suffix: &str) -> Result<ValueHandle> {
        self.heap.string(self.join_text(&[prefix.into(), self.string(value)?, suffix.into()]))
    }
    fn plus(&self, left: &ValueHandle, right: &ValueHandle) -> Result<ValueHandle> {
        let left = self.heap.to_primitive(left, false, self.runtime)?;
        let right = self.heap.to_primitive(right, false, self.runtime)?;
        if self.kind(&left)? == ValueKind::String || self.kind(&right)? == ValueKind::String {
            return self.heap.string(self.join_text(&[self.string(&left)?, self.string(&right)?]));
        }
        if self.kind(&left)? == ValueKind::BigInt || self.kind(&right)? == ValueKind::BigInt {
            return Err(ValueError::UnsupportedOperation("Logger numeric addition BigInt domain is not installed"));
        }
        self.number(self.num(&left)? + self.num(&right)?)
    }
    fn original_error(&self, error: ValueError) -> ValueError {
        let (index, message) = match error {
            ValueError::TypeError(message) => (4, message),
            ValueError::SyntaxError(message) => (5, message),
            ValueError::RangeError(message) => (6, message),
            other => return other,
        };
        let result: Result<ValueHandle> = (|| {
            let error = self.heap.object_with_prototype(Some(&self.captures[index]))?;
            self.heap.define_own(
                &error,
                "message".into(),
                PropertyDescriptor::Data {
                    value: self.text(&message)?,
                    writable: true,
                    enumerable: false,
                    configurable: true,
                },
                self.runtime,
            )?;
            Ok(error)
        })();
        match result {
            Ok(original) => ValueError::Thrown(original),
            Err(failure) => failure,
        }
    }
}

macro_rules! original {
    ($spec:ident, $name:literal, $length:expr, $body:ident) => {
        pub(super) static $spec: CompiledFunctionSpec = CompiledFunctionSpec {
            source: concat!("OMP596f2da packages/utils/src/logger.ts::", $name),
            name: $name,
            length: $length,
            form: CompiledFunctionForm::Ordinary,
            capture_count: BASE_CAPTURES,
            body: |heap, captures, _, args, runtime| {
                let c = C { heap, captures, runtime };
                $body(&c, args).map_err(|error| c.original_error(error))
            },
        };
    };
}

mod format;
mod ports;
pub(crate) mod primitives;
mod timing;
