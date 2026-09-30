use std::io;
use std::pin::Pin;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::task::{Context, Poll, Waker};

use ara_rpc::input::{InputItem, RpcInputReader};
use tokio::io::{AsyncRead, AsyncWriteExt, ReadBuf};
use tokio::sync::oneshot;

async fn parse_chunks(chunks: Vec<Vec<u8>>) -> Vec<InputItem> {
    let (mut writer, reader) = tokio::io::duplex(1024);
    let sender = tokio::spawn(async move {
        for chunk in chunks {
            writer.write_all(&chunk).await.unwrap();
        }
    });
    let mut input = RpcInputReader::new(reader);
    let mut items = Vec::new();
    while let Some(item) = input.read_next().await.unwrap() {
        items.push(item);
    }
    sender.await.unwrap();
    items
}

#[tokio::test]
async fn accepts_all_json_kinds_and_eof_tail() {
    let items = parse_chunks(vec![b"null\ntrue\n1\n\"text\"\n[]\n{}".to_vec()]).await;
    let values: Vec<_> = items
        .into_iter()
        .map(|item| match item {
            InputItem::Frame(value) => value.stringify(),
            InputItem::ParseError(error) => panic!("unexpected parse error: {error}"),
        })
        .collect();
    assert_eq!(values, ["null", "true", "1", "\"text\"", "[]", "{}"]);
}

#[tokio::test]
async fn malformed_line_reports_error_and_keeps_later_frames() {
    let items = parse_chunks(vec![b"bad\n{\"type\":\"get_state\",\"id\":\"ok\"}\n".to_vec()]).await;
    assert_eq!(items.len(), 2);
    assert!(matches!(&items[0], InputItem::ParseError(error) if error.starts_with("Failed to parse command: ")));
    assert!(matches!(&items[1], InputItem::Frame(value) if value.get("id").unwrap().stringify() == "\"ok\""));
}

#[tokio::test]
async fn fragmented_utf8_invalid_utf8_and_js_whitespace() {
    let emoji = "😀".as_bytes();
    let items = parse_chunks(vec![
        b"\xef\xbb\xbf \n{\"message\":\"".to_vec(),
        emoji[..1].to_vec(),
        emoji[1..3].to_vec(),
        [emoji[3..].to_vec(), b"\"}\n".to_vec()].concat(),
        [b"{\"message\":\"".to_vec(), vec![0xff], b"\"}\n".to_vec()].concat(),
        "\u{FEFF}\u{2028} {\"type\":\"get_state\"} \u{2029}".as_bytes().to_vec(),
    ])
    .await;
    assert_eq!(items.len(), 3);
    assert!(matches!(&items[0], InputItem::Frame(value) if value.get("message").unwrap().stringify() == "\"😀\""));
    assert!(matches!(&items[1], InputItem::Frame(value) if value.get("message").unwrap().stringify() == "\"�\""));
    assert!(matches!(&items[2], InputItem::Frame(value) if value.get("type").unwrap().stringify() == "\"get_state\""));
}

#[tokio::test]
async fn preserves_lone_surrogate_and_has_no_inbound_frame_limit() {
    let large = format!("{{\"type\":\"prompt\",\"message\":\"{}\"}}\n", "x".repeat(1024 * 1024 + 1));
    let items = parse_chunks(vec![b"{\"message\":\"\\ud800\"}\n".to_vec(), large.into_bytes()]).await;
    assert_eq!(items.len(), 2);
    assert!(matches!(&items[0], InputItem::Frame(value) if value.get("message").unwrap().stringify() == "\"\\ud800\""));
    assert!(
        matches!(&items[1], InputItem::Frame(value) if value.get("message").unwrap().as_string().unwrap().len() == 1024 * 1024 + 1)
    );
}

struct InterruptedRead {
    phase: u8,
    ready: Arc<AtomicBool>,
    parked: Arc<Mutex<Option<Waker>>>,
    waiting: Option<oneshot::Sender<()>>,
}

impl AsyncRead for InterruptedRead {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        match self.phase {
            0 => {
                buf.put_slice(b"{\"type\":\"get_");
                self.phase = 1;
                Poll::Ready(Ok(()))
            }
            1 if !self.ready.load(Ordering::Acquire) => {
                if let Some(signal) = self.waiting.take() {
                    let _ = signal.send(());
                }
                *self.parked.lock().unwrap() = Some(cx.waker().clone());
                Poll::Pending
            }
            1 => {
                buf.put_slice(b"state\"}\n");
                self.phase = 2;
                Poll::Ready(Ok(()))
            }
            _ => Poll::Ready(Ok(())),
        }
    }
}

#[tokio::test]
async fn cancelled_read_keeps_partial_line() {
    let ready = Arc::new(AtomicBool::new(false));
    let parked = Arc::new(Mutex::new(None::<Waker>));
    let (signal, mut waiting) = oneshot::channel();
    let source =
        InterruptedRead { phase: 0, ready: Arc::clone(&ready), parked: Arc::clone(&parked), waiting: Some(signal) };
    let mut input = RpcInputReader::new(source);
    {
        let future = input.read_next();
        tokio::pin!(future);
        tokio::select! {
            result = &mut future => panic!("unexpected complete line: {result:?}"),
            result = &mut waiting => result.unwrap(),
        }
    }
    ready.store(true, Ordering::Release);
    parked.lock().unwrap().take().unwrap().wake();
    assert!(
        matches!(input.read_next().await.unwrap(), Some(InputItem::Frame(value)) if value.get("type").unwrap().stringify() == "\"get_state\"")
    );
    assert!(input.read_next().await.unwrap().is_none());
}
