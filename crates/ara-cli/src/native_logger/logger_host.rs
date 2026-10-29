//! Actual Host adapter for fixed OMP596f2da logger.ts and rotating-file.ts.
//! MIT; Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük;
//! 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
use super::{
    graph::{GraphFileError, NativeGraphFileSink},
    transport::{
        LoggerHostError, NativeLoggerFs, RotatingFileOptions, RotatingFileSink, ensure_and_prune, log_path_join,
    },
};
use crate::native_realm::{
    NativeRealm,
    date::DateHost,
    logger::{LoggerFile, LoggerHost},
};
use ara_rpc::{
    WireString,
    value_heap::{DeferredValue, ValueError, ValueHandle, ValueHeap, ValueRuntime},
};
use std::{path::PathBuf, sync::Arc};

type Result<T> = std::result::Result<T, ValueError>;
pub struct NativeLoggerHostOptions {
    pub default_logs_dir: PathBuf,
    pub filename_prefix: String,
    pub date: Arc<dyn DateHost>,
}
/// Caller retains through the realm's NativeLoggerDomain. Only checked graph
/// IDs are retained; genuine IO error construction borrows the current heap.
pub struct NativeLoggerHost {
    heap_token: u64,
    error: DeferredValue,
    options: NativeLoggerHostOptions,
}
impl NativeLoggerHost {
    fn check_heap(&self, heap: &ValueHeap) -> Result<()> {
        if heap.heap_token() != self.heap_token {
            return Err(ValueError::ForeignHeap);
        }
        Ok(())
    }
    pub fn new(realm: &NativeRealm, options: NativeLoggerHostOptions) -> Result<Arc<Self>> {
        Ok(Arc::new(Self {
            heap_token: realm.values.heap().heap_token(),
            error: realm.values.heap().capture_deferred(&realm.globals.errors.error)?,
            options,
        }))
    }
    fn io_error<T>(&self, heap: &Arc<ValueHeap>, error: std::io::Error, runtime: &dyn ValueRuntime) -> Result<T> {
        native_io_error(self.heap_token, self.error, heap, error, runtime)
    }
    fn host_error<T>(&self, heap: &Arc<ValueHeap>, error: LoggerHostError, runtime: &dyn ValueRuntime) -> Result<T> {
        match error {
            LoggerHostError::Date(error) => Err(error),
            LoggerHostError::Io(error) => self.io_error(heap, error, runtime),
        }
    }
}
fn native_io_error<T>(
    heap_token: u64,
    error_constructor: DeferredValue,
    heap: &Arc<ValueHeap>,
    error: std::io::Error,
    runtime: &dyn ValueRuntime,
) -> Result<T> {
    if heap.heap_token() != heap_token {
        return Err(ValueError::ForeignHeap);
    }
    let constructor = heap.restore_deferred(&error_constructor)?;
    let message = heap.string(error.to_string().into())?;
    let reason = heap.call(&constructor, &heap.undefined()?, &[message], runtime)?;
    Err(ValueError::Thrown(reason))
}
struct NativeLoggerFile {
    heap_token: u64,
    error: DeferredValue,
    sink: NativeGraphFileSink,
}
impl LoggerFile for NativeLoggerFile {
    fn write(&self, line: &ValueHandle, runtime: &dyn ValueRuntime) -> Result<()> {
        if line.heap().heap_token() != self.heap_token {
            return Err(ValueError::ForeignHeap);
        }
        match self.sink.write(line, runtime) {
            Ok(()) => Ok(()),
            Err(GraphFileError::Value(error) | GraphFileError::Host(LoggerHostError::Date(error))) => Err(error),
            Err(GraphFileError::Host(LoggerHostError::Io(error))) => {
                native_io_error(self.heap_token, self.error, line.heap(), error, runtime)
            }
        }
    }
    fn close(&self) -> Result<()> {
        match self.sink.close() {
            Ok(()) => Ok(()),
            Err(GraphFileError::Value(error) | GraphFileError::Host(LoggerHostError::Date(error))) => Err(error),
            // NativeGraphFileSink.close only marks the native sink closed and
            // cannot perform IO. Do not turn an unexpected carrier into success.
            Err(GraphFileError::Host(LoggerHostError::Io(_))) => {
                Err(ValueError::WrongRuntimeReply("Native logger close unexpectedly performed IO"))
            }
        }
    }
}
impl LoggerHost for NativeLoggerHost {
    fn environment(&self, name: &str) -> Result<Option<WireString>> {
        std::env::var_os(name).map(|value| super::time_host::environment_string(&value)).transpose()
    }
    fn cwd(&self, heap: &Arc<ValueHeap>, runtime: &dyn ValueRuntime) -> Result<WireString> {
        self.check_heap(heap)?;
        match std::env::current_dir() {
            Ok(path) => super::time_host::environment_string(path.as_os_str()),
            Err(error) => self.io_error(heap, error, runtime),
        }
    }
    fn pid(&self) -> Result<f64> {
        Ok(std::process::id() as f64)
    }
    fn eol(&self) -> Result<WireString> {
        Ok(if cfg!(windows) { "\r\n" } else { "\n" }.into())
    }
    fn performance_now(&self) -> Result<f64> {
        super::time_host::process_performance_now()
    }
    fn default_logs_directory(&self) -> Result<WireString> {
        super::time_host::environment_string(self.options.default_logs_dir.as_os_str())
    }
    fn make_file(
        &self,
        directory: &WireString,
        heap: &Arc<ValueHeap>,
        runtime: &dyn ValueRuntime,
    ) -> Result<Arc<dyn LoggerFile>> {
        self.check_heap(heap)?;
        let directory = path_from_wire(directory);
        if directory.as_os_str().is_empty() {
            return self.io_error(
                heap,
                std::io::Error::new(std::io::ErrorKind::NotFound, "ENOENT: no such file or directory, mkdir ''"),
                runtime,
            );
        }
        let fs = Arc::new(NativeLoggerFs);
        if let Err(error) =
            ensure_and_prune(&directory, &self.options.filename_prefix, fs.as_ref(), self.options.date.as_ref())
        {
            return self.host_error(heap, error, runtime);
        }
        let pid = std::process::id().to_string();
        let audit = format!(".{}.{pid}-audit.json", self.options.filename_prefix);
        let options = RotatingFileOptions {
            audit_file: log_path_join(&directory, std::ffi::OsStr::new(&audit)),
            directory,
            filename_prefix: self.options.filename_prefix.clone(),
            filename_suffix: pid,
            max_bytes: 10 * 1024 * 1024,
            max_files: 5,
        };
        match RotatingFileSink::new(options, fs, self.options.date.clone()) {
            Ok(sink) => Ok(Arc::new(NativeLoggerFile {
                heap_token: self.heap_token,
                error: self.error,
                sink: NativeGraphFileSink::new(sink),
            })),
            Err(error) => self.host_error(heap, error, runtime),
        }
    }
    fn write_sync(
        &self,
        fd: u32,
        text: &WireString,
        heap: &Arc<ValueHeap>,
        runtime: &dyn ValueRuntime,
    ) -> Result<usize> {
        self.check_heap(heap)?;
        let bytes = String::from_utf16_lossy(text.units());
        match super::stream::write_once(fd as i32, bytes.as_bytes()) {
            Ok(count) => Ok(count),
            Err(error) => self.io_error(heap, error, runtime),
        }
    }
    fn console_error(&self, text: &WireString, heap: &Arc<ValueHeap>, runtime: &dyn ValueRuntime) -> Result<()> {
        self.check_heap(heap)?;
        let mut bytes = String::from_utf16_lossy(text.units()).into_bytes();
        bytes.push(b'\n');
        match super::stream::write_once(2, &bytes) {
            Ok(_) => Ok(()),
            Err(error) => self.io_error(heap, error, runtime),
        }
    }
}
fn path_from_wire(text: &WireString) -> PathBuf {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        PathBuf::from(std::ffi::OsString::from_wide(text.units()))
    }
    #[cfg(not(windows))]
    {
        PathBuf::from(String::from_utf16_lossy(text.units()))
    }
}
