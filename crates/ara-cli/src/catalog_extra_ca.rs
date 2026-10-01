//! Host-owned extra CA lifecycle from fixed OMP 596f2da, utils/tls-fetch.ts.
//! Native trust roots come from the host OS; OMP Bun rootCertificates are a
//! runtime-owned root set. Explicit curated lists replace default roots.
//
// MIT License
// Copyright (c) 2025 Mario Zechner
// Copyright (c) 2025-2026 Can Bölük
// Copyright (c) 2026 Stencil Labs, Inc.
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use crate::{
    catalog_discovery::{
        DiscoveryError, DiscoveryReply, DiscoveryRequest, DiscoveryTransport, js_trim, next_transport_id, wire_contains,
    },
    model_collapse::VariantSpec,
};
use ara_rpc::{WireString, WireValue};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::{
    fmt,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
    time::UNIX_EPOCH,
};

#[path = "catalog_extra_ca_reference.rs"]
mod reference;
pub use reference::{ExtraCaObject, ExtraCaValue, with_extra_ca_init};

/// Config errors retain their source name and optional cause independently of
/// retryable transport failures. Resolution's ENOENT path supplies no cause.
#[derive(Debug, Clone)]
pub struct ExtraCaError {
    pub message: WireString,
    pub cause: Option<ExtraCaValue>,
}
impl ExtraCaError {
    pub fn new(message: impl Into<WireString>, cause: Option<ExtraCaValue>) -> Self {
        Self { message: message.into(), cause: cause.filter(|cause| !cause.is_undefined()) }
    }
    pub fn name(&self) -> &'static str {
        "ExtraCaError"
    }
}
impl fmt::Display for ExtraCaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&String::from_utf16_lossy(self.message.units()))
    }
}
impl std::error::Error for ExtraCaError {}
impl From<ExtraCaError> for DiscoveryError {
    fn from(error: ExtraCaError) -> Self {
        Self::named(error.name(), error.message)
    }
}

#[derive(Debug, Clone)]
pub struct ExtraCaIoError {
    pub code: WireString,
    pub error: DiscoveryError,
}

/// The host owns environment, filesystem and default trust store. The seam
/// keeps sync stat/read/cache behavior observable without changing process env.
pub trait ExtraCaHost: Send + Sync {
    fn node_extra_ca_certs(&self) -> Option<WireString>;
    fn file_mtime_ms(&self, path: &WireString) -> Result<f64, ExtraCaIoError>;
    fn read_utf8(&self, path: &WireString) -> Result<WireString, ExtraCaIoError>;
    fn root_certificates(&self) -> Result<Vec<WireString>, DiscoveryError>;
}

#[derive(Debug, Default)]
pub struct NativeExtraCaHost;
fn native_path(path: &WireString) -> PathBuf {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        PathBuf::from(std::ffi::OsString::from_wide(path.units()))
    }
    #[cfg(not(windows))]
    {
        PathBuf::from(String::from_utf16_lossy(path.units()))
    }
}
fn native_io_error(error: std::io::Error, path: &WireString, operation: &str) -> ExtraCaIoError {
    let (code, message) = match error.kind() {
        std::io::ErrorKind::NotFound => ("ENOENT", "no such file or directory"),
        std::io::ErrorKind::PermissionDenied => ("EACCES", "permission denied"),
        std::io::ErrorKind::IsADirectory => ("EISDIR", "illegal operation on a directory"),
        std::io::ErrorKind::NotADirectory => ("ENOTDIR", "not a directory"),
        _ => ("EIO", "input/output error"),
    };
    let mut units: Vec<u16> = format!("{code}: {message}, {operation} '").encode_utf16().collect();
    units.extend(path.units());
    units.push(u16::from(b'\''));
    ExtraCaIoError { code: code.into(), error: DiscoveryError::new(WireString::from_units(units)) }
}
impl ExtraCaHost for NativeExtraCaHost {
    fn node_extra_ca_certs(&self) -> Option<WireString> {
        std::env::var_os("NODE_EXTRA_CA_CERTS").map(|value| {
            #[cfg(windows)]
            {
                use std::os::windows::ffi::OsStrExt;
                WireString::from_units(value.encode_wide().collect())
            }
            #[cfg(not(windows))]
            {
                value.to_string_lossy().into_owned().into()
            }
        })
    }
    fn file_mtime_ms(&self, path: &WireString) -> Result<f64, ExtraCaIoError> {
        let modified = std::fs::metadata(native_path(path))
            .and_then(|metadata| metadata.modified())
            .map_err(|error| native_io_error(error, path, "stat"))?;
        Ok(match modified.duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.as_secs_f64() * 1000.0,
            Err(error) => -error.duration().as_secs_f64() * 1000.0,
        })
    }
    fn read_utf8(&self, path: &WireString) -> Result<WireString, ExtraCaIoError> {
        if std::fs::metadata(native_path(path)).is_ok_and(|metadata| metadata.is_dir()) {
            return Err(native_io_error(std::io::Error::from(std::io::ErrorKind::IsADirectory), path, "read"));
        }
        let bytes = std::fs::read(native_path(path)).map_err(|error| native_io_error(error, path, "open"))?;
        Ok(String::from_utf8_lossy(&bytes).into_owned().into())
    }
    fn root_certificates(&self) -> Result<Vec<WireString>, DiscoveryError> {
        static ROOTS: OnceLock<Result<Vec<WireString>, DiscoveryError>> = OnceLock::new();
        ROOTS
            .get_or_init(|| {
                let roots = rustls_native_certs::load_native_certs();
                if roots.certs.is_empty() && !roots.errors.is_empty() {
                    return Err(DiscoveryError::new(format!("Cannot load native CA roots: {:?}", roots.errors)));
                }
                Ok(roots
                    .certs
                    .into_iter()
                    .map(|certificate| {
                        let encoded = STANDARD.encode(certificate.as_ref());
                        let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
                        for chunk in encoded.as_bytes().chunks(64) {
                            pem.push_str(std::str::from_utf8(chunk).expect("base64 ASCII"));
                            pem.push('\n');
                        }
                        pem.push_str("-----END CERTIFICATE-----\n");
                        pem.into()
                    })
                    .collect())
            })
            .clone()
    }
}

#[derive(Default)]
struct ExtraCaCache {
    key: Option<WireString>,
    value: Option<WireString>,
}
pub struct ExtraCaRuntime {
    pub host: Arc<dyn ExtraCaHost>,
    cache: Mutex<ExtraCaCache>,
}
impl ExtraCaRuntime {
    pub fn new(host: Arc<dyn ExtraCaHost>) -> Self {
        Self { host, cache: Mutex::default() }
    }
    fn raw(&self) -> Option<WireString> {
        self.host.node_extra_ca_certs().map(|raw| js_trim(&raw)).filter(|raw| !raw.is_empty())
    }
    pub fn has_configured_extra_ca(&self) -> bool {
        self.raw().is_some()
    }
    pub fn reset_extra_ca_cache(&self) {
        *self.cache.lock().expect("extra CA cache poisoned") = ExtraCaCache::default();
    }
    pub fn resolve_extra_ca(&self) -> Result<Option<WireString>, DiscoveryError> {
        let Some(raw) = self.raw() else {
            return Ok(None);
        };
        let inline = wire_contains(&raw, "-----BEGIN");
        let mut key = raw.clone();
        if !inline && let Ok(mtime) = self.host.file_mtime_ms(&raw) {
            key.append_str("@");
            key.append_str(&if mtime.is_nan() {
                "NaN".into()
            } else if mtime == f64::INFINITY {
                "Infinity".into()
            } else if mtime == f64::NEG_INFINITY {
                "-Infinity".into()
            } else {
                WireValue::Number(mtime).stringify()
            });
        }
        let mut cache = self.cache.lock().expect("extra CA cache poisoned");
        if cache.key.as_ref() == Some(&key) {
            return Ok(cache.value.clone());
        }
        let value = if inline {
            let mut units = Vec::new();
            let mut offset = 0;
            while offset < raw.len() {
                if raw.units()[offset..].starts_with(&[u16::from(b'\\'), u16::from(b'n')]) {
                    units.push(u16::from(b'\n'));
                    offset += 2;
                } else {
                    units.push(raw.units()[offset]);
                    offset += 1;
                }
            }
            WireString::from_units(units)
        } else {
            self.host.read_utf8(&raw).map_err(|error| {
                if error.code.equals_ascii("ENOENT") {
                    let mut units: Vec<u16> = "NODE_EXTRA_CA_CERTS path does not exist: ".encode_utf16().collect();
                    units.extend(raw.units());
                    ExtraCaError::new(WireString::from_units(units), None).into()
                } else {
                    error.error
                }
            })?
        };
        cache.key = Some(key);
        cache.value = Some(value.clone());
        Ok(Some(value))
    }
}
pub fn default_extra_ca_runtime() -> Arc<ExtraCaRuntime> {
    static RUNTIME: OnceLock<Arc<ExtraCaRuntime>> = OnceLock::new();
    Arc::clone(RUNTIME.get_or_init(|| Arc::new(ExtraCaRuntime::new(Arc::new(NativeExtraCaHost)))))
}
pub fn reset_extra_ca_cache() {
    default_extra_ca_runtime().reset_extra_ca_cache();
}

pub fn with_extra_ca_tls(existing: Option<&VariantSpec>, extra_ca: &WireString, roots: &[WireString]) -> VariantSpec {
    let existing = existing.map_or(ExtraCaValue::Undefined, ExtraCaValue::from_variant);
    reference::with_extra_ca_tls_native(&existing, extra_ca, roots).to_variant()
}
/// Value-only convenience. Native callers use with_extra_ca_init so unknown
/// shared references remain observable across mutation and transport calls.
pub fn with_extra_ca_init_variant(
    init: Option<&VariantSpec>,
    extra_ca: &WireString,
    roots: &[WireString],
) -> VariantSpec {
    let init = init.map(ExtraCaValue::from_variant);
    with_extra_ca_init(init.as_ref(), extra_ca, roots).to_variant()
}

struct ExtraCaTransport {
    inner: Arc<dyn DiscoveryTransport>,
    runtime: Arc<ExtraCaRuntime>,
    identity: u64,
    preconnect: Option<u64>,
}
#[async_trait]
impl DiscoveryTransport for ExtraCaTransport {
    fn context_id(&self) -> u64 {
        self.identity
    }
    fn extra_ca_wrapped(&self) -> bool {
        true
    }
    fn preconnect_identity(&self) -> Option<u64> {
        self.preconnect
    }
    async fn fetch(&self, mut request: DiscoveryRequest) -> Result<DiscoveryReply, DiscoveryError> {
        if let Some(extra_ca) = self.runtime.resolve_extra_ca()?.filter(|value| !value.is_empty()) {
            let init = request.native_init.clone().unwrap_or_else(|| {
                request.tls.as_ref().map_or(ExtraCaValue::Undefined, |tls| {
                    ExtraCaValue::object(&[("tls", ExtraCaValue::from_variant(tls))])
                })
            });
            let roots = if init.get("tls").get("ca").is_undefined() {
                self.runtime.host.root_certificates()?
            } else {
                Vec::new()
            };
            let merged = with_extra_ca_init(Some(&init), &extra_ca, &roots);
            request.tls = Some(merged.get("tls").to_variant());
            request.native_init = Some(merged);
        }
        self.inner.fetch(request).await
    }
}
/// The gate is construction-time. An unwrapped fetch stays unwrapped if env
/// becomes configured later; an installed wrapper reevaluates every request.
pub fn wrap_fetch_for_extra_ca(
    runtime: Arc<ExtraCaRuntime>,
    fetch: Arc<dyn DiscoveryTransport>,
) -> Arc<dyn DiscoveryTransport> {
    if fetch.extra_ca_wrapped() || !runtime.has_configured_extra_ca() {
        return fetch;
    }
    let preconnect = fetch.preconnect_identity();
    Arc::new(ExtraCaTransport { inner: fetch, runtime, identity: next_transport_id(), preconnect })
}

#[derive(Clone)]
pub struct ExtraCaFetchOptions {
    /// Typed input fallback when fields has no fetch property. On returned
    /// options this is a convenience snapshot; fields remains authoritative.
    pub fetch: Option<Arc<dyn DiscoveryTransport>>,
    /// Native option container. Shallow copies retain unknown field handles.
    pub fields: ExtraCaValue,
}
pub fn with_extra_ca_fetch(
    runtime: Arc<ExtraCaRuntime>,
    options: Option<Arc<ExtraCaFetchOptions>>,
    global_fetch: Arc<dyn DiscoveryTransport>,
) -> Option<Arc<ExtraCaFetchOptions>> {
    if !runtime.has_configured_extra_ca() {
        return options;
    }
    let supplied = options.as_ref().map_or(ExtraCaValue::Undefined, |options| {
        options
            .fields
            .as_object()
            .and_then(|fields| fields.property("fetch"))
            .unwrap_or_else(|| options.fetch.clone().map_or(ExtraCaValue::Undefined, ExtraCaValue::Fetch))
    });
    let fetch = if let ExtraCaValue::Fetch(fetch) = &supplied { fetch.clone() } else { global_fetch };
    let wrapped = wrap_fetch_for_extra_ca(runtime, Arc::clone(&fetch));
    if !supplied.is_undefined() && Arc::ptr_eq(&wrapped, &fetch) {
        return options;
    }
    let fields = options.as_ref().map_or_else(ExtraCaObject::default, |options| options.fields.spread());
    fields.set("fetch", ExtraCaValue::Fetch(wrapped.clone()));
    Some(Arc::new(ExtraCaFetchOptions { fetch: Some(wrapped), fields: ExtraCaValue::Object(fields) }))
}
