//! Fixed source.test.ts four original assertions, using real HTTP loopback.
//! MIT upstream notices apply. No HTTP mock stands for native networking.
//! Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;
use std::io::Write;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
struct Server {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    ranged: Arc<AtomicUsize>,
    full: Arc<AtomicUsize>,
}
impl Server {
    fn start(bytes: Vec<u8>, range_support: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let ranged = Arc::new(AtomicUsize::new(0));
        let full = Arc::new(AtomicUsize::new(0));
        let (flag, ranges, fulls) = (stop.clone(), ranged.clone(), full.clone());
        let thread = std::thread::spawn(move || {
            for stream in listener.incoming() {
                if flag.load(Ordering::Acquire) {
                    break;
                }
                let Ok(mut stream) = stream else {
                    break;
                };
                stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
                let mut request = Vec::new();
                let mut buffer = [0u8; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") && request.len() < 32768 {
                    match stream.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => request.extend_from_slice(&buffer[..n]),
                    }
                }
                let text = String::from_utf8_lossy(&request);
                let range = text.lines().find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    (key.eq_ignore_ascii_case("range")).then(|| value.trim())
                });
                let parsed = range
                    .and_then(|r| r.strip_prefix("bytes="))
                    .and_then(|r| r.split_once('-'))
                    .and_then(|(a, b)| Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok()?)));
                let (status, body, content_range) = if range_support {
                    match parsed {
                        Some((start, end)) if start < bytes.len() && end >= start => {
                            ranges.fetch_add(1, Ordering::AcqRel);
                            let end = end.min(bytes.len() - 1);
                            (
                                "206 Partial Content",
                                &bytes[start..=end],
                                format!("Content-Range: bytes {start}-{end}/{}\r\n", bytes.len()),
                            )
                        }
                        Some(_) => ("416 Range Not Satisfiable", &b"bad range"[..], String::new()),
                        None => {
                            fulls.fetch_add(1, Ordering::AcqRel);
                            ("200 OK", &bytes[..], String::new())
                        }
                    }
                } else {
                    fulls.fetch_add(1, Ordering::AcqRel);
                    ("200 OK", &bytes[..], String::new())
                };
                let header = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{content_range}Connection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(body);
            }
        });
        Self { address, stop, thread: Some(thread), ranged, full }
    }
    fn url(&self) -> String {
        format!("http://{}/archive.zip", self.address)
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.address);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}
fn remote_zip() -> Vec<u8> {
    // Deterministic incompressible padding replaces random setup, not any oracle.
    let mut state = 0x12345678u32;
    let pad = (0..4096)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        })
        .collect();
    zip::encode(&[
        ("docs/readme.md".into(), b"# remote readme\n".to_vec()),
        ("docs/data.bin".into(), b"0123456789".repeat(64)),
        ("docs/pad.bin".into(), pad),
    ])
    .unwrap()
}
#[test]
fn ranged_remote_zip_list_and_member_without_full_download() {
    let bytes = remote_zip();
    let server = Server::start(bytes.clone(), true);
    let source = source::http_byte_source(&server.url(), source::HttpByteSourceOptions::default()).unwrap();
    assert_eq!(source.size(), bytes.len() as u64);
    let i = index_source(source, ReadFormat::Zip, Some(Path::new("remote.zip")), ReadLimits::default()).unwrap();
    let reader = Reader::from_index(i, ReadLimits::default()).unwrap();
    assert_eq!(list(&reader, "docs"), ["data.bin", "pad.bin", "readme.md"]);
    assert_eq!(member(&reader, "docs/readme.md"), b"# remote readme\n");
    assert_eq!(server.full.load(Ordering::Acquire), 0);
    assert!(server.ranged.load(Ordering::Acquire) > 0);
}
#[test]
fn ignored_range_one_bounded_buffered_download() {
    let bytes = remote_zip();
    let server = Server::start(bytes.clone(), false);
    let source = source::http_byte_source(&server.url(), source::HttpByteSourceOptions::default()).unwrap();
    assert_eq!(source.size(), bytes.len() as u64);
    let i = index_source(source, ReadFormat::Zip, None, ReadLimits::default()).unwrap();
    assert_eq!(index_member(&i, "docs/data.bin").len(), 640);
    // Also checks the contract comment's one-download claim.
    assert_eq!(server.full.load(Ordering::Acquire), 1);
}
#[test]
fn no_range_fallback_cap_rejection() {
    let server = Server::start(remote_zip(), false);
    error_contains(
        source::http_byte_source(
            &server.url(),
            source::HttpByteSourceOptions { max_fallback_bytes: 8, ..Default::default() },
        ),
        "too large to buffer",
    );
}
#[test]
fn cached_shared_blocks_and_large_direct_fetch() {
    let bytes = remote_zip();
    let (source, reads) = counting(bytes.clone());
    let cached = source::caching_byte_source(source, 256, 8).unwrap();
    let first = cached.range(0, 16).unwrap();
    let second = cached.range(4, 26).unwrap();
    assert_eq!(reads.lock().unwrap().len(), 1);
    assert_eq!(first, bytes[..16]);
    assert_eq!(second, bytes[4..30]);
    cached.range(200, 100).unwrap();
    assert_eq!(reads.lock().unwrap().len(), 2);
    cached.range(0, bytes.len() as u64).unwrap();
    assert_eq!(reads.lock().unwrap().len(), 3);
}
