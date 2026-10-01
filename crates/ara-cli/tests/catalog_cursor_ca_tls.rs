//! Source/native Cursor H2 trust comparison against the same real CA-signed endpoint.
use ara_cli::{
    catalog_discovery::{cursor::*, *},
    catalog_extra_ca::*,
};
use ara_rpc::WireString;
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const SOURCE_RECEIPT_SHA: &str = "81ba2f67ec66e9395882b57d762a7052a23bcbf47965338da44f04f76b9b83b8";
fn hash(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes).as_ref().iter().map(|byte| format!("{byte:02x}")).collect()
}
struct Host {
    env: Mutex<Option<WireString>>,
}
impl ExtraCaHost for Host {
    fn node_extra_ca_certs(&self) -> Option<WireString> {
        self.env.lock().unwrap().clone()
    }
    fn file_mtime_ms(&self, path: &WireString) -> Result<f64, ExtraCaIoError> {
        NativeExtraCaHost.file_mtime_ms(path)
    }
    fn read_utf8(&self, path: &WireString) -> Result<WireString, ExtraCaIoError> {
        NativeExtraCaHost.read_utf8(path)
    }
    fn root_certificates(&self) -> Result<Vec<WireString>, DiscoveryError> {
        NativeExtraCaHost.root_certificates()
    }
}
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
#[ignore = "requires retained fixed-source Cursor CA receipt and pinned Bun"]
async fn cursor_h2_uses_startup_trust_separately_from_dynamic_fetch_ca() {
    let source_receipt =
        PathBuf::from(std::env::var_os("ARA_CATALOG_CURSOR_CA_SOURCE_RECEIPT").expect("source receipt"));
    let bytes = std::fs::read(&source_receipt).unwrap();
    assert_eq!(hash(&bytes), SOURCE_RECEIPT_SHA);
    let source: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(source["upstreamCommit"], "596f2da7101178214aa27a753529d15e6b7ad91d");
    assert_eq!(source["cursorSource"]["sha256"], "0b1583b566e80c3b8fae8583a2a421c2f197eee1f3d90cd6204b58ab0b7591e1");
    let directory = source_receipt.parent().unwrap();
    let script = directory.join("probe.mjs");
    assert_eq!(hash(&std::fs::read(&script).unwrap()), source["scriptSha256"]);
    for (name, digest) in source["fixtureSha256"].as_object().unwrap() {
        assert_eq!(hash(&std::fs::read(directory.join(name)).unwrap()), *digest);
    }
    let bun = PathBuf::from(std::env::var_os("ARA_CATALOG_DISCOVERY_BUN").expect("pinned Bun"));
    assert_eq!(hash(&std::fs::read(&bun).unwrap()), source["bunSha256"]);
    let scratch = tempfile::tempdir().unwrap();
    let ready = scratch.path().join("ready.json");
    let receipts = scratch.path().join("receipts.json");
    let config = scratch.path().join("server.json");
    std::fs::write(
        &config,
        json!({"cert":directory.join("leaf.pem"),"key":directory.join("leaf.key"),"ready":ready,"receipts":receipts})
            .to_string(),
    )
    .unwrap();
    let mut server = Process(
        Command::new(&bun)
            .arg(&script)
            .arg("--server")
            .arg(source["sourceRun"].as_str().unwrap())
            .arg(&config)
            .env_remove("NODE_EXTRA_CA_CERTS")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let started = Instant::now();
    while !ready.exists() {
        if let Some(exit) = server.0.try_wait().unwrap() {
            panic!("retained H2 server stopped: {exit}");
        }
        assert!(started.elapsed() < Duration::from_secs(5), "H2 readiness deadline");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let ports: Value = serde_json::from_slice(&std::fs::read(&ready).unwrap()).unwrap();
    let clear = format!("http://127.0.0.1:{}", ports["clearPort"].as_u64().unwrap());
    let tls = format!("https://127.0.0.1:{}", ports["tlsPort"].as_u64().unwrap());
    let mut native_clients = Vec::new();
    for client in source["clients"].as_array().unwrap() {
        let name = client["name"].as_str().unwrap();
        let input_bytes = std::fs::read(directory.join(format!("{name}.json"))).unwrap();
        let pin = source["clientConfigs"].as_array().unwrap().iter().find(|entry| entry["name"] == name).unwrap();
        assert_eq!(hash(&input_bytes), pin["sha256"]);
        let input: Value = serde_json::from_slice(&input_bytes).unwrap();
        let host = Arc::new(Host { env: Mutex::new(client["startupEnv"].as_str().map(Into::into)) });
        // The host supplies the process-start snapshot. Native H2 treats its
        // startup value as a raw filename; this does not invoke the fetch shim.
        let startup_pem = client["startupEnv"].as_str().and_then(|path| std::fs::read(path).ok());
        let context = CatalogContext::for_host_with_startup_extra_ca(
            Arc::new(ExtraCaRuntime::new(host.clone())),
            startup_pem.as_deref(),
        )
        .unwrap();
        assert!(!context.http2_transport().extra_ca_wrapped());
        let mut native_steps = Vec::new();
        for (step, expected) in input["steps"].as_array().unwrap().iter().zip(client["steps"].as_array().unwrap()) {
            if let Some(env) = step.get("env") {
                *host.env.lock().unwrap() = env.as_str().map(Into::into);
            }
            let base = if step["url"].as_str().unwrap().starts_with("https://") { &tls } else { &clear };
            let result = fetch_cursor_usable_models(
                &context,
                &CursorModelDiscoveryOptions {
                    api_key: "fixture-token".into(),
                    base_url: Some(base.as_str().into()),
                    timeout_ms: Some(500.0),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            let models = match result {
                Some(models) => {
                    assert!(models.is_empty());
                    json!([])
                }
                None => Value::Null,
            };
            let native = json!({"id":step["id"],"env":host.env.lock().unwrap().as_ref().map(|value|value.to_utf8().unwrap()),"models":models});
            assert_eq!(native, *expected, "source/native H2 mismatch in {name}");
            native_steps.push(native);
        }
        native_clients.push(json!({"name":name,"startupEnv":client["startupEnv"],"steps":native_steps}));
    }
    assert_eq!(native_clients, *source["clients"].as_array().unwrap());
    let native_server: Value = serde_json::from_slice(&std::fs::read(&receipts).unwrap()).unwrap();
    assert_eq!(native_server, source["server"], "actual source/native H2 request receipts");
    if let Some(path) = std::env::var_os("ARA_CATALOG_CURSOR_CA_NATIVE_RECEIPT") {
        std::fs::write(path, serde_json::to_vec_pretty(&json!({"sourceReceipt":source_receipt,"sourceReceiptSha256":SOURCE_RECEIPT_SHA,"nativeClients":native_clients,"nativeServer":native_server,"mismatches":[]})).unwrap()).unwrap();
    }
}
