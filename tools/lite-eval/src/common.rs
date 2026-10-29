//! Evidence conventions shared by every lite-eval component (design “公共约定”):
//! lowercase SHA-256 hex, second-precision ISO 8601 timestamps with an offset,
//! and evidence writes that never overwrite a file and never carry a known
//! credential value.

use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, ErrorKind, Write},
    path::{Path, PathBuf},
};

use anyhow::{bail, ensure, Context, Result};
use chrono::{DateTime, FixedOffset, Local, TimeZone, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};

/// Default out-of-repo evidence root for raw outputs (design DD-15).
pub const EVIDENCE_ROOT: &str = "D:/WorkSpace/tmp/ara-lite-capability";

/// Lowercase 64-character hexadecimal SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// [`sha256_hex`] of a file's bytes, streamed.
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("cannot open {} for hashing", path.display()))?;
    let mut digest = Sha256::new();
    io::copy(&mut file, &mut digest).with_context(|| format!("cannot read {} for hashing", path.display()))?;
    Ok(format!("{:x}", digest.finalize()))
}

const TIMESTAMP_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%:z";

/// Current local time, for example `2026-10-27T09:15:02+08:00`.
pub fn timestamp_now() -> String {
    format_timestamp(&Local::now())
}

/// ISO 8601 with whole seconds and a `±HH:MM` offset; sub-seconds are dropped.
pub fn format_timestamp<Tz: TimeZone>(time: &DateTime<Tz>) -> String
where
    Tz::Offset: fmt::Display,
{
    time.format(TIMESTAMP_FORMAT).to_string()
}

/// Accepts exactly the form written by [`format_timestamp`]: no fractional
/// seconds, no `Z`, and an offset with a colon.
pub fn parse_timestamp(text: &str) -> Result<DateTime<FixedOffset>> {
    let parsed = DateTime::parse_from_str(text, TIMESTAMP_FORMAT).ok().filter(|time| format_timestamp(time) == text);
    parsed.with_context(|| format!("`{text}` is not an ISO 8601 timestamp with whole seconds and a ±HH:MM offset"))
}

/// Known credential values, for example a model key read once from the
/// environment variable named by the Run_Policy. Evidence is checked against
/// them verbatim before it reaches disk. `Debug` never prints the values.
#[derive(Clone, Default)]
pub struct Credentials {
    values: Vec<String>,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Credentials").field("count", &self.values.len()).finish()
    }
}

impl Credentials {
    pub const REDACTED: &'static str = "[REDACTED_CREDENTIAL]";

    /// Empty values are dropped: an unset credential has nothing to leak.
    pub fn new(values: impl IntoIterator<Item = String>) -> Self {
        Self { values: values.into_iter().filter(|value| !value.is_empty()).collect() }
    }

    /// Whether any known value occurs verbatim in `bytes`.
    pub fn occurs_in(&self, bytes: &[u8]) -> bool {
        self.values.iter().any(|value| bytes.windows(value.len()).any(|window| window == value.as_bytes()))
    }

    /// Replace every known value, for text such as provider error messages
    /// that must be kept as evidence.
    pub fn redact(&self, text: &str) -> String {
        self.values.iter().fold(text.to_owned(), |text, value| text.replace(value.as_str(), Self::REDACTED))
    }
}

/// Write a new evidence file. Refuses to overwrite an existing path and to
/// write bytes that contain a known credential value (redact them first).
pub fn write_new(path: &Path, bytes: &[u8], credentials: &Credentials) -> Result<()> {
    ensure!(
        !credentials.occurs_in(bytes),
        "refusing to write evidence {}: it contains a known credential value",
        path.display()
    );
    let mut file = OpenOptions::new().write(true).create_new(true).open(path).map_err(|error| {
        if error.kind() == ErrorKind::AlreadyExists {
            anyhow::anyhow!("evidence {} already exists; reruns must write a new directory", path.display())
        } else {
            anyhow::Error::new(error).context(format!("cannot create evidence {}", path.display()))
        }
    })?;
    file.write_all(bytes).with_context(|| format!("cannot write evidence {}", path.display()))?;
    file.sync_all().with_context(|| format!("cannot sync evidence {}", path.display()))
}

/// Keep process output byte for byte (requirement 33.3) unless it carries a
/// known credential; only then is it decoded and redacted.
pub fn write_output_new(path: &Path, bytes: &[u8], credentials: &Credentials) -> Result<()> {
    if credentials.occurs_in(bytes) {
        write_new(path, credentials.redact(&String::from_utf8_lossy(bytes)).as_bytes(), credentials)
    } else {
        write_new(path, bytes, credentials)
    }
}

/// [`write_new`] for one pretty-printed JSON document followed by a newline.
pub fn write_json_new(path: &Path, value: &impl Serialize, credentials: &Credentials) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value).with_context(|| format!("cannot encode {}", path.display()))?;
    bytes.push(b'\n');
    write_new(path, &bytes, credentials)
}

/// Result of [`run_bounded`].
#[derive(Debug)]
pub enum Bounded {
    Exited {
        code: Option<i32>,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    /// Killed at the deadline. Its output is not collected: a grandchild
    /// holding the pipes open could otherwise block the caller indefinitely.
    TimedOut,
}

/// Run `command` with stdin closed and both pipes drained, killing it at
/// `timeout`. Only the direct child is killed; processes it started may
/// outlive it.
pub fn run_bounded(mut command: std::process::Command, timeout: std::time::Duration) -> Result<Bounded> {
    use std::{io::Read, process::Stdio, thread, time::Instant};
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("cannot start {:?}", command.get_program()))?;
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            bytes
        })
    };
    let stdout = drain(child.stdout.take().map(|pipe| Box::new(pipe) as Box<dyn Read + Send>));
    let stderr = drain(child.stderr.take().map(|pipe| Box::new(pipe) as Box<dyn Read + Send>));
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait().context("cannot poll child process")? {
            break Some(status);
        }
        if Instant::now() >= deadline {
            break None;
        }
        thread::sleep(std::time::Duration::from_millis(50));
    };
    let Some(status) = status else {
        let _ = child.kill();
        let _ = child.wait();
        return Ok(Bounded::TimedOut);
    };
    Ok(Bounded::Exited {
        code: status.code(),
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

/// Create `<parent>/<UTC yyyymmddThhmmssZ>-<nnnn>` for one run. Existing names
/// are never reused, so a rerun always writes to its own new directory.
pub fn new_run_dir(parent: &Path) -> Result<PathBuf> {
    fs::create_dir_all(parent).with_context(|| format!("cannot create evidence root {}", parent.display()))?;
    let stamp = Utc::now().format("%Y%m%dT%H%M%SZ");
    for sequence in 1..=9999u32 {
        let candidate = parent.join(format!("{stamp}-{sequence:04}"));
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("cannot create run directory {}", candidate.display()))
            }
        }
    }
    bail!("no unused run directory name left under {} for {stamp}", parent.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_is_lowercase_hex_of_known_vectors() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        let digest = sha256_hex(b"header\0\xE4\xB8\xAD");
        assert_eq!(digest.len(), 64);
        assert!(digest.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
    }

    #[test]
    fn file_hash_matches_byte_hash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blob.bin");
        let bytes: Vec<u8> = (0..200_000u32).map(|n| (n % 251) as u8).collect();
        fs::write(&path, &bytes).unwrap();
        assert_eq!(sha256_file(&path).unwrap(), sha256_hex(&bytes));
        assert!(sha256_file(&dir.path().join("missing")).is_err());
    }

    #[test]
    fn timestamps_have_whole_seconds_and_an_offset() {
        let east = FixedOffset::east_opt(8 * 3600).unwrap();
        let time = east.with_ymd_and_hms(2026, 10, 27, 9, 15, 2).unwrap() + chrono::Duration::milliseconds(750);
        assert_eq!(format_timestamp(&time), "2026-10-27T09:15:02+08:00");
        let west = FixedOffset::west_opt(5 * 3600 + 30 * 60).unwrap();
        assert_eq!(format_timestamp(&west.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap()), "2026-01-02T03:04:05-05:30");
        assert_eq!(format_timestamp(&Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap()), "2026-01-02T03:04:05+00:00");

        let now = timestamp_now();
        assert_eq!(now.len(), "2026-10-27T09:15:02+08:00".len());
        assert_eq!(format_timestamp(&parse_timestamp(&now).unwrap()), now);
    }

    #[test]
    fn timestamp_parsing_rejects_other_iso_forms() {
        assert_eq!(parse_timestamp("2026-10-27T09:15:02+08:00").unwrap().offset().local_minus_utc(), 8 * 3600);
        for text in [
            "2026-10-27T09:15:02Z",
            "2026-10-27T09:15:02.250+08:00",
            "2026-10-27T09:15:02",
            "2026-10-27T09:15:02+0800",
            "2026-10-27 09:15:02+08:00",
            "2026-10-27T09:15+08:00",
        ] {
            assert!(parse_timestamp(text).is_err(), "accepted {text}");
        }
    }

    #[test]
    fn write_new_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.json");
        let none = Credentials::default();
        write_new(&path, b"first\n", &none).unwrap();
        let error = write_new(&path, b"second\n", &none).unwrap_err().to_string();
        assert!(error.contains("already exists"), "{error}");
        assert_eq!(fs::read(&path).unwrap(), b"first\n");

        write_json_new(&dir.path().join("doc.json"), &serde_json::json!({"a": 1}), &none).unwrap();
        assert_eq!(fs::read_to_string(dir.path().join("doc.json")).unwrap(), "{\n  \"a\": 1\n}\n");
        assert!(write_json_new(&dir.path().join("doc.json"), &serde_json::json!({}), &none).is_err());
    }

    #[test]
    fn credential_values_are_detected_verbatim_and_never_written() {
        let secret = "sk-test-0123456789abcdef";
        let credentials = Credentials::new([secret.to_owned(), String::new()]);
        assert!(!format!("{credentials:?}").contains(secret));
        assert!(credentials.occurs_in(format!("Authorization: Bearer {secret}\n").as_bytes()));
        assert!(credentials.occurs_in(secret.as_bytes()));
        assert!(!credentials.occurs_in(b"sk-test-0123456789abcde"));
        assert!(!credentials.occurs_in(b""));
        // An empty value is dropped instead of matching everything.
        assert!(!Credentials::new([String::new()]).occurs_in(b"anything"));

        let dir = tempfile::tempdir().unwrap();
        let leaked = dir.path().join("leaked.json");
        let error = write_json_new(&leaked, &serde_json::json!({"error": format!("401 for {secret}")}), &credentials)
            .unwrap_err()
            .to_string();
        assert!(error.contains("known credential value"), "{error}");
        assert!(!error.contains(secret));
        assert!(!leaked.exists());

        let redacted = credentials.redact(&format!("401 for {secret}; retry {secret}"));
        assert_eq!(redacted, "401 for [REDACTED_CREDENTIAL]; retry [REDACTED_CREDENTIAL]");
        write_new(&leaked, redacted.as_bytes(), &credentials).unwrap();
        assert!(!credentials.occurs_in(&fs::read(&leaked).unwrap()));
    }

    #[test]
    fn reruns_get_new_directories() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("runs");
        let first = new_run_dir(&root).unwrap();
        fs::write(first.join("manifest.json"), b"{}").unwrap();
        let second = new_run_dir(&root).unwrap();
        assert_ne!(first, second);
        assert!(first.is_dir() && second.is_dir());
        assert_eq!(fs::read(first.join("manifest.json")).unwrap(), b"{}");
        let name = second.file_name().unwrap().to_str().unwrap();
        assert_eq!(name.len(), "20261028T021500Z-0001".len(), "{name}");
    }
}
