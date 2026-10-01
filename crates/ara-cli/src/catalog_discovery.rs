//! Host-owned model discovery for fixed OMP 596f2da.
//!
//! Provider functions keep their own timeout, authentication and fallback rules.
//! Transport injection does not apply the default host's extra-CA policy.

use crate::model_collapse::{CollapseError, CollapseRuntime, SpecRef, VariantSpec};
use ara_rpc::{WireString, WireValue};
use async_trait::async_trait;
use icu_collator::{Collator, CollatorBorrowed, options::CollatorOptions};
use icu_locale_core::Locale;
use std::{
    cmp::Ordering,
    fmt,
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicU64, Ordering as AtomicOrdering},
    },
};
use tokio_util::sync::CancellationToken;

pub mod codex;
pub mod cursor;
pub mod devin;
pub mod gitlab;
pub mod google;
pub mod openai;

pub type DiscoveryResult = Result<Option<Vec<SpecRef>>, DiscoveryError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryError {
    pub name: WireString,
    pub message: WireString,
}
impl DiscoveryError {
    pub fn new(message: impl Into<WireString>) -> Self {
        Self { name: "Error".into(), message: message.into() }
    }
    pub fn named(name: impl Into<WireString>, message: impl Into<WireString>) -> Self {
        Self { name: name.into(), message: message.into() }
    }
}
impl fmt::Display for DiscoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&String::from_utf16_lossy(self.message.units()))
    }
}
impl std::error::Error for DiscoveryError {}
impl From<CollapseError> for DiscoveryError {
    fn from(value: CollapseError) -> Self {
        Self { name: value.name().into(), message: value.message_wire() }
    }
}

#[derive(Debug, Clone, Default)]
pub struct DiscoverySignal {
    state: Arc<DiscoverySignalState>,
}
#[derive(Debug, Default)]
struct DiscoverySignalState {
    token: CancellationToken,
    reason: Mutex<Option<DiscoveryError>>,
    dependents: Mutex<Vec<Weak<DiscoverySignalState>>>,
    // A live combined signal must retain intermediate source states; otherwise
    // a temporary nested any() disappears before its root parent can abort.
    // These edges point toward older states, while dependents remain weak.
    _parents: Vec<Arc<DiscoverySignalState>>,
}
impl DiscoverySignal {
    pub fn abort(&self, reason: DiscoveryError) {
        {
            let mut guard = self.state.reason.lock().expect("discovery signal poisoned");
            if guard.is_some() {
                return;
            }
            *guard = Some(reason.clone());
            self.state.token.cancel();
        }
        let dependents = std::mem::take(&mut *self.state.dependents.lock().expect("signal dependents poisoned"));
        for dependent in dependents {
            if let Some(state) = dependent.upgrade() {
                Self { state }.abort(reason.clone());
            }
        }
    }
    /// AbortSignal.any propagates synchronously and preserves the first reason.
    /// Weak registrations keep retained response signals observable without
    /// retaining either an abandoned child or an unbounded relay task.
    pub fn any(signals: &[Self]) -> Self {
        let combined = Self {
            state: Arc::new(DiscoverySignalState {
                _parents: signals.iter().map(|signal| Arc::clone(&signal.state)).collect(),
                ..Default::default()
            }),
        };
        for parent in signals {
            let reason = parent.state.reason.lock().expect("discovery signal poisoned");
            if let Some(reason) = &*reason {
                combined.abort(reason.clone());
                break;
            }
            let mut dependents = parent.state.dependents.lock().expect("signal dependents poisoned");
            dependents.retain(|dependent| dependent.strong_count() != 0);
            dependents.push(Arc::downgrade(&combined.state));
        }
        combined
    }
    pub fn is_aborted(&self) -> bool {
        self.state.token.is_cancelled()
    }
    pub fn reason(&self) -> Option<DiscoveryError> {
        self.state.reason.lock().expect("discovery signal poisoned").clone()
    }
    pub async fn cancelled(&self) -> DiscoveryError {
        self.state.token.cancelled().await;
        self.reason().unwrap_or_else(|| DiscoveryError::named("AbortError", "This operation was aborted."))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HttpMethod {
    #[default]
    Get,
    Post,
    Put,
    Delete,
    Head,
}

#[derive(Debug, Clone, Copy, Default)]
pub enum DiscoveryBodyPolicy {
    /// Discovery checks HTTP failure at the headers boundary, without waiting
    /// for a body it will never consume. Consumers needing error text opt in.
    #[default]
    Successful,
    Always,
}

#[derive(Debug, Clone)]
pub struct DiscoveryRequest {
    pub url: WireString,
    pub method: HttpMethod,
    pub headers: Vec<(WireString, WireString)>,
    pub body: Option<Vec<u8>>,
    pub signal: Option<DiscoverySignal>,
    pub http2: bool,
    pub body_policy: DiscoveryBodyPolicy,
    /// Native values are retained while the extra-CA wrapper changes only ca.
    pub tls: Option<VariantSpec>,
    /// Optional host native object graph for observable shallow-copy aliases.
    pub native_init: Option<crate::catalog_extra_ca::ExtraCaValue>,
}
impl Default for DiscoveryRequest {
    fn default() -> Self {
        Self {
            url: "".into(),
            method: HttpMethod::Get,
            headers: Vec::new(),
            body: None,
            signal: None,
            http2: false,
            body_policy: DiscoveryBodyPolicy::default(),
            tls: None,
            native_init: None,
        }
    }
}

/// Fetch Headers uses ByteString input and case-insensitive ordered entries.
/// Record-based request options remain raw until the native fetch boundary;
/// callers constructing Headers explicitly use this helper at construction.
#[derive(Debug, Clone, Default)]
pub struct DiscoveryHeaders {
    entries: Vec<(WireString, WireString)>,
}
impl DiscoveryHeaders {
    fn normalize(name: &WireString, value: &WireString) -> Result<(WireString, WireString), DiscoveryError> {
        let error = |prefix: &str, key: &WireString, middle: &str, value: Option<&WireString>| {
            let mut units: Vec<u16> = prefix.encode_utf16().collect();
            units.extend(key.units());
            units.extend(middle.encode_utf16());
            if let Some(value) = value {
                units.extend(value.units());
            }
            units.extend("'".encode_utf16());
            DiscoveryError::named("TypeError", WireString::from_units(units))
        };
        let name_error = || error("Invalid header name: '", name, "", None);
        let value_error = || error("Header '", name, "' has invalid value: '", Some(value));
        let name_bytes: Vec<u8> =
            name.units().iter().map(|&unit| u8::try_from(unit).map_err(|_| name_error())).collect::<Result<_, _>>()?;
        let normalized_name = reqwest::header::HeaderName::from_bytes(&name_bytes).map_err(|_| name_error())?;
        let value: Vec<u8> = value
            .units()
            .iter()
            .map(|&unit| u8::try_from(unit).map_err(|_| value_error()))
            .collect::<Result<_, _>>()?;
        let whitespace = |byte: u8| matches!(byte, b' ' | b'\t' | b'\r' | b'\n');
        let start = value.iter().position(|&byte| !whitespace(byte)).unwrap_or(value.len());
        let end = value.iter().rposition(|&byte| !whitespace(byte)).map_or(start, |index| index + 1);
        let value = &value[start..end];
        if value.iter().any(|byte| matches!(byte, 0 | b'\r' | b'\n')) {
            return Err(value_error());
        }
        Ok((
            normalized_name.as_str().into(),
            WireString::from_units(value.iter().map(|&byte| u16::from(byte)).collect()),
        ))
    }
    pub fn from_pairs(pairs: &[(WireString, WireString)]) -> Result<Self, DiscoveryError> {
        let mut headers = Self::default();
        for (key, value) in pairs {
            headers.append(key, value)?;
        }
        Ok(headers)
    }
    pub fn append(&mut self, key: &WireString, value: &WireString) -> Result<(), DiscoveryError> {
        let (key, value) = Self::normalize(key, value)?;
        if let Some((_, existing)) = self.entries.iter_mut().find(|(name, _)| name == &key) {
            let mut units = existing.units().to_vec();
            units.extend(", ".encode_utf16());
            units.extend(value.units());
            *existing = WireString::from_units(units);
        } else {
            self.entries.push((key, value));
        }
        Ok(())
    }
    pub fn set(&mut self, key: &WireString, value: &WireString) -> Result<(), DiscoveryError> {
        let (key, value) = Self::normalize(key, value)?;
        if let Some((_, existing)) = self.entries.iter_mut().find(|(name, _)| name == &key) {
            *existing = value;
        } else {
            self.entries.push((key, value));
        }
        Ok(())
    }
    pub fn get(&self, key: &WireString) -> Option<&WireString> {
        let key = key.to_utf8().ok()?.to_ascii_lowercase();
        self.entries.iter().find(|(name, _)| name.equals_ascii(&key)).map(|(_, value)| value)
    }
    pub fn entries(&self) -> Vec<(WireString, WireString)> {
        let mut entries = self.entries.clone();
        entries.sort_by(|a, b| a.0.units().cmp(b.0.units()));
        entries
    }
}

#[derive(Debug, Clone)]
pub struct DiscoveryReply {
    pub status: u16,
    pub headers: Vec<(WireString, WireString)>,
    pub body: Vec<u8>,
    /// An injected fetch's Response.json may return native values that cannot
    /// occur in JSON text. The production transport always leaves this absent.
    pub json_override: Option<Result<VariantSpec, DiscoveryError>>,
}
impl DiscoveryReply {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
    pub fn header(&self, name: &str) -> Option<&WireString> {
        self.headers
            .iter()
            .find(|(key, _)| key.to_utf8().is_ok_and(|s| s.eq_ignore_ascii_case(name)))
            .map(|(_, value)| value)
    }
    pub fn text(&self) -> WireString {
        // Fetch's text decoder removes an initial UTF-8 BOM and replaces invalid
        // sequences, before JSON parsing rather than after it.
        let bytes = self.body.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&self.body);
        String::from_utf8_lossy(bytes).into_owned().into()
    }
    pub fn json(&self) -> Result<VariantSpec, DiscoveryError> {
        if let Some(value) = &self.json_override {
            return value.clone();
        }
        let text = self.text().to_utf8().expect("decoded UTF-8 is valid");
        WireValue::parse(&text)
            .map(VariantSpec::from_wire)
            .map_err(|error| DiscoveryError::named("SyntaxError", error.to_string()))
    }
}

#[async_trait]
pub trait DiscoveryTransport: Send + Sync {
    /// Stable fetch-function identity for weakly scoped models.dev sessions.
    fn context_id(&self) -> u64;
    fn extra_ca_wrapped(&self) -> bool {
        false
    }
    fn preconnect_identity(&self) -> Option<u64> {
        None
    }
    async fn fetch(&self, request: DiscoveryRequest) -> Result<DiscoveryReply, DiscoveryError>;
}

static NEXT_TRANSPORT_ID: AtomicU64 = AtomicU64::new(1);
pub(crate) fn next_transport_id() -> u64 {
    NEXT_TRANSPORT_ID.fetch_add(1, AtomicOrdering::Relaxed)
}

#[derive(Clone)]
pub struct NativeDiscoveryTransport {
    extra_ca: Option<Vec<u8>>,
    identity: u64,
}
impl NativeDiscoveryTransport {
    pub fn with_extra_ca(pem: Option<&[u8]>) -> Result<Self, DiscoveryError> {
        Ok(Self { extra_ca: pem.map(<[u8]>::to_vec), identity: next_transport_id() })
    }
}
#[async_trait]
impl DiscoveryTransport for NativeDiscoveryTransport {
    fn context_id(&self) -> u64 {
        self.identity
    }
    async fn fetch(&self, request: DiscoveryRequest) -> Result<DiscoveryReply, DiscoveryError> {
        // Keep fetch decompression and redirect policy local to discovery.
        // Cargo unifies reqwest features across clients, so enabling global
        // decoders would change unrelated model-provider requests.
        crate::catalog_tls::fetch(self.extra_ca.as_deref(), request).await
    }
}

pub struct CatalogCollation {
    inner: CollatorBorrowed<'static>,
}
impl CatalogCollation {
    pub fn for_locale(name: &str) -> Result<Self, DiscoveryError> {
        let locale: Locale =
            name.parse().map_err(|error| DiscoveryError::new(format!("Invalid catalog locale: {error}")))?;
        let inner = Collator::try_new(locale.into(), CollatorOptions::default())
            .map_err(|error| DiscoveryError::new(error.to_string()))?;
        Ok(Self { inner })
    }
    pub fn for_host() -> Result<Self, DiscoveryError> {
        // Match the existing tested Bun host-locale selection at the read tool.
        #[cfg(target_os = "linux")]
        let locale = "en-US".to_owned();
        #[cfg(not(target_os = "linux"))]
        let locale = sys_locale::get_locale().unwrap_or_else(|| "en-US".to_owned());
        Self::for_locale(&locale)
    }
    pub fn compare(&self, left: &WireString, right: &WireString) -> Ordering {
        self.inner.compare_utf16(left.units(), right.units())
    }
}

pub struct DiscoveryRuntime {
    pub collapse: Mutex<CollapseRuntime>,
    pub google_headers: Arc<google::GoogleHeaderRuntime>,
    pub diagnostics: Mutex<Vec<DiscoveryLog>>,
}
#[derive(Debug, Clone)]
pub struct DiscoveryLog {
    pub level: WireString,
    pub arguments: Vec<VariantSpec>,
}
impl DiscoveryRuntime {
    pub fn new() -> Result<Self, DiscoveryError> {
        Ok(Self {
            collapse: Mutex::new(CollapseRuntime::new()?),
            google_headers: Arc::new(google::GoogleHeaderRuntime::for_host()),
            diagnostics: Mutex::new(Vec::new()),
        })
    }
}
#[derive(Clone)]
pub struct CatalogContext {
    pub transport: Arc<dyn DiscoveryTransport>,
    pub runtime: Arc<DiscoveryRuntime>,
    pub locale: Arc<CatalogCollation>,
    /// The version manifest's source default is plain global fetch, while
    /// default model discovery uses the construction-gated extra-CA wrapper.
    pub version_transport: Option<Arc<dyn DiscoveryTransport>>,
    /// Cursor opens node:http2 directly. Its process-startup trust snapshot
    /// must not pass through the dynamically evaluated fetch CA wrapper.
    pub http2_transport: Option<Arc<dyn DiscoveryTransport>>,
}

static PROCESS_STARTUP_EXTRA_CA: OnceLock<Option<Vec<u8>>> = OnceLock::new();

/// Call at the host's process entry, before host code can change its env.
/// Like Bun's native H2 startup handling, the raw value is a filename: no
/// trimming or inline PEM expansion. Missing/invalid files do not reject H2C.
pub fn initialize_catalog_process_startup_tls() {
    PROCESS_STARTUP_EXTRA_CA.get_or_init(|| {
        let path = std::env::var_os("NODE_EXTRA_CA_CERTS")?;
        let pem = std::fs::read(path).ok()?;
        let certificates = reqwest::Certificate::from_pem_bundle(&pem).ok()?;
        (!certificates.is_empty()).then_some(pem)
    });
}
impl CatalogContext {
    pub fn for_host() -> Result<Self, DiscoveryError> {
        Self::for_host_with_extra_ca_runtime(crate::catalog_extra_ca::default_extra_ca_runtime())
    }
    pub fn for_host_with_extra_ca_runtime(
        runtime: Arc<crate::catalog_extra_ca::ExtraCaRuntime>,
    ) -> Result<Self, DiscoveryError> {
        initialize_catalog_process_startup_tls();
        Self::for_host_with_startup_extra_ca(runtime, PROCESS_STARTUP_EXTRA_CA.get().and_then(Option::as_deref))
    }
    /// Hosts with their own process lifecycle supply the already captured CA
    /// bytes here. Runtime extra-CA changes affect fetch, never this H2 seam.
    pub fn for_host_with_startup_extra_ca(
        runtime: Arc<crate::catalog_extra_ca::ExtraCaRuntime>,
        startup_pem: Option<&[u8]>,
    ) -> Result<Self, DiscoveryError> {
        let plain: Arc<dyn DiscoveryTransport> = Arc::new(
            NativeDiscoveryTransport::with_extra_ca(startup_pem)
                .or_else(|_| NativeDiscoveryTransport::with_extra_ca(None))?,
        );
        let wrapped = crate::catalog_extra_ca::wrap_fetch_for_extra_ca(runtime, Arc::clone(&plain));
        let mut context = Self::new(wrapped)?;
        context.version_transport = Some(Arc::clone(&plain));
        context.http2_transport = Some(plain);
        Ok(context)
    }
    pub fn new(transport: Arc<dyn DiscoveryTransport>) -> Result<Self, DiscoveryError> {
        Ok(Self {
            transport,
            runtime: Arc::new(DiscoveryRuntime::new()?),
            locale: Arc::new(CatalogCollation::for_host()?),
            version_transport: None,
            http2_transport: None,
        })
    }
    pub fn http2_transport(&self) -> &Arc<dyn DiscoveryTransport> {
        self.http2_transport.as_ref().unwrap_or(&self.transport)
    }
    pub fn sort_by_id(&self, models: &mut [SpecRef]) {
        models.sort_by(|left, right| self.locale.compare(&model_string(left, "id"), &model_string(right, "id")));
    }
    pub fn sort_by_name(&self, models: &mut [SpecRef]) {
        models.sort_by(|left, right| {
            self.locale
                .compare(&model_string(left, "name"), &model_string(right, "name"))
                .then_with(|| self.locale.compare(&model_string(left, "id"), &model_string(right, "id")))
        });
    }
    pub fn log(&self, level: impl Into<WireString>, arguments: Vec<VariantSpec>) {
        self.runtime
            .diagnostics
            .lock()
            .expect("discovery diagnostics poisoned")
            .push(DiscoveryLog { level: level.into(), arguments });
    }
    pub fn warn(&self, message: impl Into<WireString>, fields: VariantSpec) {
        self.log("warn", vec![VariantSpec::from_wire(wire_string(message)), fields]);
    }
}

pub fn model_string(model: &VariantSpec, key: &str) -> WireString {
    model.get(key).and_then(WireValue::as_string).cloned().unwrap_or_else(|| "".into())
}
pub fn wire_object(entries: &[(&str, WireValue)]) -> WireValue {
    WireValue::Object(entries.iter().map(|(key, value)| ((*key).into(), value.clone())).collect())
}
pub fn wire_string(value: impl Into<WireString>) -> WireValue {
    WireValue::String(value.into())
}
pub fn js_trim(value: &WireString) -> WireString {
    fn whitespace(unit: u16) -> bool {
        matches!(unit, 0x0009..=0x000d | 0x0020 | 0x00a0 | 0x1680 | 0x2000..=0x200a | 0x2028..=0x2029 | 0x202f | 0x205f | 0x3000 | 0xfeff)
    }
    let units = value.units();
    let start = units.iter().position(|&unit| !whitespace(unit)).unwrap_or(units.len());
    let end = units.iter().rposition(|&unit| !whitespace(unit)).map_or(start, |index| index + 1);
    WireString::from_units(units[start..end].to_vec())
}
pub fn trim_trailing_slashes(value: &WireString) -> WireString {
    let end = value.units().iter().rposition(|&unit| unit != u16::from(b'/')).map_or(0, |index| index + 1);
    WireString::from_units(value.units()[..end].to_vec())
}
pub fn zero_cost() -> WireValue {
    wire_object(&[
        ("input", WireValue::Number(0.0)),
        ("output", WireValue::Number(0.0)),
        ("cacheRead", WireValue::Number(0.0)),
        ("cacheWrite", WireValue::Number(0.0)),
    ])
}

pub fn js_lower(value: &WireString) -> WireString {
    let mut units = Vec::new();
    for character in char::decode_utf16(value.units().iter().copied()) {
        match character {
            Ok(character) => {
                for lower in character.to_lowercase() {
                    let mut buffer = [0; 2];
                    units.extend_from_slice(lower.encode_utf16(&mut buffer));
                }
            }
            Err(error) => units.push(error.unpaired_surrogate()),
        }
    }
    WireString::from_units(units)
}
pub fn wire_contains(value: &WireString, ascii: &str) -> bool {
    let needle: Vec<u16> = ascii.encode_utf16().collect();
    needle.is_empty() || value.units().windows(needle.len()).any(|candidate| candidate == needle)
}

pub fn bundled_references(provider: &WireString) -> std::collections::HashMap<WireString, SpecRef> {
    crate::model_identity::bundled_model_list()
        .iter()
        .filter(|row| row["provider"].as_str().is_some_and(|id| provider.equals_ascii(id)))
        .map(|row| {
            let mut reference = VariantSpec::from_json(row);
            reference.remove("compat");
            reference.remove("supportsComputerUse");
            let configured = reference.get("supportsComputerUseConfig").cloned();
            reference.remove("supportsComputerUseConfig");
            if let Some(value) = configured {
                reference.set("supportsComputerUse", value);
            }
            let compat = reference.get("compatConfig").cloned();
            reference.remove("compatConfig");
            if let Some(value) = compat {
                reference.set("compat", value);
            } else {
                reference.set_undefined("compat");
            }
            (model_string(&reference, "id"), Arc::new(reference))
        })
        .collect()
}
