//! Fixed OMP596f2da utils/sdk-stream-timeout.ts executable Rust bodies (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
//! These SDK hints do not share pre-response watchdog normalization.

use crate::graph_values::{GraphExecution, GraphValueFactory};
use ara_rpc::value_heap::{
    CompiledFunctionForm, CompiledFunctionSpec, ValueError, ValueHandle, ValueKind,
    ValueRuntime,
};
use std::sync::{Arc, Weak};

type Result<T> = std::result::Result<T, ValueError>;

pub struct SdkTimeoutModule {
    factory: Weak<GraphValueFactory>,
    pub resolve_sdk_timeout_ms: ValueHandle,
    pub create_sdk_stream_request_options: ValueHandle,
}

impl SdkTimeoutModule {
    /// Globals are the actual imported Number/Math objects for this realm.
    /// Invocation uses the caller's effective runtime, retaining factory identity.
    pub fn prepare(factory: &Arc<GraphValueFactory>, globals: ValueHandle) -> Result<Self> {
        let heap = factory.heap();
        heap.adopt(&globals)?;
        let resolve_sdk_timeout_ms =
            heap.fresh_compiled_function(&RESOLVE_TIMEOUT, std::slice::from_ref(&globals))?;
        let create_sdk_stream_request_options =
            heap.fresh_compiled_function(&CREATE_OPTIONS, &[resolve_sdk_timeout_ms.clone()])?;
        Ok(Self {
            factory: Arc::downgrade(factory),
            resolve_sdk_timeout_ms,
            create_sdk_stream_request_options,
        })
    }

    pub fn check_factory(&self, factory: &Arc<GraphValueFactory>) -> Result<()> {
        let original = self.factory.upgrade().ok_or(ValueError::UnsupportedOperation(
            "SDK timeout import owner expired",
        ))?;
        if !Arc::ptr_eq(&original, factory) {
            return Err(ValueError::UnsupportedOperation(
                "SDK timeout requires the original Session factory",
            ));
        }
        Ok(())
    }

    pub fn resolve(
        &self,
        factory: &Arc<GraphValueFactory>,
        value: ValueHandle,
        effective: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        self.check_factory(factory)?;
        factory.heap().call(
            &self.resolve_sdk_timeout_ms,
            &factory.heap().undefined()?,
            &[value],
            effective,
        )
    }

    pub fn options(
        &self,
        factory: &Arc<GraphValueFactory>,
        signal: ValueHandle,
        timeout: ValueHandle,
        effective: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        self.check_factory(factory)?;
        factory.heap().call(
            &self.create_sdk_stream_request_options,
            &factory.heap().undefined()?,
            &[signal, timeout],
            effective,
        )
    }
}

static RESOLVE_TIMEOUT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da utils/sdk-stream-timeout.ts:18-24 resolveSdkTimeoutMs",
    name: "resolveSdkTimeoutMs",
    length: 1,
    capture_count: 1,
    form: CompiledFunctionForm::Ordinary,
    body: |heap, captures, _, args, runtime| {
        let value = args.first().cloned().unwrap_or(heap.undefined()?);
        if heap.kind(&value)? == ValueKind::Undefined {
            return heap.undefined();
        }
        let number = heap.get(&captures[0], &"Number".into(), runtime)?;
        let is_finite = heap.get(&number, &"isFinite".into(), runtime)?;
        let finite = heap.call(&is_finite, &number, std::slice::from_ref(&value), runtime)?;
        if !heap.truthy(&finite)? {
            return heap.undefined();
        }
        if crate::schema_validator::graph_values_compare_with_execution(
            GraphExecution::new(heap, runtime),
            &value,
            &heap.number(0.0)?,
            "<=",
        )? {
            return heap.undefined();
        }
        let math = heap.get(&captures[0], &"Math".into(), runtime)?;
        let trunc = heap.get(&math, &"trunc".into(), runtime)?;
        heap.call(&trunc, &math, &[value], runtime)
    },
};

static CREATE_OPTIONS: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da utils/sdk-stream-timeout.ts:36-43 createSdkStreamRequestOptions",
    name: "createSdkStreamRequestOptions",
    length: 2,
    capture_count: 1,
    form: CompiledFunctionForm::Ordinary,
    body: |heap, captures, _, args, runtime| {
        let signal = args.first().cloned().unwrap_or(heap.undefined()?);
        let supplied = args.get(1).cloned().unwrap_or(heap.undefined()?);
        let timeout = heap.call(&captures[0], &heap.undefined()?, &[supplied], runtime)?;
        let object = heap.object()?;
        heap.define_own_data(&object, "signal".into(), signal, runtime)?;
        if heap.kind(&timeout)? != ValueKind::Undefined {
            heap.define_own_data(&object, "timeout".into(), timeout, runtime)?;
            heap.define_own_data(&object, "maxRetries".into(), heap.number(0.0)?, runtime)?;
        }
        Ok(object)
    },
};
