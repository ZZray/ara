//! Fixed OMP596f2da logger/rotating-file.ts and logger.ts pruning Host effects.
//! MIT; source ownership is retained in THIRD_PARTY_NOTICES.md. The Host supplies
//! every path, clock and process probe. No heap/Session/runtime lease is owned here.

use crate::native_realm::date::DateHost;
use ara_rpc::{WireString, WireValue, value_heap::ValueError};
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug)]
pub enum LoggerHostError {
    Io(io::Error),
    Date(ValueError),
}
impl From<io::Error> for LoggerHostError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}
impl From<ValueError> for LoggerHostError {
    fn from(value: ValueError) -> Self {
        Self::Date(value)
    }
}
pub type Result<T> = std::result::Result<T, LoggerHostError>;

pub struct LogDirEntry {
    pub name: OsString,
    pub is_file: bool,
}
pub struct LogFileMetadata {
    pub size: u64,
    pub mtime_ms: f64,
}
/// Synchronous effects only. Implementations may impose Host grants but must
/// not silently fabricate a successful write, stat or process observation.
pub trait LoggerFs: Send + Sync {
    fn exists(&self, path: &Path) -> bool;
    fn mkdir(&self, path: &Path) -> io::Result<()>;
    fn read_dir(&self, path: &Path) -> io::Result<Vec<LogDirEntry>>;
    fn metadata(&self, path: &Path) -> io::Result<LogFileMetadata>;
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;
    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;
    fn append(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;
    fn touch(&self, path: &Path) -> io::Result<()>;
    fn remove(&self, path: &Path) -> io::Result<()>;
    /// Original process.kill(pid,0): only absent/invalid-process errors are dead.
    fn process_is_running(&self, pid: f64) -> bool;
}
pub struct NativeLoggerFs;
fn closed(file: File) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::io::IntoRawHandle;
        let handle = file.into_raw_handle();
        // Ownership is transferred exactly once, including on a failed close.
        if unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
    #[cfg(unix)]
    {
        use std::os::fd::IntoRawFd;
        let fd = file.into_raw_fd();
        // No retry: a failed close can leave the descriptor's effect unknown.
        if unsafe { libc::close(fd) } == -1 { Err(io::Error::last_os_error()) } else { Ok(()) }
    }
    #[cfg(not(any(windows, unix)))]
    {
        drop(file);
        Err(io::Error::new(io::ErrorKind::Unsupported, "Native logger close port is unavailable"))
    }
}
fn write_and_close(mut file: File, bytes: &[u8]) -> io::Result<()> {
    let result = file.write_all(bytes);
    let close = closed(file);
    result.and(close)
}
fn milliseconds(time: SystemTime) -> f64 {
    let value = |d: std::time::Duration| d.as_secs() as f64 * 1000.0 + f64::from(d.subsec_nanos()) / 1_000_000.0;
    match time.duration_since(UNIX_EPOCH) {
        Ok(d) => value(d),
        Err(e) => -value(e.duration()),
    }
}
impl LoggerFs for NativeLoggerFs {
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
    fn mkdir(&self, path: &Path) -> io::Result<()> {
        fs::create_dir_all(path)
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<LogDirEntry>> {
        fs::read_dir(path)?
            .map(|entry| {
                let entry = entry?;
                Ok(LogDirEntry { name: entry.file_name(), is_file: entry.file_type()?.is_file() })
            })
            .collect()
    }
    fn metadata(&self, path: &Path) -> io::Result<LogFileMetadata> {
        let meta = fs::metadata(path)?;
        Ok(LogFileMetadata { size: meta.len(), mtime_ms: milliseconds(meta.modified()?) })
    }
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        fs::read(path)
    }
    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        write_and_close(OpenOptions::new().write(true).create(true).truncate(true).open(path)?, bytes)
    }
    fn append(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        write_and_close(OpenOptions::new().append(true).create(true).open(path)?, bytes)
    }
    fn touch(&self, path: &Path) -> io::Result<()> {
        closed(OpenOptions::new().append(true).create(true).open(path)?)
    }
    fn remove(&self, path: &Path) -> io::Result<()> {
        match fs::remove_file(path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    }
    fn process_is_running(&self, pid: f64) -> bool {
        if !pid.is_finite() || pid.fract() != 0.0 || pid < 0.0 || pid > f64::from(i32::MAX) {
            return true;
        }
        #[cfg(windows)]
        {
            use windows_sys::Win32::{
                Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, GetLastError, WAIT_OBJECT_0, WAIT_TIMEOUT},
                System::Threading::{OpenProcess, SYNCHRONIZATION_SYNCHRONIZE, WaitForSingleObject},
            };
            // Same pinned Bun Windows probe: process.kill(0, 0) succeeds.
            if pid == 0.0 {
                return true;
            }
            let handle = unsafe { OpenProcess(SYNCHRONIZATION_SYNCHRONIZE, 0, pid as u32) };
            if handle.is_null() {
                return unsafe { GetLastError() } != ERROR_INVALID_PARAMETER;
            }
            let wait = unsafe { WaitForSingleObject(handle, 0) };
            let running = match wait {
                WAIT_OBJECT_0 => false,
                WAIT_TIMEOUT => true,
                _ => true,
            };
            unsafe {
                CloseHandle(handle);
            }
            running
        }
        #[cfg(unix)]
        {
            if unsafe { libc::kill(pid as libc::pid_t, 0) } == 0 {
                return true;
            }
            !matches!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH | libc::EINVAL))
        }
        #[cfg(not(any(windows, unix)))]
        {
            true
        }
    }
}

/// Node path.join's lexical normalization for native Host paths. Never resolve
/// symlinks or filesystem existence. Filename is appended even when rooted.
pub fn log_path_join(directory: &Path, filename: &std::ffi::OsStr) -> PathBuf {
    let mut text = directory.as_os_str().to_os_string();
    if !text.is_empty() {
        text.push(std::path::MAIN_SEPARATOR.to_string());
    }
    text.push(filename);
    let mut result = PathBuf::new();
    for part in Path::new(&text).components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => match result.components().next_back() {
                Some(Component::Normal(_)) => {
                    result.pop();
                }
                Some(Component::Prefix(_)) if !result.has_root() => result.push(".."),
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => result.push(".."),
            },
            other => result.push(other.as_os_str()),
        }
    }
    if result.as_os_str().is_empty() {
        result.push(".");
    }
    result
}
fn path_text(path: &Path) -> WireString {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        WireString::from_units(path.as_os_str().encode_wide().collect())
    }
    #[cfg(not(windows))]
    {
        path.to_string_lossy().into_owned().into()
    }
}
fn text_path(text: &WireString) -> PathBuf {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        PathBuf::from(OsString::from_wide(text.units()))
    }
    #[cfg(not(windows))]
    {
        PathBuf::from(String::from_utf16_lossy(text.units()))
    }
}
fn scalar(value: f64) -> String {
    WireValue::Number(value).stringify()
}
fn number_text(value: f64) -> String {
    if value.is_nan() {
        "NaN".into()
    } else if value == f64::INFINITY {
        "Infinity".into()
    } else if value == f64::NEG_INFINITY {
        "-Infinity".into()
    } else {
        scalar(value)
    }
}
fn clip(value: f64) -> f64 {
    if !value.is_finite() || value.abs() > 8_640_000_000_000_000.0 {
        f64::NAN
    } else if value == 0.0 {
        0.0
    } else {
        value.trunc()
    }
}
fn local_day(clock: &dyn DateHost, ms: f64) -> Result<String> {
    if !ms.is_finite() {
        return Ok("NaN-NaN-NaN".into());
    }
    let parts = clock.local_parts(ms as i64)?;
    Ok(format!("{}-{:02}-{:02}", parts.year, parts.month, parts.day))
}
#[derive(Clone)]
struct AuditEntry {
    date: f64,
    name: WireString,
    hash: WireString,
}
pub struct RotatingFileOptions {
    pub directory: PathBuf,
    pub filename_prefix: String,
    pub filename_suffix: String,
    pub audit_file: PathBuf,
    pub max_bytes: u64,
    pub max_files: usize,
}
pub struct RotatingFileSink {
    options: RotatingFileOptions,
    fs: Arc<dyn LoggerFs>,
    clock: Arc<dyn DateHost>,
    files: Vec<AuditEntry>,
    active_day: Option<String>,
    active_index: u64,
    active_path: Option<PathBuf>,
    active_bytes: u64,
    closed: bool,
}
/// One synchronous write's captured destination. This cannot be cloned or
/// constructed outside this module. No graph/Session/runtime owner is retained.
pub(crate) struct PendingLogWrite {
    path: PathBuf,
}
impl RotatingFileSink {
    pub fn new(options: RotatingFileOptions, fs: Arc<dyn LoggerFs>, clock: Arc<dyn DateHost>) -> Result<Self> {
        let files = Self::read_audit(fs.as_ref(), &options.audit_file);
        let mut sink = Self {
            options,
            fs,
            clock,
            files,
            active_day: None,
            active_index: 0,
            active_path: None,
            active_bytes: 0,
            closed: false,
        };
        let now = clip(sink.clock.now_ms()?);
        sink.select_file(local_day(sink.clock.as_ref(), now)?)?;
        if let Some(path) = sink.active_path.clone() {
            sink.register_file(&path, now)?;
            sink.fs.touch(&path)?;
        }
        Ok(sink)
    }
    pub fn write(&mut self, line: &WireString) -> Result<()> {
        let Some(pending) = self.begin_write()? else { return Ok(()) };
        self.append_prepared(pending, line)
    }
    /// Preserve original Date/select/register effects before graph coercion.
    /// The graph adapter releases its state guard after this method, then does
    /// ToString with only transient handles/runtime before consuming the ticket.
    pub(crate) fn begin_write(&mut self) -> Result<Option<PendingLogWrite>> {
        if self.closed {
            return Ok(None);
        }
        let now = clip(self.clock.now_ms()?);
        self.select_file(local_day(self.clock.as_ref(), now)?)?;
        let Some(path) = self.active_path.clone() else { return Ok(None) };
        self.register_file(&path, now)?;
        Ok(Some(PendingLogWrite { path }))
    }
    /// No new closed/path check: coercion can reenter and mutate the same sink.
    /// Original write still appends its captured destination, then adds bytes
    /// to the current counter, including any intervening inner write effects.
    pub(crate) fn append_prepared(&mut self, pending: PendingLogWrite, line: &WireString) -> Result<()> {
        let mut record = String::from_utf16_lossy(line.units());
        record.push_str(if cfg!(windows) { "\r\n" } else { "\n" });
        self.fs.append(&pending.path, record.as_bytes())?;
        self.active_bytes = self.active_bytes.saturating_add(record.len() as u64);
        Ok(())
    }
    pub fn close(&mut self) {
        self.closed = true;
    }
    pub fn active_path(&self) -> Option<&Path> {
        self.active_path.as_deref()
    }
    fn select_file(&mut self, day: String) -> Result<()> {
        if self.active_day.as_ref() != Some(&day) {
            self.active_day = Some(day.clone());
            self.active_index = 0;
            self.set_active_path(&day);
        }
        while self.active_bytes > self.options.max_bytes {
            self.active_index = self
                .active_index
                .checked_add(1)
                .ok_or(ValueError::UnsupportedOperation("Host log rollover index exhausted"))?;
            self.set_active_path(&day);
        }
        Ok(())
    }
    fn set_active_path(&mut self, day: &str) {
        let index = if self.active_index == 0 { String::new() } else { format!(".{}", self.active_index) };
        let file = format!("{}.{}.{}.log{}", self.options.filename_prefix, day, self.options.filename_suffix, index);
        let path = log_path_join(&self.options.directory, file.as_ref());
        self.active_bytes = self.fs.metadata(&path).map(|m| m.size).unwrap_or(0);
        self.active_path = Some(path);
    }
    fn read_audit(fs: &dyn LoggerFs, path: &Path) -> Vec<AuditEntry> {
        let parse = || -> Option<Vec<AuditEntry>> {
            let bytes = fs.read(path).ok()?;
            let value = WireValue::parse(&String::from_utf8_lossy(&bytes)).ok()?;
            let WireValue::Array(files) = value.get("files")? else { return Some(vec![]) };
            Some(
                files
                    .iter()
                    .filter_map(|v| {
                        let (WireValue::Number(date), WireValue::String(name), WireValue::String(hash)) =
                            (v.get("date")?, v.get("name")?, v.get("hash")?)
                        else {
                            return None;
                        };
                        Some(AuditEntry { date: *date, name: name.clone(), hash: hash.clone() })
                    })
                    .collect(),
            )
        };
        parse().unwrap_or_default()
    }
    fn register_file(&mut self, path: &Path, date: f64) -> Result<()> {
        let name = path_text(path);
        if self.files.iter().any(|file| file.name == name) {
            return Ok(());
        }
        let text = format!("{}LOG_FILE{}", String::from_utf16_lossy(name.units()), number_text(date));
        let hash = ring::digest::digest(&ring::digest::SHA256, text.as_bytes())
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        self.files.push(AuditEntry { date, name, hash: hash.into() });
        while self.files.len() > self.options.max_files {
            let removed = self.files.remove(0);
            let _ = self.fs.remove(&text_path(&removed.name));
        }
        self.fs.write(&self.options.audit_file, self.audit().as_bytes())?;
        Ok(())
    }
    fn audit(&self) -> String {
        let string = |s: &WireString| WireValue::String(s.clone()).stringify();
        let files = if self.files.is_empty() {
            "[]".into()
        } else {
            format!("[\n{}\n    ]", self.files.iter().map(|f| format!("        {{\n            \"date\": {},\n            \"name\": {},\n            \"hash\": {}\n        }}", scalar(f.date), string(&f.name), string(&f.hash))).collect::<Vec<_>>().join(",\n"))
        };
        format!(
            "{{\n    \"keep\": {{\n        \"days\": false,\n        \"amount\": {}\n    }},\n    \"auditLog\": {},\n    \"files\": {},\n    \"hashType\": \"sha256\"\n}}",
            self.options.max_files,
            string(&path_text(&self.options.audit_file)),
            files
        )
    }
}

/// Original pruning with explicit Host filename namespace (fixed OMP uses
/// "omp"; the ARA host's rename is a separately recorded naming decision).
pub fn ensure_and_prune(directory: &Path, prefix: &str, fs: &dyn LoggerFs, clock: &dyn DateHost) -> Result<()> {
    if !fs.exists(directory) {
        fs.mkdir(directory)?;
    }
    let Ok(entries) = fs.read_dir(directory) else { return Ok(()) };
    let now = clip(clock.now_ms()?);
    let today = local_day(clock, now)?;
    let cutoff_ms = if now.is_finite() {
        let parts = clock.local_parts(now as i64)?;
        clip(clock.local_day_to_utc(parts, i64::from(parts.day) - 4)?)
    } else {
        f64::NAN
    };
    let cutoff = local_day(clock, cutoff_ms)?;
    let logs = regex::Regex::new(&format!(
        r"^{}\.([0-9]{{4}}-[0-9]{{2}}-[0-9]{{2}})\.([0-9]+)\.log(?:\.([0-9]+))?$",
        regex::escape(prefix)
    ))
    .map_err(|_| ValueError::UnsupportedOperation("Native log namespace pattern"))?;
    let audits = regex::Regex::new(&format!(r"^\.{}\.([0-9]+)-audit\.json$", regex::escape(prefix)))
        .map_err(|_| ValueError::UnsupportedOperation("Native audit namespace pattern"))?;
    let mut groups: Vec<(String, Vec<(PathBuf, f64)>)> = Vec::new();
    for entry in entries {
        if !entry.is_file {
            continue;
        }
        let name = entry.name.to_string_lossy();
        let log = logs.captures(&name);
        let audit = audits.captures(&name);
        let pid =
            log.as_ref().and_then(|m| m.get(2)).or_else(|| audit.as_ref().and_then(|m| m.get(1))).map(|m| m.as_str());
        let Some(pid) = pid else { continue };
        if pid.is_empty() || fs.process_is_running(pid.parse::<f64>().unwrap_or(f64::NAN)) {
            continue;
        }
        let path = log_path_join(directory, &entry.name);
        if audit.is_some() {
            let _ = fs.remove(&path);
            continue;
        }
        let Some(log) = log else { continue };
        let Some(day) = log.get(1).map(|m| m.as_str()) else { continue };
        if day < cutoff.as_str() || day > today.as_str() {
            let _ = fs.remove(&path);
            continue;
        }
        let rollover = log.get(3).map(|m| m.as_str().parse::<f64>().unwrap_or(f64::INFINITY)).unwrap_or(0.0);
        let key = format!("{pid}:{day}");
        if let Some((_, group)) = groups.iter_mut().find(|(k, _)| *k == key) {
            group.push((path, rollover));
        } else {
            groups.push((key, vec![(path, rollover)]));
        }
    }
    for (_, stale) in &groups {
        if stale.len() <= 1 {
            continue;
        }
        let mut ranked: Vec<_> =
            stale.iter().filter_map(|(p, r)| fs.metadata(p).ok().map(|m| (p, m.mtime_ms, *r))).collect();
        ranked.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal))
                .then_with(|| path_text(a.0).units().cmp(path_text(b.0).units()))
        });
        for (path, _, _) in ranked.into_iter().skip(1) {
            let _ = fs.remove(path);
        }
    }
    Ok(())
}
