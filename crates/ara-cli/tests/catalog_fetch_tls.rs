//! Real native TLS options compared with independently observed pinned Bun requests.
//! Error text varies by TLS backend; successful handshakes and server receipts do not.
use ara_cli::{catalog_discovery::*, catalog_extra_ca::*, model_collapse::VariantSpec};
use ara_rpc::{WireString, WireValue};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};
const RECEIPT_SHA: &str = "7cdbe7cde4e4d68ed54de4672b3ddf05f4c76b3979dad1a945f50804077ec0c2";
const SERVER_CONFIG_SHA: &str = "9b192d596abbcb0f14a5a2e602143bf150546eaefc08b90067e0d4310df6f444";
const READY_SHA: &str = "a7a80504647a6d2af6016907ea6110a8cc37c1ebb57158a4640ebe903cfd68ac";
fn hash(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes).as_ref().iter().map(|b| format!("{b:02x}")).collect()
}
struct Host(Option<WireString>);
impl ExtraCaHost for Host {
    fn node_extra_ca_certs(&self) -> Option<WireString> {
        self.0.clone()
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
fn load_pinned(path: &std::path::Path, pin: &str) -> Value {
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(hash(&bytes), pin, "pin {}", path.display());
    serde_json::from_slice(&bytes).unwrap()
}
#[tokio::test]
#[ignore = "requires retained pinned Bun cleartext TLS receipt"]
async fn cleartext_ignores_invalid_tls_options_like_bun() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let artifact = PathBuf::from(
        std::env::var_os("ARA_CATALOG_FETCH_CLEARTEXT_TLS_SOURCE_RECEIPT").expect("frozen cleartext TLS receipt"),
    );
    let source = load_pinned(&artifact, "df8698af7786ad0969f3db13764a2a258096be1e01c37109a357ff321ec60b36");
    assert_eq!(
        hash(&std::fs::read(artifact.parent().unwrap().join("probe.mjs")).unwrap()),
        "da0bf6e13346bbd31b941293dc223d588a616788eef7bcfe0978d466e8bbe75f"
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut paths = Vec::new();
        for _ in 0..4 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                let mut chunk = [0; 2048];
                let count = socket.read(&mut chunk).await.unwrap();
                assert_ne!(count, 0, "request header ended early");
                bytes.extend_from_slice(&chunk[..count]);
            }
            let request = String::from_utf8(bytes).unwrap();
            paths.push(request.lines().next().unwrap().split_whitespace().nth(1).unwrap().to_owned());
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 18\r\nConnection: close\r\n\r\ncleartext accepted")
                .await
                .unwrap();
        }
        paths
    });
    let transport = NativeDiscoveryTransport::with_extra_ca(None).unwrap();
    let mut native = Vec::new();
    for case in source["cases"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        let reply = tokio::time::timeout(
            Duration::from_secs(2),
            transport.fetch(DiscoveryRequest {
                url: format!("http://{address}/{id}").into(),
                tls: Some(VariantSpec::from_json(&case["tls"])),
                ..Default::default()
            }),
        )
        .await
        .expect("cleartext request settled")
        .unwrap();
        assert_eq!(json!(reply.status), case["status"], "{id}");
        assert_eq!(json!(String::from_utf8(reply.body.clone()).unwrap()), case["body"], "{id}");
        native.push(json!({"id": id, "status": reply.status, "body": String::from_utf8(reply.body).unwrap()}));
    }
    let requests = tokio::time::timeout(Duration::from_secs(2), server).await.unwrap().unwrap();
    assert_eq!(json!(requests), source["requests"]);
    if let Some(path) = std::env::var_os("ARA_CATALOG_FETCH_CLEARTEXT_TLS_NATIVE_RECEIPT") {
        std::fs::write(path, serde_json::to_vec_pretty(&json!({"native":native,"requests":requests})).unwrap())
            .unwrap();
    }
}
#[tokio::test]
#[ignore = "requires retained Bun TLS option receipt and pinned Bun"]
async fn fetch_tls_options_match_actual_bun_handshakes() {
    replay_tls_receipt(
        "ARA_CATALOG_FETCH_TLS_SOURCE_RECEIPT",
        "ARA_CATALOG_FETCH_TLS_NATIVE_RECEIPT",
        RECEIPT_SHA,
        SERVER_CONFIG_SHA,
        READY_SHA,
    )
    .await;
}
#[tokio::test]
#[ignore = "requires retained supplemental Bun TLS receipt and pinned Bun"]
async fn supplemental_tls_options_match_actual_bun_handshakes() {
    replay_tls_receipt(
        "ARA_CATALOG_FETCH_TLS_SUPPLEMENTAL_SOURCE_RECEIPT",
        "ARA_CATALOG_FETCH_TLS_SUPPLEMENTAL_NATIVE_RECEIPT",
        "7bc9bd775bce548ee37219e78b7f11815f73dc8be21103d6c9ac823a830b3741",
        "c678cb069d0d0d6100e9693f5c60b3551c51d08c43ed0ff706e33469d9cbf733",
        "9b218a1b55b9f6beb0fdfcf35f6994a1fef5b969de8a3b74c34abb6861300664",
    )
    .await;
}
async fn replay_tls_receipt(source_env: &str, native_env: &str, receipt_sha: &str, server_sha: &str, ready_sha: &str) {
    let artifact = PathBuf::from(std::env::var_os(source_env).expect("frozen fetch TLS receipt"));
    let source = load_pinned(&artifact, receipt_sha);
    let directory = artifact.parent().unwrap();
    assert_eq!(source["upstreamCommit"], "596f2da7101178214aa27a753529d15e6b7ad91d");
    let source_file = PathBuf::from(source["sourceRun"].as_str().unwrap())
        .join("upstream")
        .join(source["source"]["path"].as_str().unwrap());
    assert_eq!(hash(&std::fs::read(source_file).unwrap()), source["source"]["sha256"]);
    let script = directory.join("probe.mjs");
    assert_eq!(hash(&std::fs::read(&script).unwrap()), source["scriptSha256"]);
    for (name, pin) in source["fixtureSha256"].as_object().unwrap() {
        assert_eq!(hash(&std::fs::read(directory.join(name)).unwrap()), *pin, "fixture {name}");
    }
    let bun = PathBuf::from(std::env::var_os("ARA_CATALOG_DISCOVERY_BUN").expect("pinned Bun"));
    assert_eq!(hash(&std::fs::read(&bun).unwrap()), source["bunSha256"]);
    let mut server_config = load_pinned(&directory.join("server.json"), server_sha);
    let old_ports = load_pinned(&directory.join("ready.json"), ready_sha);
    let scratch = tempfile::tempdir().unwrap();
    let ready = scratch.path().join("ready.json");
    let receipts = scratch.path().join("receipts.json");
    let config = scratch.path().join("server.json");
    server_config["ready"] = json!(ready);
    server_config["receipts"] = json!(receipts);
    std::fs::write(&config, server_config.to_string()).unwrap();
    let mut server = Process(
        Command::new(&bun)
            .arg(&script)
            .arg("--server")
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
            panic!("TLS fixture stopped {exit}");
        }
        assert!(started.elapsed() < Duration::from_secs(5), "TLS fixture readiness");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let ports: Value = serde_json::from_slice(&std::fs::read(&ready).unwrap()).unwrap();
    let mut actual_cases = Vec::new();
    let mut mismatches = Vec::new();
    for expected in source["cases"].as_array().unwrap() {
        let id = expected["id"].as_str().unwrap();
        let case = load_pinned(&directory.join(format!("{id}.json")), source["caseConfigSha256"][id].as_str().unwrap());
        let mut url = reqwest::Url::parse(case["url"].as_str().unwrap()).unwrap();
        let old_port = u64::from(url.port().unwrap());
        let (server_name, _) = old_ports
            .as_object()
            .unwrap()
            .iter()
            .find(|(_, port)| port.as_u64() == Some(old_port))
            .expect("source endpoint port");
        url.set_port(Some(ports[server_name].as_u64().unwrap() as u16)).unwrap();
        let mut tls = case["tls"].clone();
        for key in ["ca", "cert", "key"] {
            if let Some(value) = tls.get_mut(key) {
                if let Some(paths) = value.as_array_mut() {
                    for path in paths {
                        *path = json!(std::fs::read_to_string(path.as_str().unwrap()).unwrap());
                    }
                } else if let Some(path) = value.as_str() {
                    *value = json!(std::fs::read_to_string(path).unwrap());
                }
            }
        }
        let startup = case["startup"].as_str().map(|path| std::fs::read(path).unwrap());
        let plain: Arc<dyn DiscoveryTransport> =
            Arc::new(NativeDiscoveryTransport::with_extra_ca(startup.as_deref()).unwrap());
        let transport = if case["wrapper"].as_bool() == Some(true) {
            wrap_fetch_for_extra_ca(
                Arc::new(ExtraCaRuntime::new(Arc::new(Host(case["startup"].as_str().map(Into::into))))),
                plain,
            )
        } else {
            plain
        };
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            transport.fetch(DiscoveryRequest {
                url: url.to_string().into(),
                tls: Some(VariantSpec::from_wire(WireValue::parse(&tls.to_string()).unwrap())),
                headers: vec![("Connection".into(), "close".into())],
                ..Default::default()
            }),
        )
        .await
        .expect("TLS probe must settle; timeout cannot stand for a rejected handshake");
        let actual = match result {
            Ok(reply) => {
                json!({"id":id,"ok":true,"status":reply.status,"body":serde_json::from_slice::<Value>(&reply.body).expect("fixture JSON")})
            }
            Err(error) => {
                json!({"id":id,"ok":false,"name":error.name.to_utf8().unwrap(),"message":error.message.to_utf8().unwrap()})
            }
        };
        if actual["ok"] != expected["ok"]
            || actual["ok"] == true && (actual["status"] != expected["status"] || actual["body"] != expected["body"])
        {
            mismatches.push(json!({"id":id,"expected":expected,"actual":actual}));
        }
        actual_cases.push(actual);
    }
    let native_server: Value =
        if receipts.exists() { serde_json::from_slice(&std::fs::read(&receipts).unwrap()).unwrap() } else { json!([]) };
    if native_server != source["serverReceipts"] {
        mismatches.push(json!({"id":"all-server-receipts","expected":source["serverReceipts"],"actual":native_server}));
    }
    let receipt = json!({"sourceReceipt":artifact,"sourceReceiptSha256":receipt_sha,"nativeCases":actual_cases,"nativeServer":native_server,"mismatches":mismatches});
    if let Some(path) = std::env::var_os(native_env) {
        std::fs::write(path, serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();
    }
    assert!(mismatches.is_empty(), "{}", serde_json::to_string_pretty(&receipt).unwrap());
}
