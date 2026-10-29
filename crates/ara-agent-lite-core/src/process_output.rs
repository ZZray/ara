//! Optional raw process-output retention. A receipt describes the files at receipt time.
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub(crate) const CAPTURE_LIMIT: u64 = 8 * 1024 * 1024;
pub(crate) type SinkHandle = Arc<Mutex<Sink>>;

#[derive(Clone)]
pub(crate) struct CapturePair {
    directory: PathBuf,
    stdout: SinkHandle,
    stderr: SinkHandle,
}

impl CapturePair {
    pub(crate) fn create(cwd: &Path, raw: &str) -> Result<Self> {
        if raw.trim().is_empty() || raw.contains('\0') || raw.len() > 4096 {
            bail!("capture_dir must be a nonempty path without NUL bytes, at most 4096 UTF-8 bytes");
        }
        let supplied = Path::new(raw);
        let requested = if supplied.is_absolute() { supplied.to_path_buf() } else { cwd.join(supplied) };
        let name = requested.file_name().context("capture_dir must name a new directory")?;
        let parent = requested
            .parent()
            .context("capture_dir must have an existing parent")?
            .canonicalize()
            .context("resolve capture_dir parent")?;
        let directory = parent.join(name);
        // Bound JSON escaping as well as path bytes, so receipts fit below 64 KiB.
        if directory.to_string_lossy().len() > 8192 || serde_json::to_string(&directory)?.len() > 16_384 {
            bail!("resolved capture_dir exceeds the receipt path budget");
        }
        #[cfg(unix)]
        let mut builder = fs::DirBuilder::new();
        #[cfg(not(unix))]
        let builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&directory).with_context(|| {
            format!("create new capture directory {} (existing paths are never reused)", directory.display())
        })?;
        // On failure leave only this newly-created directory/partial files for inspection.
        let stdout = Arc::new(Mutex::new(Sink::create(directory.join("stdout.log"))?));
        let stderr = Arc::new(Mutex::new(Sink::create(directory.join("stderr.log"))?));
        Ok(Self { directory, stdout, stderr })
    }

    pub(crate) fn stdout(&self) -> SinkHandle {
        Arc::clone(&self.stdout)
    }
    pub(crate) fn stderr(&self) -> SinkHandle {
        Arc::clone(&self.stderr)
    }

    pub(crate) fn receipt(&self) -> Value {
        let stdout = self.stdout.lock().unwrap_or_else(|p| p.into_inner()).receipt();
        let stderr = self.stderr.lock().unwrap_or_else(|p| p.into_inner()).receipt();
        json!({
            "schema": "ara.process-output.capture.v1", "directory": self.directory,
            "limit_bytes_per_stream": CAPTURE_LIMIT, "encoding": "raw-bytes",
            "complete": stdout["complete"] == true && stderr["complete"] == true,
            "stdout": stdout, "stderr": stderr,
        })
    }

    pub(crate) fn failed(&self) -> bool {
        self.stdout.lock().unwrap_or_else(|p| p.into_inner()).error.is_some()
            || self.stderr.lock().unwrap_or_else(|p| p.into_inner()).error.is_some()
    }
}

pub(crate) struct Sink {
    path: PathBuf,
    writer: Box<dyn Write + Send>,
    observed: u64,
    stored: u64,
    digest: Sha256,
    reached_eof: bool,
    quota_exceeded: bool,
    error: Option<String>,
}

impl Sink {
    fn create(path: PathBuf) -> Result<Self> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path).with_context(|| format!("create raw output {}", path.display()))?;
        Ok(Self::new(path, Box::new(file)))
    }

    fn new(path: PathBuf, writer: Box<dyn Write + Send>) -> Self {
        Self {
            path,
            writer,
            observed: 0,
            stored: 0,
            digest: Sha256::new(),
            reached_eof: false,
            quota_exceeded: false,
            error: None,
        }
    }

    pub(crate) fn record(&mut self, bytes: &[u8]) {
        let before = self.observed;
        self.observed = self.observed.saturating_add(bytes.len() as u64);
        self.quota_exceeded |= self.observed > CAPTURE_LIMIT;
        if self.error.is_some() {
            return;
        }
        let retained = bytes.len().min(CAPTURE_LIMIT.saturating_sub(before) as usize);
        let mut remaining = &bytes[..retained];
        while !remaining.is_empty() {
            match self.writer.write(remaining) {
                Ok(0) => {
                    self.fail("write", &io::Error::from(io::ErrorKind::WriteZero));
                    break;
                }
                Ok(count) => {
                    self.stored += count as u64;
                    self.digest.update(&remaining[..count]);
                    remaining = &remaining[count..];
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    self.fail("write", &error);
                    break;
                }
            }
        }
    }

    pub(crate) fn eof(&mut self) {
        self.reached_eof = true;
    }
    pub(crate) fn read_error(&mut self, error: &io::Error) {
        self.fail("read", error);
    }

    fn fail(&mut self, operation: &str, error: &io::Error) {
        if self.error.is_none() {
            let mut message = format!("{operation}: {error}");
            let mut end = message.len().min(512);
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            message.truncate(end);
            self.error = Some(message);
        }
    }

    fn receipt(&mut self) -> Value {
        if let Err(error) = self.writer.flush() {
            self.fail("flush", &error);
        }
        let expected = self.digest.clone().finalize();
        let verified = (|| -> io::Result<String> {
            let file = File::open(&self.path)?;
            if file.metadata()?.len() != self.stored {
                return Err(io::Error::other("stored output length changed"));
            }
            let mut contents = Vec::new();
            file.take(CAPTURE_LIMIT + 1).read_to_end(&mut contents)?;
            let actual = Sha256::digest(&contents);
            if contents.len() as u64 != self.stored || actual != expected {
                return Err(io::Error::other("stored output content changed"));
            }
            Ok(format!("{actual:x}"))
        })();
        let sha256 = match verified {
            Ok(hash) => Some(hash),
            Err(error) => {
                self.fail("verify", &error);
                None
            }
        };
        json!({
            "path": self.path, "stored_bytes": self.stored, "observed_bytes": self.observed,
            "sha256": sha256, "eof": self.reached_eof, "quota_exceeded": self.quota_exceeded,
            "error": self.error, "complete": self.reached_eof && !self.quota_exceeded && self.error.is_none(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("ara-capture-{}", crate::fresh_id()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn finish(pair: &CapturePair) -> Value {
        pair.stdout().lock().unwrap().eof();
        pair.stderr().lock().unwrap().eof();
        pair.receipt()
    }

    #[test]
    fn raw_bytes_and_middle_output_remain_recoverable() {
        let temp = Temp::new();
        let pair = CapturePair::create(&temp.0, "capture").unwrap();
        let mut data = vec![b'x'; 160_000];
        let marker = b"\xff\x00\r\n\xe4\xb8\xad\xe6\x96\x87\nMIDDLE";
        data[80_000..80_000 + marker.len()].copy_from_slice(marker);
        for chunk in data.chunks(997) {
            pair.stdout().lock().unwrap().record(chunk);
        }
        let receipt = finish(&pair);
        assert_eq!(fs::read(temp.0.join("capture/stdout.log")).unwrap(), data);
        assert_eq!(receipt["stdout"]["sha256"], format!("{:x}", Sha256::digest(&data)));
        assert_eq!(receipt["stdout"]["stored_bytes"], data.len() as u64);
        assert_eq!(receipt["complete"], true);
        assert!(!pair.failed());
    }

    #[test]
    fn capture_never_reuses_existing_paths() {
        let temp = Temp::new();
        fs::create_dir(temp.0.join("existing")).unwrap();
        fs::write(temp.0.join("file"), b"keep").unwrap();
        assert!(CapturePair::create(&temp.0, "existing").is_err());
        assert!(CapturePair::create(&temp.0, "file").is_err());
        assert!(CapturePair::create(&temp.0, "missing/child").is_err());
        assert!(CapturePair::create(&temp.0, "").is_err());
        assert!(CapturePair::create(&temp.0, "bad\0path").is_err());
        assert!(CapturePair::create(&temp.0, &"x".repeat(4097)).is_err());
        assert_eq!(fs::read(temp.0.join("file")).unwrap(), b"keep");
        assert!(!temp.0.join("existing/stdout.log").exists());
    }

    #[test]
    fn quota_keeps_draining_and_reports_a_prefix() {
        let temp = Temp::new();
        let pair = CapturePair::create(&temp.0, "capture").unwrap();
        let block = vec![b'q'; 65_536];
        for _ in 0..129 {
            pair.stdout().lock().unwrap().record(&block);
        }
        let receipt = finish(&pair);
        assert_eq!(receipt["stdout"]["stored_bytes"], CAPTURE_LIMIT);
        assert_eq!(receipt["stdout"]["observed_bytes"], 129 * 65_536);
        assert_eq!(receipt["stdout"]["quota_exceeded"], true);
        assert_eq!(receipt["complete"], false);
        assert_eq!(fs::metadata(temp.0.join("capture/stdout.log")).unwrap().len(), CAPTURE_LIMIT);
        assert!(!pair.failed());
    }

    struct FailingWriter {
        file: File,
        remaining: usize,
    }
    impl Write for FailingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.remaining == 0 {
                return Err(io::Error::other("injected disk failure"));
            }
            let count = self.file.write(&bytes[..bytes.len().min(self.remaining)])?;
            self.remaining -= count;
            Ok(count)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.file.flush()
        }
    }

    #[test]
    fn partial_write_counts_only_persisted_prefix_and_continues_observing() {
        let temp = Temp::new();
        let path = temp.0.join("partial.log");
        let writer = FailingWriter { file: File::create(&path).unwrap(), remaining: 3 };
        let mut sink = Sink::new(path.clone(), Box::new(writer));
        sink.record(b"abcdef");
        sink.record(b"later");
        sink.eof();
        let receipt = sink.receipt();
        assert_eq!(fs::read(path).unwrap(), b"abc");
        assert_eq!(receipt["stored_bytes"], 3);
        assert_eq!(receipt["observed_bytes"], 11);
        assert_eq!(receipt["sha256"], format!("{:x}", Sha256::digest(b"abc")));
        assert!(receipt["error"].as_str().unwrap().contains("injected disk failure"));
        assert_eq!(receipt["complete"], false);
    }

    #[test]
    fn read_failure_and_changed_file_are_never_complete() {
        let temp = Temp::new();
        let pair = CapturePair::create(&temp.0, "capture").unwrap();
        pair.stdout().lock().unwrap().record(b"abc");
        pair.stderr().lock().unwrap().read_error(&io::Error::other("pipe failed"));
        fs::write(temp.0.join("capture/stdout.log"), b"xyz").unwrap();
        let receipt = finish(&pair);
        assert_eq!(receipt["stdout"]["sha256"], Value::Null);
        assert_eq!(receipt["complete"], false);
        assert!(pair.failed());
    }

    #[cfg(unix)]
    #[test]
    fn capture_permissions_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let temp = Temp::new();
        let _pair = CapturePair::create(&temp.0, "capture").unwrap();
        assert_eq!(fs::metadata(temp.0.join("capture")).unwrap().permissions().mode() & 0o077, 0);
        assert_eq!(fs::metadata(temp.0.join("capture/stdout.log")).unwrap().permissions().mode() & 0o177, 0);
    }
}
