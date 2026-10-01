//! Real files/mtime/root PEM, without mutating the shared process environment.
use ara_cli::{catalog_discovery::*, catalog_extra_ca::*};
use ara_rpc::WireString;
use async_trait::async_trait;
use std::{
    fs::{File, FileTimes},
    sync::{Arc, Mutex},
    time::{Duration, UNIX_EPOCH},
};

struct EnvironmentHost {
    env: Mutex<Option<WireString>>,
    native: NativeExtraCaHost,
}
impl ExtraCaHost for EnvironmentHost {
    fn node_extra_ca_certs(&self) -> Option<WireString> {
        self.env.lock().unwrap().clone()
    }
    fn file_mtime_ms(&self, path: &WireString) -> Result<f64, ExtraCaIoError> {
        self.native.file_mtime_ms(path)
    }
    fn read_utf8(&self, path: &WireString) -> Result<WireString, ExtraCaIoError> {
        self.native.read_utf8(path)
    }
    fn root_certificates(&self) -> Result<Vec<WireString>, DiscoveryError> {
        self.native.root_certificates()
    }
}
fn host(env: Option<WireString>) -> (Arc<EnvironmentHost>, Arc<ExtraCaRuntime>) {
    let host = Arc::new(EnvironmentHost { env: Mutex::new(env), native: NativeExtraCaHost });
    let runtime = Arc::new(ExtraCaRuntime::new(host.clone()));
    (host, runtime)
}
fn write_file(path: &std::path::Path, contents: &[u8], seconds: u64) {
    std::fs::write(path, contents).unwrap();
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(seconds)))
        .unwrap();
}
#[test]
fn real_file_rotation_reset_removal_and_utf8_replace() {
    let temporary = tempfile::tempdir().unwrap();
    let file = temporary.path().join("extensionless-ca");
    let file_env = file.to_str().unwrap().into();
    write_file(&file, b"first\n", 100);
    let (_, runtime) = host(Some(file_env));
    assert_eq!(runtime.resolve_extra_ca().unwrap(), Some("first\n".into()));
    write_file(&file, b"same-mtime-is-cached", 100);
    assert_eq!(runtime.resolve_extra_ca().unwrap(), Some("first\n".into()));
    write_file(&file, b"rotated\n", 101);
    assert_eq!(runtime.resolve_extra_ca().unwrap(), Some("rotated\n".into()));
    write_file(&file, b"after-reset\n", 101);
    runtime.reset_extra_ca_cache();
    assert_eq!(runtime.resolve_extra_ca().unwrap(), Some("after-reset\n".into()));
    write_file(&file, &[b'X', 0xff, b'Y'], 102);
    assert_eq!(runtime.resolve_extra_ca().unwrap(), Some("X\u{fffd}Y".into()));
    std::fs::remove_file(&file).unwrap();
    let error = runtime.resolve_extra_ca().unwrap_err();
    assert!(error.name.equals_ascii("ExtraCaError"));
    assert!(error.message.to_utf8().unwrap().ends_with(file.to_str().unwrap()));
}
#[test]
fn real_directory_failure_bubbles_and_inline_configuration_rotates() {
    let temporary = tempfile::tempdir().unwrap();
    let (environment, runtime) = host(Some(temporary.path().to_str().unwrap().into()));
    let error = runtime.resolve_extra_ca().unwrap_err();
    assert!(error.name.equals_ascii("Error"));
    assert!(error.message.to_utf8().unwrap().starts_with("EISDIR:"));
    *environment.env.lock().unwrap() =
        Some(" \u{feff}-----BEGIN CERTIFICATE-----\\nA\\n-----END CERTIFICATE-----\\n\u{a0} ".into());
    assert_eq!(
        runtime.resolve_extra_ca().unwrap(),
        Some("-----BEGIN CERTIFICATE-----\nA\n-----END CERTIFICATE-----\n".into())
    );
    *environment.env.lock().unwrap() = None;
    assert_eq!(runtime.resolve_extra_ca().unwrap(), None);
    *environment.env.lock().unwrap() = Some("-----BEGIN CERTIFICATE-----\\nB".into());
    assert_eq!(runtime.resolve_extra_ca().unwrap(), Some("-----BEGIN CERTIFICATE-----\nB".into()));
}
struct Capture {
    requests: Mutex<Vec<DiscoveryRequest>>,
}
#[async_trait]
impl DiscoveryTransport for Capture {
    fn context_id(&self) -> u64 {
        401
    }
    async fn fetch(&self, request: DiscoveryRequest) -> Result<DiscoveryReply, DiscoveryError> {
        self.requests.lock().unwrap().push(request);
        Ok(DiscoveryReply { status: 200, headers: Vec::new(), body: Vec::new(), json_override: None })
    }
}
#[tokio::test]
async fn construction_gate_and_native_root_pem_across_real_host_adapter() {
    let (environment, runtime) = host(None);
    let capture = Arc::new(Capture { requests: Mutex::default() });
    let transport: Arc<dyn DiscoveryTransport> = capture.clone();
    let unwrapped = wrap_fetch_for_extra_ca(runtime.clone(), transport.clone());
    assert!(Arc::ptr_eq(&unwrapped, &transport));
    *environment.env.lock().unwrap() =
        Some("-----BEGIN CERTIFICATE-----\\nextra-fixture\\n-----END CERTIFICATE-----".into());
    unwrapped.fetch(DiscoveryRequest::default()).await.unwrap();
    assert!(capture.requests.lock().unwrap()[0].tls.is_none());
    let roots = environment.root_certificates().unwrap();
    assert!(!roots.is_empty(), "actual host has no native CA roots");
    for root in &roots {
        assert!(!reqwest::Certificate::from_pem_bundle(root.to_utf8().unwrap().as_bytes()).unwrap().is_empty());
    }
    let wrapped = wrap_fetch_for_extra_ca(runtime, transport);
    assert!(wrapped.extra_ca_wrapped());
    wrapped.fetch(DiscoveryRequest::default()).await.unwrap();
    let requests = capture.requests.lock().unwrap();
    let ca = requests[1].tls.as_ref().unwrap().get("ca").unwrap().as_array().unwrap();
    assert_eq!(ca.len(), roots.len() + 1);
    for (value, root) in ca.iter().zip(&roots) {
        assert_eq!(value.as_string().unwrap(), root);
    }
}
