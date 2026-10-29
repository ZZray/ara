//! Supplemental transport/concurrency faults, separate from the four original tests.
use super::*;
use std::sync::{
    Barrier,
    atomic::{AtomicUsize, Ordering},
};

struct CountedSource {
    calls: AtomicUsize,
    fail_first: bool,
    held: Option<Arc<(Mutex<bool>, std::sync::Condvar)>>,
    entered: Option<std::sync::mpsc::Sender<()>>,
}
impl ReadByteSource for CountedSource {
    fn size(&self) -> u64 {
        1024
    }
    fn range(&self, start: u64, len: u64) -> Result<Vec<u8>> {
        self.range_classified(start, len)
            .map_err(ReadSourceFailure::flatten)
    }
    fn range_classified(
        &self,
        start: u64,
        len: u64,
    ) -> std::result::Result<Vec<u8>, ReadSourceFailure> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(sender) = &self.entered {
            sender.send(()).unwrap();
        }
        if let Some(gate) = &self.held {
            let mut released = gate.0.lock().unwrap();
            while !*released {
                released = gate.1.wait(released).unwrap();
            }
        }
        if self.fail_first && call == 0 {
            return Err(ReadSourceFailure::External(error("transport failed")));
        }
        Ok((start..start + len).map(|i| i as u8).collect())
    }
}
#[test]
fn cache_concurrent_readers_share_one_pending_block_and_retry_failure() {
    let held = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (completed_tx, completed_rx) = std::sync::mpsc::channel();
    let inner = Arc::new(CountedSource {
        calls: AtomicUsize::new(0),
        fail_first: false,
        held: Some(held.clone()),
        entered: Some(entered_tx),
    });
    let cached = caching_byte_source(Source::Read(inner.clone()), 256, 8).unwrap();
    let gate = Arc::new(Barrier::new(8));
    let workers: Vec<_> = (0..8)
        .map(|_| {
            let source = cached.clone();
            let gate = gate.clone();
            let completed = completed_tx.clone();
            std::thread::spawn(move || {
                gate.wait();
                let result = source.range(4, 26);
                completed.send(()).unwrap();
                assert_eq!(result.unwrap(), (4u8..30).collect::<Vec<_>>());
            })
        })
        .collect();
    drop(completed_tx);
    let entered = entered_rx
        .recv_timeout(std::time::Duration::from_secs(3))
        .is_ok();
    let pending = matches!(
        completed_rx.recv_timeout(std::time::Duration::from_millis(100)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    );
    let calls_while_held = inner.calls.load(Ordering::SeqCst);
    // Always release before asserting, so a failed observation cannot strand
    // a thread. The upstream data has not completed during the observation.
    *held.0.lock().unwrap() = true;
    held.1.notify_all();
    for worker in workers {
        worker.join().unwrap();
    }
    assert!(entered && pending, "held upstream did not remain in flight");
    assert_eq!(calls_while_held, 1);
    assert_eq!(inner.calls.load(Ordering::SeqCst), 1);
    let failed = Arc::new(CountedSource {
        calls: AtomicUsize::new(0),
        fail_first: true,
        held: None,
        entered: None,
    });
    let cached = caching_byte_source(Source::Read(failed.clone()), 256, 8).unwrap();
    let Source::Read(source) = cached else {
        panic!("cache source");
    };
    assert!(
        matches!(source.range_classified(0, 16), Err(ReadSourceFailure::External(ToolError(message))) if message == "transport failed")
    );
    assert_eq!(source.range(0, 16).unwrap(), (0u8..16).collect::<Vec<_>>());
    assert_eq!(failed.calls.load(Ordering::SeqCst), 2);
}

struct Body {
    bytes: std::io::Cursor<Vec<u8>>,
    read: Arc<AtomicUsize>,
    drops: Arc<AtomicUsize>,
}
impl Read for Body {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let read = self.bytes.read(bytes)?;
        self.read.fetch_add(read, Ordering::SeqCst);
        Ok(read)
    }
}
impl Drop for Body {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}
struct TestFetch {
    responses: Mutex<std::collections::VecDeque<HttpResponse>>,
    requests: Mutex<Vec<Vec<(String, String)>>>,
}
impl HttpFetch for TestFetch {
    fn fetch(&self, _: &str, headers: &[(String, String)]) -> Result<HttpResponse> {
        self.requests.lock().unwrap().push(headers.to_vec());
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| error("unexpected request"))
    }
}
fn response(
    status: u16,
    headers: Vec<(String, String)>,
    bytes: Vec<u8>,
    read: &Arc<AtomicUsize>,
    drops: &Arc<AtomicUsize>,
) -> HttpResponse {
    HttpResponse {
        status,
        headers,
        body: Box::new(Body {
            bytes: std::io::Cursor::new(bytes),
            read: read.clone(),
            drops: drops.clone(),
        }),
    }
}
#[test]
fn http_fallback_without_length_stops_at_cap_and_closes_body() {
    let read = Arc::new(AtomicUsize::new(0));
    let drops = Arc::new(AtomicUsize::new(0));
    let fetch = Arc::new(TestFetch {
        responses: Mutex::new([response(200, vec![], vec![0; 1024], &read, &drops)].into()),
        requests: Mutex::new(vec![]),
    });
    let result = http_byte_source(
        "http://fixture.invalid/",
        HttpByteSourceOptions {
            max_fallback_bytes: 8,
            fetch: Some(fetch),
            ..Default::default()
        },
    );
    assert!(
        matches!(result, Err(ToolError(message)) if message == "Remote archive is too large to buffer without range support (> 8 bytes)")
    );
    assert_eq!(read.load(Ordering::SeqCst), 9);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}
#[test]
fn http_probe_drop_headers_empty_and_range_failure_contracts() {
    let read = Arc::new(AtomicUsize::new(0));
    let drops = Arc::new(AtomicUsize::new(0));
    let fetch = Arc::new(TestFetch {
        responses: Mutex::new(
            [
                response(
                    206,
                    vec![("content-range".into(), "bytes 0-0/10".into())],
                    vec![1],
                    &read,
                    &drops,
                ),
                response(400, vec![], vec![0; 1000], &read, &drops),
            ]
            .into(),
        ),
        requests: Mutex::new(vec![]),
    });
    let source = http_byte_source(
        "http://fixture.invalid/",
        HttpByteSourceOptions {
            headers: vec![
                ("authorization".into(), "fixture-token".into()),
                ("Range".into(), "wrong".into()),
            ],
            fetch: Some(fetch.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(source.size(), 10);
    assert_eq!(read.load(Ordering::SeqCst), 0);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert_eq!(source.range(20, 0).unwrap(), Vec::<u8>::new());
    assert_eq!(fetch.requests.lock().unwrap().len(), 1);
    let cached = caching_byte_source(source.clone(), 256, 8).unwrap();
    assert_eq!(
        cached.range(20, 0).unwrap_err().0,
        "Invalid archive: truncated data"
    );
    assert_eq!(
        source.range(0, 2).unwrap_err().0,
        "Remote archive range request failed (HTTP 400)"
    );
    assert_eq!(read.load(Ordering::SeqCst), 0);
    assert_eq!(drops.load(Ordering::SeqCst), 2);
    let requests = fetch.requests.lock().unwrap();
    assert_eq!(
        requests[0],
        vec![
            ("authorization".into(), "fixture-token".into()),
            ("Range".into(), "wrong".into()),
            ("range".into(), "bytes=0-0".into())
        ]
    );
    assert_eq!(requests[1].last().unwrap().1, "bytes=0-1");
}
