//! Selected real Host imports of fixed OMP596f2da logger.ts:365-393,473-483,787-837.
//! MIT; Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük;
//! 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
//! The realm retains this domain strongly; NativeRuntime holds only its Weak.
//! Preparation is explicit. No full process/fs/performance API is installed.

use super::time::NativeLoggerTimeImports;
use crate::native_realm::NativeRealm;
use ara_rpc::{
    WireString,
    value_heap::{
        CapabilityId, CompiledFunctionForm, CompiledFunctionSpec, DeferredValue, PropertyDescriptor, RuntimeReply,
        ValueError, ValueHandle, ValueHeap, ValueRuntime, ValueSnapshot,
    },
};
use std::{
    sync::{Arc, OnceLock},
    time::Instant,
};

type Result<T> = std::result::Result<T, ValueError>;
const PERFORMANCE_NOW: CapabilityId = CapabilityId(0x4152415f4c544e57);
const STARTUP_ENV: CapabilityId = CapabilityId(0x4152415f4c54454e);
const STDERR_WRITE: CapabilityId = CapabilityId(0x4152415f4c545752);
static PROCESS_PERFORMANCE_ORIGIN: OnceLock<Instant> = OnceLock::new();

pub fn claims(capability: CapabilityId) -> bool {
    matches!(capability, PERFORMANCE_NOW | STARTUP_ENV | STDERR_WRITE)
}

/// Call once at the actual CLI process entry; tests may explicitly initialize
/// their own process. This records that entry Instant, not OS creation uptime.
/// Repeated calls reuse the first origin across all realms and logical Sessions.
pub fn initialize_process_performance() {
    PROCESS_PERFORMANCE_ORIGIN.get_or_init(Instant::now);
}

/// The same process-origin clock serves the selected time imports and the
/// complete logger Host. Logical Session/module preparation cannot reset it.
pub fn process_performance_now() -> Result<f64> {
    let origin = PROCESS_PERFORMANCE_ORIGIN.get().ok_or(ValueError::UnsupportedOperation(
        "Process performance origin has not been initialized at the actual Host entry",
    ))?;
    Ok(origin.elapsed().as_secs_f64() * 1000.0)
}

/// Checked edges only: no heap, SessionValues, runtime, factory or graph handle.
/// The sole clock is the same global process-origin OnceLock; no Session/module
/// construction invents a new origin. No domain lock crosses IO or graph code.
pub struct NativeLoggerTimeHostDomain {
    heap_token: u64,
    performance_port: DeferredValue,
    env_port: DeferredValue,
    stderr_port: DeferredValue,
    error_constructor: DeferredValue,
}

impl NativeLoggerTimeHostDomain {
    pub fn new(realm: &NativeRealm) -> Result<Arc<Self>> {
        let heap = realm.values.heap();
        Ok(Arc::new(Self {
            heap_token: heap.heap_token(),
            performance_port: heap.capture_deferred(&heap.object_with_prototype(None)?)?,
            env_port: heap.capture_deferred(&heap.object_with_prototype(None)?)?,
            stderr_port: heap.capture_deferred(&heap.object_with_prototype(None)?)?,
            error_constructor: heap.capture_deferred(&realm.globals.errors.error)?,
        }))
    }

    fn check_heap(&self, heap: &ValueHeap) -> Result<()> {
        if heap.heap_token() != self.heap_token {
            return Err(ValueError::ForeignHeap);
        }
        Ok(())
    }

    /// Construct only selected source-visible objects/functions in this heap.
    /// Function captures contain original port edges, never an Arc domain or
    /// Host runtime. The caller retains the complete module+realm+domain bundle.
    pub fn imports(&self, heap: &Arc<ValueHeap>, runtime: &dyn ValueRuntime) -> Result<NativeLoggerTimeImports> {
        self.check_heap(heap)?;
        let performance = heap.object()?;
        let now =
            heap.fresh_compiled_function(&PERFORMANCE_NOW_BODY, &[heap.restore_deferred(&self.performance_port)?])?;
        heap.define_own_data(&performance, "now".into(), now, runtime)?;

        let env = heap.object()?;
        let getter = heap.fresh_compiled_function(&STARTUP_ENV_BODY, &[heap.restore_deferred(&self.env_port)?])?;
        let ValueSnapshot::Function(target) = heap.snapshot(&getter)? else {
            return Err(ValueError::WrongRuntimeReply("Native startup environment getter is not a compiled Function"));
        };
        heap.define_own(
            &env,
            "PI_DEBUG_STARTUP".into(),
            PropertyDescriptor::Accessor { get: Some(target), set: None, enumerable: true, configurable: true },
            runtime,
        )?;
        let process = heap.object()?;
        heap.define_own_data(&process, "env".into(), env, runtime)?;

        let fs = heap.object()?;
        let write = heap.fresh_compiled_function(&STDERR_WRITE_BODY, &[heap.restore_deferred(&self.stderr_port)?])?;
        heap.define_own_data(&fs, "writeSync".into(), write, runtime)?;
        Ok(NativeLoggerTimeImports { performance, process, fs })
    }

    fn io_failure<T>(&self, heap: &Arc<ValueHeap>, error: std::io::Error, effective: &dyn ValueRuntime) -> Result<T> {
        // A genuine failed IO becomes the originating realm's actual Error.
        // Failures restoring/calling that constructor remain infra carriers;
        // no string rejection, fallback error or successful write is fabricated.
        let constructor = heap.restore_deferred(&self.error_constructor)?;
        let message = heap.string(error.to_string().into())?;
        let original = heap.call(&constructor, &heap.undefined()?, &[message], effective)?;
        Err(ValueError::Thrown(original))
    }

    pub fn resolve_call(
        &self,
        capability: CapabilityId,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Option<Result<RuntimeReply>> {
        if !claims(capability) {
            return None;
        }
        Some((|| {
            let heap = receiver.heap();
            self.check_heap(heap)?;
            let expected = match capability {
                PERFORMANCE_NOW => self.performance_port,
                STARTUP_ENV => self.env_port,
                STDERR_WRITE => self.stderr_port,
                _ => unreachable!(),
            };
            if heap.capture_deferred(receiver)? != expected {
                return Err(ValueError::UnsupportedOperation("Native logger time Host private port identity mismatch"));
            }
            // Every argument is admitted before actual external effects.
            for argument in arguments {
                heap.adopt(argument)?;
            }
            match capability {
                PERFORMANCE_NOW => {
                    if !arguments.is_empty() {
                        return Err(ValueError::UnsupportedOperation("Selected performance.now has no arguments"));
                    }
                    let millis = process_performance_now()?;
                    heap.number(millis).map(RuntimeReply::Value)
                }
                STARTUP_ENV => {
                    if !arguments.is_empty() {
                        return Err(ValueError::UnsupportedOperation(
                            "Selected startup environment getter has no arguments",
                        ));
                    }
                    let value = match std::env::var_os("PI_DEBUG_STARTUP") {
                        Some(value) => heap.string(environment_string(&value)?)?,
                        None => heap.undefined()?,
                    };
                    Ok(RuntimeReply::Value(value))
                }
                STDERR_WRITE => {
                    if arguments.len() != 2 {
                        return Err(ValueError::UnsupportedOperation(
                            "Only selected fs.writeSync(fd=2, text) is connected",
                        ));
                    }
                    if !matches!(heap.snapshot(&arguments[0])?, ValueSnapshot::Number(fd) if fd == 2.0) {
                        return Err(ValueError::UnsupportedOperation(
                            "Selected logger stderr writer only accepts fd=2",
                        ));
                    }
                    let ValueSnapshot::String(text) = heap.snapshot(&arguments[1])? else {
                        return Err(ValueError::UnsupportedOperation(
                            "Selected logger stderr writer requires the original text String",
                        ));
                    };
                    // Node's UTF8 encoding replaces isolated UTF16 surrogates.
                    // No graph coercion is inserted into this selected Host seam.
                    let utf8 = String::from_utf16_lossy(text.units());
                    // Exactly one synchronous native fd=2 write call;
                    // a partial write returns its true byte count, without retry,
                    // write_all, close or re-emission after an unknown IO effect.
                    let outcome = write_stderr_once(utf8.as_bytes());
                    match outcome {
                        Ok(written) => heap.number(written as f64).map(RuntimeReply::Value),
                        Err(error) => self.io_failure(heap, error, effective),
                    }
                }
                _ => Err(ValueError::MissingCapability(capability)),
            }
        })())
    }
}

fn write_stderr_once(bytes: &[u8]) -> std::io::Result<usize> {
    super::stream::write_once(2, bytes)
}

pub(super) fn environment_string(value: &std::ffi::OsStr) -> Result<WireString> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let mut units = Vec::new();
        for unit in value.encode_wide() {
            units.try_reserve(1).map_err(|_| ValueError::Allocation)?;
            units.push(unit);
        }
        Ok(WireString::from_units(units))
    }
    #[cfg(not(windows))]
    {
        // The selected non-Windows binding uses the OS byte sequence's lossy
        // UTF8 decoding, as Node exposes environment values as JS Strings.
        Ok(value.to_string_lossy().as_ref().into())
    }
}

fn value(reply: RuntimeReply) -> Result<ValueHandle> {
    match reply {
        RuntimeReply::Value(value) => Ok(value),
        _ => Err(ValueError::WrongRuntimeReply("Native logger time Host port needs a graph Value")),
    }
}

static PERFORMANCE_NOW_BODY: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "selected real process-origin performance.now Host binding for OMP596f2da logger.ts",
    name: "now",
    length: 0,
    form: CompiledFunctionForm::Builtin,
    capture_count: 1,
    body: |_, captures, _, arguments, effective| {
        value(effective.resolve_call(PERFORMANCE_NOW, &captures[0], arguments, effective)?)
    },
};
static STARTUP_ENV_BODY: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "selected live process.env.PI_DEBUG_STARTUP Host getter for OMP596f2da logger.ts:366",
    name: "get PI_DEBUG_STARTUP",
    length: 0,
    form: CompiledFunctionForm::Builtin,
    capture_count: 1,
    body: |_, captures, _, arguments, effective| {
        value(effective.resolve_call(STARTUP_ENV, &captures[0], arguments, effective)?)
    },
};
static STDERR_WRITE_BODY: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "selected real synchronous stderr fd=2/text UTF8 fs.writeSync for OMP596f2da logger.ts:368",
    name: "writeSync",
    length: 2,
    form: CompiledFunctionForm::Builtin,
    capture_count: 1,
    body: |_, captures, _, arguments, effective| {
        value(effective.resolve_call(STDERR_WRITE, &captures[0], arguments, effective)?)
    },
};
