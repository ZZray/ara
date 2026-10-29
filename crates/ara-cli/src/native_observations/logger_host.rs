//! One coarse observation family for the actual fixed-OMP logger Host backend.
//! Real tempfile FS and native owned-process probes are distinct from explicitly
//! controlled clock / PID / metadata / fault observations. No new test entrance.

use ara_cli::{
    native_logger::transport::{
        LogDirEntry, LogFileMetadata, LoggerFs, NativeLoggerFs, RotatingFileOptions, RotatingFileSink, ensure_and_prune,
    },
    native_realm::date::{DateHost, DateLocalParts},
};
use ara_rpc::{WireString, value_heap::ValueError};
use chrono::{Datelike, FixedOffset, TimeZone, Timelike, Utc};
use serde::Serialize;
use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
};

type DateResult<T> = std::result::Result<T, ValueError>;
const FIRST_MS: i64 = 1_709_222_400_123; // 2024-02-29T16:00:00.123Z; +08 March 1.
const NEXT_MS: i64 = FIRST_MS + 86_400_000;

#[derive(Clone, Debug, Eq, PartialEq)]
struct Effect {
    op: &'static str,
    path: Option<PathBuf>,
}
type Trace = Arc<Mutex<Vec<Effect>>>;
fn record(trace: &Trace, op: &'static str, path: Option<&Path>) {
    trace.lock().unwrap().push(Effect { op, path: path.map(Path::to_path_buf) });
}
fn take(trace: &Trace) -> Vec<Effect> {
    std::mem::take(&mut *trace.lock().unwrap())
}
fn ops(trace: &[Effect]) -> Vec<&'static str> {
    trace.iter().map(|effect| effect.op).collect()
}
fn effect(op: &'static str, path: &Path) -> Effect {
    Effect { op, path: Some(path.to_path_buf()) }
}

/// Controlled fixed-offset calendar, not the system local-time implementation.
/// The real DateHost adapter's DST gap remains an explicit separate open item.
struct ObservedClock {
    now: Mutex<i64>,
    offset: FixedOffset,
    trace: Trace,
}
impl ObservedClock {
    fn new(trace: Trace) -> Arc<Self> {
        Arc::new(Self { now: Mutex::new(FIRST_MS), offset: FixedOffset::east_opt(8 * 3600).unwrap(), trace })
    }
    fn set(&self, now: i64) {
        *self.now.lock().unwrap() = now;
    }
}
impl DateHost for ObservedClock {
    fn now_ms(&self) -> DateResult<f64> {
        record(&self.trace, "now", None);
        Ok(*self.now.lock().unwrap() as f64)
    }
    fn local_parts(&self, ms: i64) -> DateResult<DateLocalParts> {
        record(&self.trace, "local", None);
        let date = chrono::DateTime::<Utc>::from_timestamp_millis(ms)
            .ok_or(ValueError::UnsupportedOperation("Observation clock instant"))?
            .with_timezone(&self.offset);
        Ok(DateLocalParts {
            year: date.year(),
            month: date.month(),
            day: date.day(),
            hour: date.hour(),
            minute: date.minute(),
            second: date.second(),
            millisecond: date.timestamp_subsec_millis(),
            timezone_offset: -480.0,
        })
    }
    fn local_day_to_utc(&self, current: DateLocalParts, day: i64) -> DateResult<f64> {
        record(&self.trace, "set-local-day", None);
        let wall = current.wall_time_with_day(day)?;
        let date = self
            .offset
            .from_local_datetime(&wall)
            .single()
            .ok_or(ValueError::UnsupportedOperation("Observation fixed-offset calendar"))?;
        Ok(date.timestamp_millis() as f64)
    }
}

/// Every ordinary FS effect delegates to NativeLoggerFs. Faults are one-shot,
/// explicit and happen before the delegated native operation. No partial-write
/// simulation is used to claim anything about unknown external effects.
struct ObservedFs {
    native: NativeLoggerFs,
    trace: Trace,
    fail_once: Mutex<Option<Effect>>,
    mtimes: Mutex<HashMap<PathBuf, f64>>,
    controlled_live: Mutex<Option<Vec<f64>>>,
}
impl ObservedFs {
    fn new(trace: Trace) -> Arc<Self> {
        Arc::new(Self {
            native: NativeLoggerFs,
            trace,
            fail_once: Mutex::new(None),
            mtimes: Mutex::new(HashMap::new()),
            controlled_live: Mutex::new(None),
        })
    }
    fn fail_next(&self, op: &'static str, path: &Path) {
        assert!(self.fail_once.lock().unwrap().replace(effect(op, path)).is_none());
    }
    fn enter(&self, op: &'static str, path: &Path) -> io::Result<()> {
        record(&self.trace, op, Some(path));
        let mut fault = self.fail_once.lock().unwrap();
        if fault.as_ref() == Some(&effect(op, path)) {
            *fault = None;
            Err(io::Error::other("controlled logger observation fault"))
        } else {
            Ok(())
        }
    }
    fn mtime(&self, path: &Path, value: f64) {
        self.mtimes.lock().unwrap().insert(path.to_path_buf(), value);
    }
    fn live(&self, pids: Vec<f64>) {
        *self.controlled_live.lock().unwrap() = Some(pids);
    }
}
impl LoggerFs for ObservedFs {
    fn exists(&self, path: &Path) -> bool {
        record(&self.trace, "exists", Some(path));
        self.native.exists(path)
    }
    fn mkdir(&self, path: &Path) -> io::Result<()> {
        self.enter("mkdir", path)?;
        self.native.mkdir(path)
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<LogDirEntry>> {
        self.enter("read-dir", path)?;
        self.native.read_dir(path)
    }
    fn metadata(&self, path: &Path) -> io::Result<LogFileMetadata> {
        self.enter("stat", path)?;
        let mut meta = self.native.metadata(path)?;
        if let Some(mtime) = self.mtimes.lock().unwrap().get(path).copied() {
            meta.mtime_ms = mtime;
        }
        Ok(meta)
    }
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.enter("read", path)?;
        self.native.read(path)
    }
    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        self.enter("write", path)?;
        self.native.write(path, bytes)
    }
    fn append(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        self.enter("append", path)?;
        self.native.append(path, bytes)
    }
    fn touch(&self, path: &Path) -> io::Result<()> {
        self.enter("touch", path)?;
        self.native.touch(path)
    }
    fn remove(&self, path: &Path) -> io::Result<()> {
        self.enter("remove", path)?;
        self.native.remove(path)
    }
    fn process_is_running(&self, pid: f64) -> bool {
        record(&self.trace, "pid", None);
        match self.controlled_live.lock().unwrap().as_ref() {
            Some(live) => live.contains(&pid),
            None => self.native.process_is_running(pid),
        }
    }
}

fn options(dir: &Path, suffix: &str, max_bytes: u64, max_files: usize) -> RotatingFileOptions {
    RotatingFileOptions {
        directory: dir.to_path_buf(),
        filename_prefix: "omp".into(),
        filename_suffix: suffix.into(),
        audit_file: dir.join(format!(".omp.{suffix}-audit.json")),
        max_bytes,
        max_files,
    }
}
fn log(dir: &Path, day: &str, pid: &str, index: Option<&str>) -> PathBuf {
    dir.join(format!("omp.{day}.{pid}.log{}", index.map(|i| format!(".{i}")).unwrap_or_default()))
}
fn line(text: &str) -> WireString {
    text.into()
}
fn record_bytes(text: &str) -> Vec<u8> {
    format!("{text}{}", if cfg!(windows) { "\r\n" } else { "\n" }).into_bytes()
}
fn seed(path: &Path) {
    fs::write(path, b"fixture").unwrap();
}

// The backend's manual audit layout is compared with a separate serializer
// configured to the original JSON.stringify(..., undefined, 4) indentation.
// Struct declaration order pins the original fields, independent of map order.
#[derive(Serialize)]
struct ExpectedKeep {
    days: bool,
    amount: usize,
}
#[derive(Serialize)]
struct ExpectedEntry {
    date: i64,
    name: String,
    hash: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExpectedAudit {
    keep: ExpectedKeep,
    audit_log: String,
    files: Vec<ExpectedEntry>,
    hash_type: &'static str,
}
fn expected_audit(audit: &Path, amount: usize, entries: &[(PathBuf, i64)]) -> Vec<u8> {
    let files = entries
        .iter()
        .map(|(path, date)| {
            let name = path.to_str().unwrap().to_owned();
            let hash = ring::digest::digest(&ring::digest::SHA256, format!("{name}LOG_FILE{date}").as_bytes())
                .as_ref()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            ExpectedEntry { date: *date, name, hash }
        })
        .collect();
    let expected = ExpectedAudit {
        keep: ExpectedKeep { days: false, amount },
        audit_log: audit.to_str().unwrap().to_owned(),
        files,
        hash_type: "sha256",
    };
    let mut bytes = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
    expected.serialize(&mut serde_json::Serializer::with_formatter(&mut bytes, formatter)).unwrap();
    bytes
}

fn observe_rotation_and_audit(root: &Path) {
    let dir = root.join("rotation-日志");
    fs::create_dir(&dir).unwrap();
    let trace: Trace = Arc::default();
    let clock = ObservedClock::new(trace.clone());
    let observed = ObservedFs::new(trace.clone());
    let bytes = record_bytes("中😀");
    let mut sink =
        RotatingFileSink::new(options(&dir, "91", bytes.len() as u64, 3), observed.clone(), clock.clone()).unwrap();
    let first = log(&dir, "2024-03-01", "91", None);
    let rolled = log(&dir, "2024-03-01", "91", Some("1"));
    let next = log(&dir, "2024-03-02", "91", None);
    let audit = dir.join(".omp.91-audit.json");
    assert_eq!(ops(&take(&trace)), ["read", "now", "local", "stat", "write", "touch"]);
    assert_eq!(fs::read(&audit).unwrap(), expected_audit(&audit, 3, &[(first.clone(), FIRST_MS)]));
    assert!(fs::read(&first).unwrap().is_empty());

    sink.write(&line("中😀")).unwrap();
    assert_eq!(sink.active_path(), Some(first.as_path()));
    assert_eq!(fs::read(&first).unwrap(), bytes);
    sink.write(&line("")).unwrap(); // Prior bytes == limit: original does not rotate yet.
    assert_eq!(sink.active_path(), Some(first.as_path()));
    assert!(!rolled.exists());
    sink.write(&line("tail")).unwrap(); // Prior bytes > limit: rotate before this record.
    assert_eq!(sink.active_path(), Some(rolled.as_path()));
    assert_eq!(fs::read(&rolled).unwrap(), record_bytes("tail"));
    let mut first_bytes = bytes;
    first_bytes.extend(record_bytes(""));
    assert_eq!(fs::read(&first).unwrap(), first_bytes);

    clock.set(NEXT_MS);
    sink.write(&line("new-day")).unwrap();
    assert_eq!(sink.active_path(), Some(next.as_path()));
    assert_eq!(fs::read(&next).unwrap(), record_bytes("new-day"));
    let expected = expected_audit(&audit, 3, &[(first, FIRST_MS), (rolled, FIRST_MS), (next.clone(), NEXT_MS)]);
    assert_eq!(fs::read(&audit).unwrap(), expected);
    assert_ne!(expected.last(), Some(&b'\n'));

    sink.close();
    take(&trace);
    sink.write(&line("closed")).unwrap();
    assert!(take(&trace).is_empty(), "Closed sink must not sample Date or touch FS");
    let mut reopened = RotatingFileSink::new(options(&dir, "91", 1024, 3), observed, clock).unwrap();
    assert_eq!(ops(&take(&trace)), ["read", "now", "local", "stat", "touch"]);
    assert_eq!(fs::read(&audit).unwrap(), expected, "Registered path reopens without new audit entry");
    reopened.write(&line("reopened")).unwrap();
    let mut next_bytes = record_bytes("new-day");
    next_bytes.extend(record_bytes("reopened"));
    assert_eq!(fs::read(&next).unwrap(), next_bytes);
    reopened.close();
}

fn observe_existing_files_and_faults(root: &Path) {
    let dir = root.join("oversized");
    fs::create_dir(&dir).unwrap();
    let trace: Trace = Arc::default();
    let clock = ObservedClock::new(trace.clone());
    let observed = ObservedFs::new(trace.clone());
    let first = log(&dir, "2024-03-01", "92", None);
    let second = log(&dir, "2024-03-01", "92", Some("1"));
    let third = log(&dir, "2024-03-01", "92", Some("2"));
    fs::write(&first, b"12345").unwrap();
    fs::write(&second, b"12345").unwrap();
    let mut sink = RotatingFileSink::new(options(&dir, "92", 4, 2), observed.clone(), clock.clone()).unwrap();
    assert_eq!(sink.active_path(), Some(third.as_path()));
    assert_eq!(ops(&take(&trace)), ["read", "now", "local", "stat", "stat", "stat", "write", "touch"]);
    sink.write(&line("x")).unwrap();
    assert_eq!(fs::read(&third).unwrap(), record_bytes("x"));
    sink.close();

    // Original constructor registers before its open/close touch failure.
    let dir = root.join("constructor-fault");
    fs::create_dir(&dir).unwrap();
    let path = log(&dir, "2024-03-01", "93", None);
    let audit = dir.join(".omp.93-audit.json");
    take(&trace);
    observed.fail_next("touch", &path);
    assert!(RotatingFileSink::new(options(&dir, "93", 4, 1), observed.clone(), clock.clone()).is_err());
    assert_eq!(ops(&take(&trace)), ["read", "now", "local", "stat", "write", "touch"]);
    assert_eq!(fs::read(&audit).unwrap(), expected_audit(&audit, 1, &[(path.clone(), FIRST_MS)]));
    assert!(!path.exists());

    // Audit failure follows internal registration and best-effort FIFO deletion.
    // A later call on that same path must not manufacture an audit repair.
    let dir = root.join("audit-fault");
    fs::create_dir(&dir).unwrap();
    let old = log(&dir, "2024-03-01", "94", None);
    let new = log(&dir, "2024-03-02", "94", None);
    let audit = dir.join(".omp.94-audit.json");
    let mut sink = RotatingFileSink::new(options(&dir, "94", 4, 1), observed.clone(), clock.clone()).unwrap();
    let old_audit = fs::read(&audit).unwrap();
    clock.set(NEXT_MS);
    observed.fail_next("write", &audit);
    take(&trace);
    assert!(sink.write(&line("first-attempt")).is_err());
    assert_eq!(
        take(&trace),
        [
            Effect { op: "now", path: None },
            Effect { op: "local", path: None },
            effect("stat", &new),
            effect("remove", &old),
            effect("write", &audit),
        ]
    );
    assert_eq!(sink.active_path(), Some(new.as_path()));
    assert!(!old.exists());
    assert!(!new.exists());
    assert_eq!(fs::read(&audit).unwrap(), old_audit);
    sink.write(&line("accepted")).unwrap();
    assert_eq!(ops(&take(&trace)), ["now", "local", "append"]);
    assert_eq!(fs::read(&audit).unwrap(), old_audit);
    assert_eq!(fs::read(&new).unwrap(), record_bytes("accepted"));
    sink.close();

    // Failure before native append leaves byte count unchanged; no replay.
    clock.set(FIRST_MS);
    let dir = root.join("append-fault");
    fs::create_dir(&dir).unwrap();
    let first = log(&dir, "2024-03-01", "95", None);
    let rolled = log(&dir, "2024-03-01", "95", Some("1"));
    let limit = record_bytes("x").len() as u64;
    let mut sink = RotatingFileSink::new(options(&dir, "95", limit, 2), observed.clone(), clock.clone()).unwrap();
    take(&trace);
    observed.fail_next("append", &first);
    assert!(sink.write(&line("would-exceed-the-limit")).is_err());
    assert_eq!(ops(&take(&trace)), ["now", "local", "append"]);
    assert!(fs::read(&first).unwrap().is_empty());
    sink.write(&line("x")).unwrap();
    assert_eq!(sink.active_path(), Some(first.as_path()));
    sink.write(&line("")).unwrap();
    assert_eq!(sink.active_path(), Some(first.as_path()));
    assert!(!rolled.exists());
    sink.write(&line("next")).unwrap();
    assert_eq!(sink.active_path(), Some(rolled.as_path()));
    assert_eq!(fs::read(&rolled).unwrap(), record_bytes("next"));
    sink.close();

    // Best-effort retention deletion is observed independently of audit failure.
    let dir = root.join("remove-fault");
    fs::create_dir(&dir).unwrap();
    let old = log(&dir, "2024-03-01", "96", None);
    let new = log(&dir, "2024-03-02", "96", None);
    let audit = dir.join(".omp.96-audit.json");
    let mut sink = RotatingFileSink::new(options(&dir, "96", 1024, 1), observed.clone(), clock.clone()).unwrap();
    take(&trace);
    clock.set(NEXT_MS);
    observed.fail_next("remove", &old);
    sink.write(&line("retention-is-best-effort")).unwrap();
    assert_eq!(ops(&take(&trace)), ["now", "local", "stat", "remove", "write", "append"]);
    assert!(old.exists());
    assert_eq!(fs::read(&audit).unwrap(), expected_audit(&audit, 1, &[(new.clone(), NEXT_MS)]));
    assert_eq!(fs::read(&new).unwrap(), record_bytes("retention-is-best-effort"));
    sink.close();
}

fn observe_calendar_and_prune(root: &Path) {
    let dir = root.join("prune");
    fs::create_dir(&dir).unwrap();
    let trace: Trace = Arc::default();
    let clock = ObservedClock::new(trace.clone());
    let observed = ObservedFs::new(trace.clone());
    observed.live(vec![777.0]); // Controlled PID policy; actual native probe is below.
    let cutoff = log(&dir, "2024-02-26", "201", None);
    let too_old = log(&dir, "2024-02-25", "202", None);
    let tomorrow = log(&dir, "2024-03-02", "203", None);
    let live_old = log(&dir, "2020-01-01", "777", None);
    let live_more = log(&dir, "2020-01-01", "777", Some("3"));
    let live_audit = dir.join(".omp.777-audit.json");
    let dead_audit = dir.join(".omp.204-audit.json");
    let unrelated = dir.join("host.log");
    let directory_match = log(&dir, "2020-01-01", "205", None);
    for path in [&cutoff, &too_old, &tomorrow, &live_old, &live_more, &live_audit, &dead_audit, &unrelated] {
        seed(path);
    }
    fs::create_dir(&directory_match).unwrap();

    // Fractional mtime outranks rollover; equal mtimes use numeric rollover.
    let newer = log(&dir, "2024-03-01", "211", None);
    let older = log(&dir, "2024-03-01", "211", Some("99"));
    seed(&newer);
    seed(&older);
    observed.mtime(&newer, 10.25);
    observed.mtime(&older, 10.125);
    let low_roll = log(&dir, "2024-03-01", "212", Some("2"));
    let high_roll = log(&dir, "2024-03-01", "212", Some("10"));
    seed(&low_roll);
    seed(&high_roll);
    observed.mtime(&low_roll, 20.5);
    observed.mtime(&high_roll, 20.5);
    // Same Number(rollover) and mtime: full UTF16 path ascending decides.
    let path_first = log(&dir, "2024-03-01", "213", Some("01"));
    let path_later = log(&dir, "2024-03-01", "213", Some("1"));
    seed(&path_first);
    seed(&path_later);
    observed.mtime(&path_first, 30.0);
    observed.mtime(&path_later, 30.0);
    // Raw PID text is the group identity even when Number(pidText) is equal.
    let raw_a = log(&dir, "2024-03-01", "0214", None);
    let raw_b = log(&dir, "2024-03-01", "214", None);
    seed(&raw_a);
    seed(&raw_b);
    let stat_missing = log(&dir, "2024-03-01", "215", Some("5"));
    let stat_ok = log(&dir, "2024-03-01", "215", None);
    seed(&stat_missing);
    seed(&stat_ok);
    observed.fail_next("stat", &stat_missing);

    ensure_and_prune(&dir, "omp", observed.as_ref(), clock.as_ref()).unwrap();
    let effects = take(&trace);
    assert_eq!(&ops(&effects)[..6], ["exists", "read-dir", "now", "local", "local", "set-local-day"]);
    assert_eq!(effects.iter().filter(|e| e.op == "now").count(), 1);
    assert!(cutoff.exists(), "+08 March 1 minus four calendar days crosses into leap-year February 26");
    assert!(!too_old.exists());
    assert!(!tomorrow.exists());
    assert!(!dead_audit.exists());
    for path in [
        &live_old,
        &live_more,
        &live_audit,
        &unrelated,
        &directory_match,
        &newer,
        &high_roll,
        &path_first,
        &raw_a,
        &raw_b,
        &stat_missing,
        &stat_ok,
    ] {
        assert!(path.exists(), "Original prune must retain {}", path.display());
    }
    for path in [&older, &low_roll, &path_later] {
        assert!(!path.exists(), "Original prune must remove {}", path.display());
    }
    assert!(!effects.iter().any(|e| e.op == "stat" && e.path.as_ref() == Some(&live_old)));
    assert!(
        !effects
            .iter()
            .any(|e| e.op == "remove"
                && [&live_old, &live_more, &live_audit].iter().any(|p| e.path.as_ref() == Some(*p)))
    );

    // Ensure a new directory, and return before Date on a read-dir failure.
    let missing = root.join("ensure").join("nested");
    ensure_and_prune(&missing, "omp", observed.as_ref(), clock.as_ref()).unwrap();
    assert!(missing.is_dir());
    assert_eq!(&ops(&take(&trace))[..3], ["exists", "mkdir", "read-dir"]);
    observed.fail_next("read-dir", &missing);
    ensure_and_prune(&missing, "omp", observed.as_ref(), clock.as_ref()).unwrap();
    assert_eq!(ops(&take(&trace)), ["exists", "read-dir"]);
}

fn observe_native_pid_and_prune(root: &Path) {
    let native = NativeLoggerFs;
    let current = std::process::id();
    assert!(current > 0 && current <= i32::MAX as u32, "Current PID must enter the supported native probe domain");
    assert!(native.process_is_running(f64::from(current)), "Current test process is actually live");
    assert!(native.process_is_running(0.0));
    for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0, 0.5, f64::from(i32::MAX) + 1.0, f64::MAX] {
        assert!(native.process_is_running(invalid), "Invalid native PID domain conservatively retained: {invalid}");
    }
    // Same owned test executable; --list does not run tests or any live command.
    // Reap exactly this child before probing. No pid lookup or arbitrary kill.
    let executable = std::env::current_exe().unwrap();
    let mut child = Command::new(executable)
        .arg("--list")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let exited = child.id();
    assert!(exited > 0 && exited <= i32::MAX as u32, "Owned child PID must enter the supported native probe domain");
    let status = child.wait().unwrap();
    assert!(status.success(), "Owned test child --list must exit successfully");
    assert_ne!(exited, current);
    assert!(!native.process_is_running(f64::from(exited)), "Reaped owned child PID is actually exited");

    let dir = root.join("native-pid-prune");
    fs::create_dir(&dir).unwrap();
    let live_log = log(&dir, "2020-01-01", &current.to_string(), None);
    let live_audit = dir.join(format!(".omp.{current}-audit.json"));
    let dead_log = log(&dir, "2020-01-01", &exited.to_string(), None);
    let dead_audit = dir.join(format!(".omp.{exited}-audit.json"));
    let pid_zero = log(&dir, "2020-01-01", "0", None);
    let invalid_pid = log(&dir, "2020-01-01", "2147483648", None);
    for path in [&live_log, &live_audit, &dead_log, &dead_audit, &pid_zero, &invalid_pid] {
        seed(path);
    }
    let trace: Trace = Arc::default();
    let clock = ObservedClock::new(trace);
    ensure_and_prune(&dir, "omp", &native, clock.as_ref()).unwrap();
    assert!(live_log.exists());
    assert!(live_audit.exists());
    assert!(pid_zero.exists());
    assert!(invalid_pid.exists());
    assert!(!dead_log.exists());
    assert!(!dead_audit.exists());
    eprintln!("Logger native PID observations: current={current}; owned reaped --list child={exited}; exit={status}");
}

/// Root wires this into the existing native coarse family. tempfile owns only
/// this fresh root; none of the candidate code reads a user logs/config path.
#[cfg(windows)]
fn observe_win32_path_join() {
    use ara_cli::native_logger::transport::log_path_join;
    // Exact rows from the retained fixed-source Bun win32.join observation.
    let filename = std::ffi::OsStr::new("omp.2026-10-07.123.log");
    let rows = [
        (r"C:..", r"C:..\omp.2026-10-07.123.log"),
        (r"C:foo\..\..", r"C:..\omp.2026-10-07.123.log"),
        (r"C:\..", r"C:\omp.2026-10-07.123.log"),
        (r"C:\x\..\..", r"C:\omp.2026-10-07.123.log"),
        (".", "omp.2026-10-07.123.log"),
        ("..", r"..\omp.2026-10-07.123.log"),
    ];
    for (directory, expected) in rows {
        assert_eq!(
            log_path_join(Path::new(directory), filename),
            PathBuf::from(expected),
            "Fixed Bun win32.join: {directory}"
        );
    }
    eprintln!("Logger win32 lexical join: all 6 fixed Bun reference rows matched Rust");
}

use ara_cli::native_logger::graph::{GraphFileError, NativeGraphFileSink};
use ara_rpc::value_heap::{
    CapabilityId, NoRuntime, PropertyDescriptor, RuntimeReply, RuntimeRequest, ValueHeap, ValueRuntime,
};
use std::sync::atomic::{AtomicUsize, Ordering};

const GRAPH_COERCION: CapabilityId = CapabilityId(0x4152415f4c475256);
enum GraphCoercion<'a> {
    Close,
    Rotate(&'a ObservedClock),
    Throw(&'a ara_rpc::value_heap::ValueHandle),
}
struct GraphCoercionRuntime<'a> {
    file: &'a NativeGraphFileSink,
    action: GraphCoercion<'a>,
    calls: AtomicUsize,
}
impl ValueRuntime for GraphCoercionRuntime<'_> {
    fn resolve(&self, request: RuntimeRequest<'_>) -> DateResult<RuntimeReply> {
        match request {
            RuntimeRequest::Call { capability, receiver, arguments } => {
                self.resolve_call(capability, receiver, arguments, self)
            }
            request => NoRuntime.resolve(request),
        }
    }
    fn resolve_call(
        &self,
        capability: CapabilityId,
        receiver: &ara_rpc::value_heap::ValueHandle,
        arguments: &[ara_rpc::value_heap::ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> DateResult<RuntimeReply> {
        if capability != GRAPH_COERCION {
            return NoRuntime.resolve_call(capability, receiver, arguments, effective);
        }
        assert!(arguments.is_empty());
        self.calls.fetch_add(1, Ordering::Relaxed);
        let heap = receiver.heap();
        match &self.action {
            GraphCoercion::Close => self.file.close().expect("Reentrant close must succeed"),
            GraphCoercion::Rotate(clock) => {
                clock.set(NEXT_MS);
                self.file
                    .write(&heap.string("inner".into())?, effective)
                    .expect("Actual inner native write must succeed");
            }
            GraphCoercion::Throw(reason) => return Err(ValueError::Thrown((*reason).clone())),
        }
        Ok(RuntimeReply::Value(heap.string("outer".into())?))
    }
    fn resolve_local_call(
        &self,
        token: u64,
        receiver: &ara_rpc::value_heap::ValueHandle,
        arguments: &[ara_rpc::value_heap::ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> DateResult<RuntimeReply> {
        NoRuntime.resolve_local_call(token, receiver, arguments, effective)
    }
}
fn graph_line(heap: &Arc<ValueHeap>) -> ara_rpc::value_heap::ValueHandle {
    let object = heap.object().unwrap();
    heap.define_own(
        &object,
        "toString".into(),
        PropertyDescriptor::Data {
            value: heap.function(GRAPH_COERCION).unwrap(),
            writable: true,
            enumerable: true,
            configurable: true,
        },
        &NoRuntime,
    )
    .unwrap();
    object
}

fn observe_graph_file_reentry(root: &Path) {
    let heap = ValueHeap::new().unwrap();
    let value = graph_line(&heap);
    let reason = heap.object().unwrap();

    let dir = root.join("graph-close");
    fs::create_dir(&dir).unwrap();
    let trace: Trace = Arc::default();
    let clock = ObservedClock::new(trace.clone());
    let file = NativeGraphFileSink::new(
        RotatingFileSink::new(options(&dir, "101", 1024, 3), ObservedFs::new(trace.clone()), clock).unwrap(),
    );
    take(&trace);
    let runtime = GraphCoercionRuntime { file: &file, action: GraphCoercion::Close, calls: AtomicUsize::new(0) };
    file.write(&value, &runtime).unwrap();
    assert_eq!(runtime.calls.load(Ordering::Relaxed), 1);
    assert_eq!(fs::read(log(&dir, "2024-03-01", "101", None)).unwrap(), record_bytes("outer"));
    assert_eq!(ops(&take(&trace)), ["now", "local", "append"]);
    let runtime =
        GraphCoercionRuntime { file: &file, action: GraphCoercion::Throw(&reason), calls: AtomicUsize::new(0) };
    file.write(&value, &runtime).unwrap();
    assert_eq!(runtime.calls.load(Ordering::Relaxed), 0, "Closed guard precedes ToString");
    assert!(take(&trace).is_empty());

    let dir = root.join("graph-rotate");
    fs::create_dir(&dir).unwrap();
    let trace: Trace = Arc::default();
    let clock = ObservedClock::new(trace.clone());
    let file = NativeGraphFileSink::new(
        RotatingFileSink::new(
            options(&dir, "102", record_bytes("inner").len() as u64, 3),
            ObservedFs::new(trace.clone()),
            clock.clone(),
        )
        .unwrap(),
    );
    let runtime =
        GraphCoercionRuntime { file: &file, action: GraphCoercion::Rotate(clock.as_ref()), calls: AtomicUsize::new(0) };
    take(&trace);
    file.write(&value, &runtime).unwrap();
    assert_eq!(runtime.calls.load(Ordering::Relaxed), 1);
    assert_eq!(fs::read(log(&dir, "2024-03-01", "102", None)).unwrap(), record_bytes("outer"));
    assert_eq!(fs::read(log(&dir, "2024-03-02", "102", None)).unwrap(), record_bytes("inner"));
    assert_eq!(ops(&take(&trace)), ["now", "local", "now", "local", "stat", "write", "append", "append"]);
    file.write(&heap.string("after".into()).unwrap(), &NoRuntime).unwrap();
    assert_eq!(fs::read(log(&dir, "2024-03-02", "102", Some("1"))).unwrap(), record_bytes("after"));
    assert_eq!(ops(&take(&trace)), ["now", "local", "stat", "write", "append"]);

    let dir = root.join("graph-throw");
    fs::create_dir(&dir).unwrap();
    let trace: Trace = Arc::default();
    let clock = ObservedClock::new(trace.clone());
    let file = NativeGraphFileSink::new(
        RotatingFileSink::new(options(&dir, "103", 1024, 1), ObservedFs::new(trace.clone()), clock.clone()).unwrap(),
    );
    clock.set(NEXT_MS);
    let runtime =
        GraphCoercionRuntime { file: &file, action: GraphCoercion::Throw(&reason), calls: AtomicUsize::new(0) };
    match file.write(&value, &runtime).unwrap_err() {
        GraphFileError::Value(ValueError::Thrown(actual)) => assert!(actual.same_identity(&reason)),
        other => panic!("Original thrown carrier was changed: {other:?}"),
    }
    assert!(!log(&dir, "2024-03-01", "103", None).exists());
    assert!(!log(&dir, "2024-03-02", "103", None).exists());
    let audit = dir.join(".omp.103-audit.json");
    assert_eq!(
        fs::read(&audit).unwrap(),
        expected_audit(&audit, 1, &[(log(&dir, "2024-03-02", "103", None), NEXT_MS)])
    );
    take(&trace);
    let third_ms = NEXT_MS + 86_400_000;
    clock.set(third_ms);
    match file.write(&heap.symbol(Some("implicit".into())).unwrap(), &NoRuntime).unwrap_err() {
        GraphFileError::Value(ValueError::TypeError(_)) => {}
        other => panic!("Implicit Symbol must throw: {other:?}"),
    }
    assert_eq!(ops(&take(&trace)), ["now", "local", "stat", "remove", "write"]);
    assert_eq!(
        fs::read(&audit).unwrap(),
        expected_audit(&audit, 1, &[(log(&dir, "2024-03-03", "103", None), third_ms)])
    );
    assert!(!log(&dir, "2024-03-03", "103", None).exists());
    eprintln!(
        "Logger graph file observations: unlocked actual ToString; reentrant close/rotation captured paths; same-Thrown after registration; implicit Symbol rejection"
    );
}

pub(crate) fn observe_native_logger_host() {
    #[cfg(windows)]
    observe_win32_path_join();
    let temp = tempfile::Builder::new().prefix("ara-native-logger-host-").tempdir().unwrap();
    observe_rotation_and_audit(temp.path());
    observe_existing_files_and_faults(temp.path());
    observe_calendar_and_prune(temp.path());
    observe_native_pid_and_prune(temp.path());
    observe_graph_file_reentry(temp.path());
    let artifacts = temp.keep();
    eprintln!("Logger Host retained artifacts: {}", artifacts.display());
    eprintln!(
        "Logger Host coarse family: real Temp UTF8/EOL/rotation/audit/reopen; controlled failures/+08 month/prune order; native current/owned exited PID"
    );
}
