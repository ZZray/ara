//! Replays RPC scenarios; Cursor uses the retained fixture's real loopback h2 server.
use crate::{FixtureTransport, array, decode, encode, failure, field, insert_record, s, specs_value, success, text};
use ara_cli::{
    catalog_discovery::{cursor::*, devin::*, *},
    catalog_protobuf::ProtoValue,
    model_collapse::{SpecRef, VariantSpec},
};
use ara_rpc::{WireString, WireValue as W};
use async_trait::async_trait;
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

fn optional(value: &VariantSpec, key: &str) -> Option<WireString> {
    value.get(key).and_then(W::as_string).cloned()
}
fn with_logs(context: &CatalogContext, mut result: W) -> W {
    let logs = context
        .runtime
        .diagnostics
        .lock()
        .unwrap()
        .iter()
        .map(|log| {
            let arguments: Vec<SpecRef> = log.arguments.iter().cloned().map(Arc::new).collect();
            W::object(vec![("level", s(log.level.clone())), ("args", encode(&specs_value(&arguments)))])
        })
        .collect();
    result.insert("logs", W::Array(logs));
    result
}
fn model_result(value: DiscoveryResult) -> W {
    match value {
        Err(error) => failure(error),
        Ok(None) => success(&VariantSpec::from_wire(W::Null)),
        Ok(Some(models)) => success(&specs_value(&models)),
    }
}
pub(super) fn normalize_rpc_port(value: &mut W, port: &str) {
    match value {
        W::String(s) => {
            if let Ok(text) = s.to_utf8() {
                *s = text.replace(port, "$PORT").into();
            }
        }
        W::Array(values) => {
            for value in values {
                normalize_rpc_port(value, port);
            }
        }
        W::Object(values) => {
            for (_, value) in values {
                normalize_rpc_port(value, port);
            }
        }
        _ => {}
    }
}
struct Server(Child);
struct RetainingTransport {
    inner: Arc<FixtureTransport>,
    signal: Mutex<Option<DiscoverySignal>>,
}
#[async_trait]
impl DiscoveryTransport for RetainingTransport {
    fn context_id(&self) -> u64 {
        self.inner.context_id()
    }
    async fn fetch(&self, request: DiscoveryRequest) -> Result<DiscoveryReply, DiscoveryError> {
        *self.signal.lock().unwrap() = request.signal.clone();
        self.inner.fetch(request).await
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Owns the retained oracle adapter process and its receipt files for one h2 scenario.
pub(super) struct RpcFixtureServer {
    _server: Server,
    _scratch: tempfile::TempDir,
    pub(super) port: u16,
    receipts: PathBuf,
}

impl RpcFixtureServer {
    pub(super) async fn read_receipts(&self, expect_closed: bool) -> W {
        let closed_deadline = Instant::now() + Duration::from_millis(500);
        loop {
            let receipt = std::fs::read_to_string(&self.receipts)
                .ok()
                .and_then(|text| W::parse(&text).ok())
                .unwrap_or_else(|| W::object(vec![("requests", W::Array(vec![])), ("peerClosed", W::Bool(false))]));
            if !expect_closed
                || matches!(receipt.get("peerClosed"), Some(W::Bool(true)))
                || Instant::now() >= closed_deadline
            {
                return receipt;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

pub(super) async fn start_rpc_fixture_server(test: &W) -> RpcFixtureServer {
    let oracle_file =
        PathBuf::from(std::env::var_os("ARA_CATALOG_DISCOVERY_ORACLE").expect("retained fixed-source corpus"));
    let directory = oracle_file.parent().unwrap();
    let oracle = W::parse(&std::fs::read_to_string(&oracle_file).unwrap()).unwrap();
    let adapter = array(field(&oracle, "adapters"))
        .iter()
        .find(|entry| text(field(entry, "source")).ends_with("catalog_discovery_fixtures_rpc.mjs"))
        .expect("RPC adapter provenance");
    let adapter_path = directory.join(text(field(adapter, "path")));
    let bun = PathBuf::from(std::env::var_os("ARA_CATALOG_DISCOVERY_BUN").expect("pinned Bun for native h2 server"));
    let digest = ring::digest::digest(&ring::digest::SHA256, &std::fs::read(&bun).unwrap())
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    assert_eq!(digest, text(field(&oracle, "bunSha256")));
    let fixture_digest = ring::digest::digest(&ring::digest::SHA256, &std::fs::read(&adapter_path).unwrap())
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    assert_eq!(fixture_digest, text(field(adapter, "sha256")));
    let scratch = tempfile::tempdir().unwrap();
    let config = scratch.path().join("test.json");
    let ready = scratch.path().join("ready.json");
    let receipts = scratch.path().join("receipts.json");
    std::fs::write(&config, test.stringify()).unwrap();
    let mut server = Server(
        Command::new(bun)
            .arg(&adapter_path)
            .arg("--server")
            .arg(&config)
            .arg(&ready)
            .arg(&receipts)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let started = Instant::now();
    while !ready.exists() {
        if let Some(exit) = server.0.try_wait().unwrap() {
            panic!("RPC h2 fixture stopped before ready: {exit}");
        }
        assert!(started.elapsed() < Duration::from_secs(5), "h2 server readiness deadline");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let ready_value = W::parse(&std::fs::read_to_string(&ready).unwrap()).unwrap();
    let port = field(&ready_value, "port").as_number().unwrap() as u16;
    RpcFixtureServer { _server: server, _scratch: scratch, port, receipts }
}

pub async fn replay(test: &W) -> W {
    let options = test
        .get("optionsEncoded")
        .map(decode)
        .unwrap_or_else(|| VariantSpec::from_wire(test.get("options").cloned().unwrap_or_else(|| W::Object(vec![]))));
    if text(field(test, "family")) == "devin" {
        let transport = Arc::new(FixtureTransport::new(test));
        let retaining = Arc::new(RetainingTransport { inner: transport.clone(), signal: Mutex::new(None) });
        let context = CatalogContext::new(retaining.clone()).unwrap();
        let signal = test.get("signal").map(|value| {
            let signal = DiscoverySignal::default();
            if matches!(value.get("aborted"), Some(W::Bool(true))) {
                signal.abort(DiscoveryError::named("AbortError", "This operation was aborted."));
            }
            signal
        });
        let options = DevinModelDiscoveryOptions {
            api_key: optional(&options, "apiKey"),
            base_url: optional(&options, "baseUrl"),
            timeout_ms: options.get("timeoutMs").and_then(W::as_number),
            signal,
        };
        let models = fetch_devin_models(&context, &options).await;
        let result = if matches!(test.get("afterAbort"), Some(W::Bool(true))) {
            options.signal.as_ref().unwrap().abort(DiscoveryError::named("AbortError", "This operation was aborted."));
            let mut value = VariantSpec::from_wire(W::Object(vec![]));
            match models {
                Ok(Some(models)) => insert_record(&mut value, "models", &specs_value(&models)),
                Ok(None) => value.set("models", W::Null),
                Err(error) => return failure(error),
            };
            value.set(
                "retainedSignal",
                W::object(vec![("aborted", W::Bool(retaining.signal.lock().unwrap().as_ref().unwrap().is_aborted()))]),
            );
            success(&value)
        } else {
            model_result(models)
        };
        return with_logs(&context, transport.finish(result));
    }
    let server = start_rpc_fixture_server(test).await;
    let port = server.port;
    let suffix = test.get("baseSuffix").map(text).unwrap_or_default();
    let base = format!("http://127.0.0.1:{port}{suffix}");
    let context = CatalogContext::new(Arc::new(NativeDiscoveryTransport::with_extra_ca(None).unwrap())).unwrap();
    let options = CursorModelDiscoveryOptions {
        api_key: optional(&options, "apiKey").unwrap_or_else(|| "".into()),
        base_url: Some(base.into()),
        client_version: optional(&options, "clientVersion"),
        timeout_ms: options.get("timeoutMs").and_then(W::as_number),
        custom_model_ids: options
            .get("customModelIds")
            .and_then(W::as_array)
            .map(|values| values.iter().map(ProtoValue::from_wire).collect()),
    };
    let models = fetch_cursor_usable_models(&context, &options).await;
    let expect_closed = matches!(test.get("expectClosed"), Some(W::Bool(true)));
    let receipt = server.read_receipts(expect_closed).await;
    let requests = field(&receipt, "requests").clone();
    let mut value = VariantSpec::from_wire(W::object(vec![]));
    match models {
        Ok(Some(models)) => insert_record(&mut value, "models", &specs_value(&models)),
        Ok(None) => value.set("models", W::Null),
        Err(error) => return failure(error),
    };
    let mut protocol = W::object(vec![("requests", requests)]);
    if expect_closed {
        protocol.insert("peerClosed", field(&receipt, "peerClosed").clone());
    }
    value.set("protocol", protocol);
    normalize_rpc_port(&mut value.value, &port.to_string());
    let mut result = success(&value);
    result.insert("fetchCalls", W::Array(vec![]));
    with_logs(&context, result)
}
