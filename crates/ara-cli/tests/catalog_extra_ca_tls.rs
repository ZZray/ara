//! Actual HTTPS acceptance of default discovery's construction-gated CA path.
use ara_cli::{catalog_discovery::*, catalog_extra_ca::*};
use ara_rpc::WireString;
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

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
fn cert(openssl: &Path, root: &Path, name: &str) -> (PathBuf, PathBuf, PathBuf) {
    let pem = root.join(format!("{name}.pem"));
    let ca_key = root.join(format!("{name}-ca.key"));
    let key = root.join(format!("{name}.key"));
    let output = Command::new(openssl)
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-days",
            "2",
            "-subj",
            "/CN=localhost",
            "-addext",
            "subjectAltName=DNS:localhost,IP:127.0.0.1",
            "-addext",
            "basicConstraints=critical,CA:TRUE",
            "-keyout",
        ])
        .arg(&ca_key)
        .arg("-out")
        .arg(&pem)
        .output()
        .unwrap();
    assert!(output.status.success(), "OpenSSL: {}", String::from_utf8_lossy(&output.stderr));
    let csr = root.join(format!("{name}.csr"));
    let leaf = root.join(format!("{name}-leaf.pem"));
    let extensions = root.join(format!("{name}-leaf.cnf"));
    std::fs::write(&extensions,"subjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n").unwrap();
    let request = Command::new(openssl)
        .args(["req", "-new", "-newkey", "rsa:2048", "-nodes", "-subj", "/CN=localhost", "-keyout"])
        .arg(&key)
        .arg("-out")
        .arg(&csr)
        .output()
        .unwrap();
    assert!(request.status.success(), "leaf CSR");
    let signed = Command::new(openssl)
        .args(["x509", "-req", "-days", "2", "-in"])
        .arg(&csr)
        .arg("-CA")
        .arg(&pem)
        .arg("-CAkey")
        .arg(&ca_key)
        .arg("-CAcreateserial")
        .arg("-out")
        .arg(&leaf)
        .arg("-extfile")
        .arg(&extensions)
        .output()
        .unwrap();
    assert!(signed.status.success(), "leaf signature");
    (pem, leaf, key)
}
async fn serve(bun: &Path, fixture: &Path, root: &Path, pem: &Path, key: &Path, name: &str) -> (Process, WireString) {
    let ready = root.join(format!("{name}.json"));
    let process = Process(
        Command::new(bun)
            .arg(fixture)
            .arg("--server")
            .arg(pem)
            .arg(key)
            .arg(&ready)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let start = Instant::now();
    while !ready.exists() {
        assert!(start.elapsed() < Duration::from_secs(5), "HTTPS fixture readiness");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(ready).unwrap()).unwrap();
    (process, format!("https://127.0.0.1:{}/models", value["port"].as_u64().unwrap()).into())
}
async fn fetch(context: &CatalogContext, url: &WireString) -> Result<DiscoveryReply, DiscoveryError> {
    tokio::time::timeout(
        Duration::from_secs(5),
        context.transport.fetch(DiscoveryRequest { url: url.clone(), ..Default::default() }),
    )
    .await
    .expect("HTTPS request bound")
}
#[tokio::test]
#[ignore = "requires local OpenSSL, Bun and byte-verified fixed extra-CA artifact"]
async fn default_ca_wrapper_and_original_cross_real_https() {
    let bun = PathBuf::from(std::env::var_os("ARA_CATALOG_DISCOVERY_BUN").expect("Bun"));
    let openssl = PathBuf::from(std::env::var_os("ARA_CATALOG_EXTRA_CA_OPENSSL").expect("OpenSSL"));
    let artifact = PathBuf::from(std::env::var_os("ARA_CATALOG_EXTRA_CA_ORACLE").expect("fixed source artifact"));
    let oracle: serde_json::Value = serde_json::from_slice(&std::fs::read(&artifact).unwrap()).unwrap();
    assert_eq!(oracle["upstreamCommit"], "596f2da7101178214aa27a753529d15e6b7ad91d");
    let source_path = "packages/utils/src/tls-fetch.ts";
    let source = artifact.parent().unwrap().join("upstream").join(source_path);
    let bytes = std::fs::read(&source).unwrap();
    let pin = oracle["sourceManifest"].as_array().unwrap().iter().find(|row| row["path"] == source_path).unwrap();
    let digest = ring::digest::digest(&ring::digest::SHA256, &bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(pin["sha256"], digest);
    let root = tempfile::tempdir().unwrap();
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/catalog_extra_ca_tls_fixture.mjs");
    let (pem, leaf, key) = cert(&openssl, root.path(), "first");
    let (_first, url) = serve(&bun, &fixture, root.path(), &leaf, &key, "first-ready").await;
    let host = Arc::new(Host { env: Mutex::new(None) });
    let runtime = Arc::new(ExtraCaRuntime::new(host.clone()));
    let unwrapped = CatalogContext::for_host_with_startup_extra_ca(runtime.clone(), None).unwrap();
    assert!(!unwrapped.transport.extra_ca_wrapped());
    assert!(fetch(&unwrapped, &url).await.is_err());
    *host.env.lock().unwrap() = Some(pem.to_string_lossy().into_owned().into());
    assert!(fetch(&unwrapped, &url).await.is_err(), "construction gate remains closed");
    let wrapped = CatalogContext::for_host_with_startup_extra_ca(runtime.clone(), None).unwrap();
    assert!(wrapped.transport.extra_ca_wrapped());
    let reply = fetch(&wrapped, &url).await.unwrap();
    assert_eq!(reply.status, 200);
    assert!(reply.json().is_ok());
    let (second_pem, second_leaf, second_key) = cert(&openssl, root.path(), "second");
    let (_second, second_url) = serve(&bun, &fixture, root.path(), &second_leaf, &second_key, "second-ready").await;
    let original_pem = std::fs::read(&pem).unwrap();
    tokio::time::sleep(Duration::from_millis(5)).await;
    std::fs::write(&pem, std::fs::read(&second_pem).unwrap()).unwrap();
    assert!(fetch(&wrapped, &url).await.is_err(), "rotation must remove old trust");
    assert_eq!(fetch(&wrapped, &second_url).await.unwrap().status, 200, "rotation must trust the new root");
    tokio::time::sleep(Duration::from_millis(5)).await;
    std::fs::write(&pem, &original_pem).unwrap();
    let version_plain = wrapped.version_transport.as_ref().unwrap();
    assert!(!version_plain.extra_ca_wrapped());
    assert!(version_plain.fetch(DiscoveryRequest { url: url.clone(), ..Default::default() }).await.is_err());
    *host.env.lock().unwrap() = Some(format!("{}-missing", pem.display()).into());
    assert_eq!(fetch(&wrapped, &url).await.unwrap_err().name, WireString::from("ExtraCaError"));
    *host.env.lock().unwrap() = None;
    assert!(fetch(&wrapped, &url).await.is_err());
    *host.env.lock().unwrap() = Some(std::fs::read_to_string(&pem).unwrap().replace('\n', "\\n").into());
    assert!(fetch(&wrapped, &url).await.is_ok(), "inline PEM literal escapes");
    let output = Command::new(&bun)
        .arg(&fixture)
        .arg("--original")
        .arg(&source)
        .arg(url.to_utf8().unwrap())
        .arg(&pem)
        .arg(&second_pem)
        .arg(second_url.to_utf8().unwrap())
        .output()
        .unwrap();
    assert!(output.status.success(), "original TLS client: {}", String::from_utf8_lossy(&output.stderr));
    let original: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        original,
        serde_json::json!({"untrusted":true,"trustedStatus":200,"payload":{"models":[]},"rotationOldRejected":true,"rotationNewStatus":200,"rotationPayload":{"models":[]},"missingName":"ExtraCaError"})
    );
    if let Some(path) = std::env::var_os("ARA_CATALOG_EXTRA_CA_TLS_RECEIPT") {
        std::fs::write(path,serde_json::to_vec_pretty(&serde_json::json!({"original":original,"native":{"constructionGate":true,"defaultDiscoveryTrusted":true,"rotationOldRejected":true,"rotationNewStatus":200,"plainVersionTransportUntrusted":true,"missingPath":"ExtraCaError","inlinePem":true},"sourceSha256":digest})).unwrap()).unwrap();
    }
}
