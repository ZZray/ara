//! Shared models.dev revalidation sessions from fixed OMP openai-compat.ts.
use super::{common::start_discovery_timeout, descriptor_types::ProviderFactoryHost};
use crate::{
    catalog_discovery::{CatalogContext, DiscoveryError, DiscoveryRequest, DiscoverySignal, DiscoveryTransport},
    model_collapse::VariantSpec,
};
use ara_rpc::{WireString, WireValue};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::Notify;
pub const MODELS_DEV_URL: &str = "https://catalog.stencil.so/models.json.zstd";
#[derive(Default)]
struct SessionState {
    inflight: Option<Arc<Flight>>,
    payload: Option<VariantSpec>,
    etag: Option<WireString>,
}
#[derive(Default)]
struct Session {
    state: Mutex<SessionState>,
}
#[derive(Default)]
struct Flight {
    result: Mutex<Option<Result<VariantSpec, DiscoveryError>>>,
    notify: Notify,
}
type FetchSessions = HashMap<usize, (Weak<dyn DiscoveryTransport>, Arc<Session>)>;
#[derive(Default)]
pub struct CatalogSessions {
    default: Arc<Session>,
    sessions: Mutex<FetchSessions>,
}
impl CatalogSessions {
    fn get(&self, context: &CatalogContext, explicit: bool) -> Arc<Session> {
        if !explicit {
            return self.default.clone();
        }
        let mut sessions = self.sessions.lock().expect("catalog sessions poisoned");
        sessions.retain(|_, (transport, _)| transport.strong_count() > 0);
        sessions
            .entry(Arc::as_ptr(&context.transport) as *const () as usize)
            .or_insert_with(|| (Arc::downgrade(&context.transport), Arc::new(Session::default())))
            .1
            .clone()
    }
}
pub async fn fetch_revalidated_well_known_models(
    context: &CatalogContext,
    host: &ProviderFactoryHost,
    explicit: bool,
    signal: Option<DiscoverySignal>,
) -> Result<VariantSpec, DiscoveryError> {
    let session = host.sessions.get(context, explicit);
    let flight = {
        let mut state = session.state.lock().expect("catalog state poisoned");
        if let Some(flight) = &state.inflight {
            flight.clone()
        } else {
            let flight = Arc::new(Flight::default());
            state.inflight = Some(flight.clone());
            let session = session.clone();
            let context = context.clone();
            let user_agent = host.user_agent.clone();
            let running = flight.clone();
            tokio::spawn(async move {
                let result = fetch_payload(&context, &session, user_agent).await;
                *running.result.lock().expect("catalog result poisoned") = Some(result);
                session.state.lock().expect("catalog state poisoned").inflight = None;
                running.notify.notify_waiters();
            });
            flight
        }
    };
    let wait = async {
        loop {
            let notified = flight.notify.notified();
            if let Some(result) = flight.result.lock().expect("catalog result poisoned").clone() {
                return result;
            }
            notified.await;
        }
    };
    if let Some(signal) = signal {
        tokio::select! {biased;reason=signal.cancelled()=>Err(reason),result=wait=>result}
    } else {
        wait.await
    }
}
pub async fn fetch_well_known_models(
    context: &CatalogContext,
    host: &ProviderFactoryHost,
    explicit: bool,
    signal: Option<DiscoverySignal>,
) -> Result<VariantSpec, DiscoveryError> {
    match fetch_revalidated_well_known_models(context, host, explicit, signal).await {
        Ok(payload) => Ok(payload),
        Err(error) => host
            .sessions
            .get(context, explicit)
            .state
            .lock()
            .expect("catalog state poisoned")
            .payload
            .clone()
            .ok_or(error),
    }
}
pub async fn fetch_revalidated_with_timeout(
    context: &CatalogContext,
    host: &ProviderFactoryHost,
    explicit: bool,
    milliseconds: f64,
) -> Result<VariantSpec, DiscoveryError> {
    let signal = DiscoverySignal::default();
    let timer = start_discovery_timeout(&signal, milliseconds);
    let result = fetch_revalidated_well_known_models(context, host, explicit, Some(signal)).await;
    drop(timer);
    result
}
async fn fetch_payload(
    context: &CatalogContext,
    session: &Session,
    user_agent: WireString,
) -> Result<VariantSpec, DiscoveryError> {
    let mut headers =
        vec![("Accept".into(), "application/zstd, application/json".into()), ("User-Agent".into(), user_agent)];
    {
        let state = session.state.lock().expect("catalog state poisoned");
        if state.payload.is_some()
            && let Some(etag) = state.etag.as_ref().filter(|s| !s.is_empty())
        {
            headers.push(("If-None-Match".into(), etag.clone()));
        }
    }
    let signal = DiscoverySignal::default();
    let timer = start_discovery_timeout(&signal, 10000.0);
    let response = context
        .transport
        .fetch(DiscoveryRequest { url: MODELS_DEV_URL.into(), headers, signal: Some(signal), ..Default::default() })
        .await?;
    drop(timer);
    if response.status == 304
        && let Some(payload) = session.state.lock().expect("catalog state poisoned").payload.clone()
    {
        return Ok(payload);
    }
    if !response.ok() {
        return Err(DiscoveryError::new(format!("models catalog fetch failed: {}", response.status)));
    }
    let bytes = if response.body.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        zstd::stream::decode_all(response.body.as_slice()).map_err(|error| DiscoveryError::new(error.to_string()))?
    } else {
        response.body
    };
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
    let text = String::from_utf8_lossy(bytes);
    let payload = VariantSpec::from_wire(
        WireValue::parse(&text).map_err(|error| DiscoveryError::named("SyntaxError", error.to_string()))?,
    );
    let mut state = session.state.lock().expect("catalog state poisoned");
    state.payload = Some(payload.clone());
    state.etag = response
        .headers
        .iter()
        .find(|(key, _)| key.to_utf8().is_ok_and(|s| s.eq_ignore_ascii_case("etag")))
        .map(|(_, value)| value.clone());
    Ok(payload)
}
