use super::*;
use crate::process_output::CapturePair;
use std::{
    io,
    pin::Pin,
    task::{Context as TaskContext, Poll},
};
use tokio::io::ReadBuf;

struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("ara capture tests {}", crate::fresh_id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

struct BrokenReader(bool);
impl AsyncRead for BrokenReader {
    fn poll_read(mut self: Pin<&mut Self>, _: &mut TaskContext<'_>, buffer: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        if self.0 {
            return Poll::Ready(Err(io::Error::other("injected pipe failure")));
        }
        self.0 = true;
        buffer.put_slice(b"observed prefix\r\n");
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn pipe_error_retains_partial_archive_and_other_stream() {
    let files = Files::new();
    let capture = CapturePair::create(&files.0, "logs").unwrap();
    assert!(drain_captured(BrokenReader(false), 256, true, Some(capture.stdout())).await.is_err());
    let (bytes, cut) =
        drain_captured(b"other stream completed".as_slice(), 256, true, Some(capture.stderr())).await.unwrap();
    assert_eq!(bytes, b"other stream completed");
    assert!(!cut);
    assert!(capture.failed());
    let receipt = capture.receipt();
    assert_eq!(receipt["stdout"]["complete"], false);
    assert_eq!(receipt["stderr"]["complete"], true);
    assert_eq!(std::fs::read(files.0.join("logs/stdout.log")).unwrap(), b"observed prefix\r\n");
}

#[tokio::test]
async fn spawn_failure_retains_both_unstarted_artifact_references() {
    let files = Files::new();
    let capture = CapturePair::create(&files.0, "logs").unwrap();
    let error = run_process(
        Command::new(files.0.join("not-an-executable")),
        &files.0,
        CancellationToken::new(),
        Duration::from_secs(1),
        256,
        true,
        Some(capture),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("not_started") && error.contains("stdout.log") && error.contains("stderr.log"), "{error}");
    assert_eq!(std::fs::read(files.0.join("logs/stdout.log")).unwrap(), b"");
    assert_eq!(std::fs::read(files.0.join("logs/stderr.log")).unwrap(), b"");
}

#[tokio::test]
async fn canceled_before_process_never_creates_effects() {
    let files = Files::new();
    let capture = CapturePair::create(&files.0, "logs").unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let error =
        run_process(Command::new("not-used"), &files.0, cancel, Duration::from_secs(1), 256, true, Some(capture))
            .await
            .unwrap_err()
            .to_string();
    assert!(error.contains("not_started") && error.contains("stdout.log"), "{error}");
    assert_eq!(std::fs::read_dir(&files.0).unwrap().count(), 1);
}

#[test]
fn capture_option_rejects_null_and_invalid_types_before_execution() {
    assert_eq!(capture_argument(&json!({})).unwrap(), None);
    for bad in [Value::Null, json!(4), json!(false), json!(""), json!(" \t"), json!("bad\0path")] {
        assert!(capture_argument(&json!({"capture_dir":bad})).is_err());
    }
}
