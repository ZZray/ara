//! Graph-value write adapter for fixed OMP596f2da rotating-file.ts::write.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY.md.
//! This owns the native sink only, never a heap/runtime/Session lease.

use super::transport::{LoggerHostError, RotatingFileSink};
use ara_rpc::value_heap::{ValueError, ValueHandle, ValueKind, ValueRuntime};
use std::sync::{Mutex, MutexGuard};

#[derive(Debug)]
pub enum GraphFileError {
    Host(LoggerHostError),
    Value(ValueError),
}
impl From<LoggerHostError> for GraphFileError {
    fn from(error: LoggerHostError) -> Self {
        Self::Host(error)
    }
}
impl From<ValueError> for GraphFileError {
    fn from(error: ValueError) -> Self {
        Self::Value(error)
    }
}
pub type GraphFileResult<T> = std::result::Result<T, GraphFileError>;

/// The originating sink privately captures and consumes each write permit.
/// Graph coercion runs with no state lock and may close or reenter this sink.
pub struct NativeGraphFileSink {
    sink: Mutex<RotatingFileSink>,
}
impl NativeGraphFileSink {
    pub fn new(sink: RotatingFileSink) -> Self {
        Self { sink: Mutex::new(sink) }
    }

    fn lock(&self) -> GraphFileResult<MutexGuard<'_, RotatingFileSink>> {
        self.sink.lock().map_err(|_| GraphFileError::Value(ValueError::PoisonedHeap))
    }

    pub fn write(&self, line: &ValueHandle, runtime: &dyn ValueRuntime) -> GraphFileResult<()> {
        let pending = { self.lock()?.begin_write()? };
        let Some(pending) = pending else { return Ok(()) };
        // ValueHeap.to_string also serves explicit String(Symbol). The original
        // template uses implicit conversion, so a direct Symbol must throw.
        let heap = line.heap();
        if heap.kind(line)? == ValueKind::Symbol {
            return Err(ValueError::TypeError("Cannot convert a Symbol value to a string".into()).into());
        }
        let text = heap.to_string(line, runtime)?;
        // No closed/current-path recheck after user coercion. Append captured
        // destination, then add bytes to the sink's current counter as upstream.
        self.lock()?.append_prepared(pending, &text)?;
        Ok(())
    }

    pub fn close(&self) -> GraphFileResult<()> {
        self.lock()?.close();
        Ok(())
    }
}
