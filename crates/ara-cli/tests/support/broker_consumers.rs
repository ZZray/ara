//! Path-routed Broker fixture for actual CLI/RPC child consumers. The retained
//! event stream is independent of JSON request order and ends with the owner.

use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub body: Value,
}

pub struct BrokerFixture {
    pub url: String,
    pub requests: Arc<Mutex<Vec<Request>>>,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl BrokerFixture {
    pub async fn start(handler: impl Fn(&Request) -> (u16, Value) + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let cancel = CancellationToken::new();
        let (seen, stop) = (requests.clone(), cancel.clone());
        let handler = Arc::new(handler);
        let task = tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    biased;
                    _ = stop.cancelled() => break,
                    accepted = listener.accept() => accepted,
                };
                let Ok((mut socket, _)) = accepted else { break };
                let (seen, stop, handler) = (seen.clone(), stop.clone(), handler.clone());
                tokio::spawn(async move {
                    let Some(request) = read_request(&mut socket, &stop).await else { return };
                    seen.lock().unwrap().push(request.clone());
                    if request.method == "GET" && request.path.starts_with("/v1/events") {
                        let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n: fixture stream\n\n";
                        if socket.write_all(head.as_bytes()).await.is_ok() {
                            stop.cancelled().await;
                        }
                        return;
                    }
                    let (status, body) = handler(&request);
                    let body = body.to_string();
                    let wire = format!(
                        "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(wire.as_bytes()).await;
                });
            }
        });
        Self { url, requests, cancel, task }
    }
}

impl Drop for BrokerFixture {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.task.abort();
    }
}

async fn read_request(socket: &mut TcpStream, stop: &CancellationToken) -> Option<Request> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 2048];
    let (header_end, length) = loop {
        let count = tokio::select! {
            biased;
            _ = stop.cancelled() => return None,
            read = socket.read(&mut buffer) => read.ok()?,
        };
        if count == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&bytes[..end]);
            let length = headers
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                .unwrap_or(0);
            break (end + 4, length);
        }
        assert!(bytes.len() < 65_536, "fixture request headers are bounded");
    };
    while bytes.len() < header_end + length {
        let count = tokio::select! {
            biased;
            _ = stop.cancelled() => return None,
            read = socket.read(&mut buffer) => read.ok()?,
        };
        if count == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    let head = String::from_utf8_lossy(&bytes[..header_end]);
    let mut words = head.lines().next()?.split_whitespace();
    Some(Request {
        method: words.next()?.to_owned(),
        path: words.next()?.to_owned(),
        body: if length == 0 {
            Value::Null
        } else {
            serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap()
        },
    })
}

pub fn snapshot(generation: i64, entries: Vec<Value>) -> Value {
    let now = chrono::Utc::now().timestamp_millis();
    json!({"generation":generation,"generatedAt":now,"serverNowMs":now,
        "refresher":{"enabled":false,"intervalMs":0,"skewMs":0,"nextSweepInMs":9007199254740991_u64},
        "credentials":entries})
}
