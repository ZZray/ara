use std::io;
use std::pin::Pin;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::task::{Context, Poll, Waker};

use ara_rpc::WireValue;
use ara_rpc::writer::RpcOutput;
use tokio::io::{AsyncReadExt, AsyncWrite};
use tokio::sync::oneshot;

#[tokio::test]
async fn writes_ordered_physical_lines_and_returns_completion_receipts() {
    let (writer, mut reader) = tokio::io::duplex(8192);
    let task = tokio::spawn(async move {
        let mut output = RpcOutput::new(writer);
        let ready = WireValue::parse(r#"{"type":"ready","protocolVersion":1}"#).unwrap();
        let first = output.write_frame(&ready).await.unwrap();
        output.set_protocol_version(2).unwrap();
        let large = WireValue::object(vec![
            ("type", WireValue::String("response".into())),
            ("payload", WireValue::String("x".repeat(1024 * 1024).into())),
        ]);
        let second = output.write_frame(&large).await.unwrap();
        (first, second)
    });
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).await.unwrap();
    let (first, second) = task.await.unwrap();
    assert_eq!(first.physical_frames, 1);
    assert!(second.physical_frames > 1);
    assert_eq!(first.bytes_written + second.bytes_written, bytes.len());
    let lines: Vec<_> = bytes.split_inclusive(|&byte| byte == b'\n').collect();
    assert_eq!(lines.len(), first.physical_frames + second.physical_frames);
    assert!(lines.iter().all(|line| line.last() == Some(&b'\n') && line.len() <= 1024 * 1024));
    assert_eq!(
        WireValue::parse(std::str::from_utf8(lines[0]).unwrap()).unwrap().get("type").unwrap().stringify(),
        "\"ready\""
    );
    assert!(
        lines[1..].iter().all(|line| WireValue::parse(std::str::from_utf8(line).unwrap())
            .unwrap()
            .get("type")
            .unwrap()
            .stringify()
            == "\"rpc_chunk\"")
    );
}

struct GatedWriter {
    ready: Arc<AtomicBool>,
    parked: Arc<Mutex<Option<Waker>>>,
    first_poll: Option<oneshot::Sender<()>>,
    bytes: Vec<u8>,
}

impl AsyncWrite for GatedWriter {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, bytes: &[u8]) -> Poll<io::Result<usize>> {
        if !self.ready.load(Ordering::Acquire) {
            if let Some(signal) = self.first_poll.take() {
                let _ = signal.send(());
            }
            *self.parked.lock().unwrap() = Some(cx.waker().clone());
            return Poll::Pending;
        }
        self.bytes.extend_from_slice(bytes);
        Poll::Ready(Ok(bytes.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn receipt_waits_for_write_backpressure() {
    let ready = Arc::new(AtomicBool::new(false));
    let parked = Arc::new(Mutex::new(None::<Waker>));
    let (tx, rx) = oneshot::channel();
    let writer =
        GatedWriter { ready: Arc::clone(&ready), parked: Arc::clone(&parked), first_poll: Some(tx), bytes: Vec::new() };
    let frame = WireValue::parse(r#"{"type":"ready"}"#).unwrap();
    let task = tokio::spawn(async move {
        let mut output = RpcOutput::new(writer);
        let receipt = output.write_frame(&frame).await.unwrap();
        (receipt, output.into_inner().bytes)
    });
    rx.await.unwrap();
    assert!(!task.is_finished(), "write_frame must not acknowledge enqueue alone");
    ready.store(true, Ordering::Release);
    parked.lock().unwrap().take().unwrap().wake();
    let (receipt, bytes) = task.await.unwrap();
    assert_eq!(receipt.physical_frames, 1);
    assert_eq!(receipt.bytes_written, bytes.len());
    assert_eq!(bytes, b"{\"type\":\"ready\"}\n");
}

struct FailFlush(Arc<AtomicUsize>);

impl AsyncWrite for FailFlush {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, bytes: &[u8]) -> Poll<io::Result<usize>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Poll::Ready(Ok(bytes.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, "peer closed")))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn failed_flush_has_no_receipt_and_stops_later_frames() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut output = RpcOutput::new(FailFlush(Arc::clone(&calls)));
    let frame = WireValue::parse(r#"{"type":"ready"}"#).unwrap();
    assert_eq!(output.write_frame(&frame).await.unwrap_err().kind(), io::ErrorKind::BrokenPipe);
    let after_first = calls.load(Ordering::SeqCst);
    assert_eq!(output.write_frame(&frame).await.unwrap_err().kind(), io::ErrorKind::BrokenPipe);
    assert_eq!(calls.load(Ordering::SeqCst), after_first);
}

struct PartialThenPending {
    polls: Arc<AtomicUsize>,
    pending: Option<oneshot::Sender<()>>,
    accepted: Vec<u8>,
}

impl AsyncWrite for PartialThenPending {
    fn poll_write(mut self: Pin<&mut Self>, _: &mut Context<'_>, bytes: &[u8]) -> Poll<io::Result<usize>> {
        if self.polls.fetch_add(1, Ordering::SeqCst) == 0 {
            let n = bytes.len().min(7);
            self.accepted.extend_from_slice(&bytes[..n]);
            return Poll::Ready(Ok(n));
        }
        if let Some(signal) = self.pending.take() {
            let _ = signal.send(());
        }
        Poll::Pending
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn cancelled_partial_frame_poisoned_before_next_frame() {
    let polls = Arc::new(AtomicUsize::new(0));
    let (signal, mut pending) = oneshot::channel();
    let writer = PartialThenPending { polls: Arc::clone(&polls), pending: Some(signal), accepted: Vec::new() };
    let mut output = RpcOutput::new(writer);
    let frame = WireValue::parse(r#"{"type":"ready"}"#).unwrap();
    {
        let future = output.write_frame(&frame);
        tokio::pin!(future);
        tokio::select! {
            result = &mut future => panic!("unexpected write completion: {result:?}"),
            result = &mut pending => result.unwrap(),
        }
    }
    let prior_polls = polls.load(Ordering::SeqCst);
    assert!(prior_polls >= 2);
    assert_eq!(output.write_frame(&frame).await.unwrap_err().kind(), io::ErrorKind::BrokenPipe);
    assert_eq!(polls.load(Ordering::SeqCst), prior_polls);
    assert_eq!(output.into_inner().accepted.len(), 7);
}
