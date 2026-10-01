//! Real socket boundaries that cannot be established by injected Responses.
use ara_cli::{
    catalog_discovery::{openai::*, *},
    model_collapse::VariantSpec,
};
use ara_rpc::{WireString, WireValue};
use std::{io::Write, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

async fn server(response: Vec<u8>, stall: bool) -> (WireString, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 8192];
        if stream.read(&mut request).await.unwrap() == 0 {
            return;
        }
        stream.write_all(&response).await.unwrap();
        stream.flush().await.unwrap();
        if stall {
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });
    (format!("http://{address}").into(), task)
}

#[tokio::test]
async fn error_headers_return_without_stalled_body() {
    for tls in [None, Some(VariantSpec::from_wire(WireValue::Object(vec![])))] {
        let (url, task) = server(
            b"HTTP/1.1 403 Forbidden\r\nContent-Length: 1000000\r\nConnection: keep-alive\r\n\r\n".to_vec(),
            true,
        )
        .await;
        let transport = NativeDiscoveryTransport::with_extra_ca(None).unwrap();
        let reply = tokio::time::timeout(
            Duration::from_secs(2),
            transport.fetch(DiscoveryRequest { tls: tls.clone(), url, ..Default::default() }),
        )
        .await
        .expect("must resolve at headers")
        .unwrap();
        assert_eq!(reply.status, 403);
        assert!(reply.body.is_empty());
        task.abort();
    }
}

#[tokio::test]
async fn error_body_consumer_can_opt_in_and_cancel() {
    for tls in [None, Some(VariantSpec::from_wire(WireValue::Object(vec![])))] {
        let (url, task) =
            server(b"HTTP/1.1 500 Error\r\nContent-Length: 1000000\r\nConnection: keep-alive\r\n\r\n".to_vec(), true)
                .await;
        let transport = NativeDiscoveryTransport::with_extra_ca(None).unwrap();
        let signal = DiscoverySignal::default();
        let cancellation = signal.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancellation.abort(DiscoveryError::new("cancel body"));
        });
        let error = tokio::time::timeout(
            Duration::from_secs(2),
            transport.fetch(DiscoveryRequest {
                tls: tls.clone(),
                url,
                body_policy: DiscoveryBodyPolicy::Always,
                signal: Some(signal),
                ..Default::default()
            }),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert_eq!(error.message, WireString::from("cancel body"));
        task.abort();
    }
}

#[tokio::test]
async fn compressed_json_and_latin1_headers_cross_real_socket() {
    for tls in [None, Some(VariantSpec::from_wire(WireValue::Object(vec![])))] {
        let payload = br#"{"data":[{"id":"socket-model"}]}"#;
        for encoding in ["gzip", "br", "deflate", "zstd"] {
            let body = match encoding {
                "gzip" => {
                    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                    encoder.write_all(payload).unwrap();
                    encoder.finish().unwrap()
                }
                "br" => {
                    let mut encoder = brotli::CompressorWriter::new(Vec::new(), 4096, 5, 22);
                    encoder.write_all(payload).unwrap();
                    encoder.into_inner()
                }
                "deflate" => {
                    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
                    encoder.write_all(payload).unwrap();
                    encoder.finish().unwrap()
                }
                _ => zstd::stream::encode_all(payload.as_slice(), 0).unwrap(),
            };
            let mut response=format!("HTTP/1.1 200 OK\r\nContent-Encoding: {encoding}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nEtag: ",body.len()).into_bytes();
            response.push(0xe9);
            response.extend_from_slice(b"\r\nConnection: close\r\n\r\n");
            response.extend_from_slice(&body);
            let (url, task) = server(response, false).await;
            let transport = NativeDiscoveryTransport::with_extra_ca(None).unwrap();
            let reply =
                transport.fetch(DiscoveryRequest { tls: tls.clone(), url, ..Default::default() }).await.unwrap();
            assert_eq!(reply.header("etag").unwrap().units(), &[233]);
            assert_eq!(reply.body, payload, "{encoding}");
            assert!(reply.json().is_ok());
            task.await.unwrap();
        }
    }
}

#[tokio::test]
async fn repeated_response_headers_are_combined() {
    for tls in [None, Some(VariantSpec::from_wire(WireValue::Object(vec![])))] {
        let (url,task)=server(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nEtag: one\r\nEtag: two\r\nX-Next-Page: 1\r\nX-Next-Page: 2\r\nConnection: close\r\n\r\n".to_vec(),false).await;
        let transport = NativeDiscoveryTransport::with_extra_ca(None).unwrap();
        let reply = transport.fetch(DiscoveryRequest { tls: tls.clone(), url, ..Default::default() }).await.unwrap();
        assert_eq!(reply.header("etag"), Some(&WireString::from("one, two")));
        assert_eq!(reply.header("x-next-page"), Some(&WireString::from("1, 2")));
        task.await.unwrap();
    }
}

#[tokio::test]
async fn redirects_keep_fixed_bun_boundary() {
    for tls in [None, Some(VariantSpec::from_wire(WireValue::Object(vec![])))] {
        for redirects in [11usize, 126, 127] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let task = tokio::spawn(async move {
                for index in 0..=redirects {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let mut request = vec![0; 8192];
                    if stream.read(&mut request).await.unwrap() == 0 {
                        return;
                    }
                    let response = if index == redirects {
                        "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_owned()
                    } else {
                        format!(
                            "HTTP/1.1 302 Found\r\nLocation: /{}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                            index + 1
                        )
                    };
                    stream.write_all(response.as_bytes()).await.unwrap();
                }
            });
            let transport = NativeDiscoveryTransport::with_extra_ca(None).unwrap();
            let result = tokio::time::timeout(
                Duration::from_secs(10),
                transport.fetch(DiscoveryRequest {
                    tls: tls.clone(),
                    url: format!("http://{address}/0").into(),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
            assert_eq!(result.is_ok(), redirects <= 126, "{redirects} redirects");
            task.abort();
        }
    }
}

#[tokio::test]
async fn overflow_timer_aborts_real_pending_fetch() {
    let (url, task) = server(b"HTTP/1.1 200 OK\r\nContent-Length: 1000000\r\n\r\n".to_vec(), true).await;
    let context = CatalogContext::new(Arc::new(NativeDiscoveryTransport::with_extra_ca(None).unwrap())).unwrap();
    let mut options = OpenAiCompatibleOptions::new("openai-completions", "fixture", url);
    options.timeout_ms = Some(2_147_483_648.0);
    assert!(
        tokio::time::timeout(Duration::from_secs(2), fetch_openai_compatible_models(&context, &options))
            .await
            .expect("overflow cannot schedule 24 days")
            .unwrap()
            .is_none()
    );
    task.abort();
}

#[test]
fn combined_signal_is_synchronous_and_keeps_first_reason() {
    let parent = DiscoverySignal::default();
    let secondary = DiscoverySignal::default();
    let combined = DiscoverySignal::any(&[parent.clone(), secondary.clone()]);
    let retained = combined.clone();
    parent.abort(DiscoveryError::new("parent reason"));
    assert!(retained.is_aborted());
    secondary.abort(DiscoveryError::new("later reason"));
    assert_eq!(combined.reason().unwrap().message, WireString::from("parent reason"));
    let pre_aborted = DiscoverySignal::any(&[parent]);
    assert!(pre_aborted.is_aborted());
    assert_eq!(pre_aborted.reason(), combined.reason());
}

#[test]
fn combined_signal_retains_temporary_nested_sources() {
    let parent = DiscoverySignal::default();
    let retained = DiscoverySignal::any(&[DiscoverySignal::any(std::slice::from_ref(&parent))]);
    parent.abort(DiscoveryError::new("nested reason"));
    assert!(retained.is_aborted());
    assert_eq!(retained.reason().unwrap().message, WireString::from("nested reason"));
}
