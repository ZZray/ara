//! Transient response notification port. Fixed OMP 596f2da7101178214aa27a753529d15e6b7ad91d,
//! `types.ts` and `utils/provider-response.ts` (MIT; THIRD_PARTY_NOTICES.md).
//! Response facts are delivered to the Host, never added to model events or journals.

use crate::{Model, ProviderError};
use futures::future::BoxFuture;
use serde_json::{Map, Value};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone)]
pub struct ProviderResponseMetadata {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    /// None = absent, Some(None) = an explicitly unknown request ID.
    pub request_id: Option<Option<String>>,
    pub metadata: Option<Map<String, Value>>,
}

impl ProviderResponseMetadata {
    pub fn from_response(response: &reqwest::Response) -> Self {
        let mut headers = BTreeMap::new();
        for name in response.headers().keys() {
            let values = response
                .headers()
                .get_all(name)
                .iter()
                .map(|value| value.as_bytes().iter().copied().map(char::from).collect::<String>())
                .collect::<Vec<_>>();
            headers.insert(name.as_str().to_ascii_lowercase(), values.join(", "));
        }
        let request_id = Some(headers.get("x-request-id").cloned());
        Self { status: response.status().as_u16(), headers, request_id, metadata: None }
    }
}

type Callback =
    dyn Fn(ProviderResponseMetadata, Option<Model>) -> BoxFuture<'static, Result<(), ProviderError>> + Send + Sync;

#[derive(Clone)]
pub struct ProviderResponseCallback(Arc<Callback>);

impl std::fmt::Debug for ProviderResponseCallback {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ProviderResponseCallback")
    }
}

impl ProviderResponseCallback {
    pub fn new(
        callback: impl Fn(ProviderResponseMetadata, Option<Model>) -> BoxFuture<'static, Result<(), ProviderError>>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self(Arc::new(callback))
    }

    pub async fn notify(&self, response: ProviderResponseMetadata, model: Option<Model>) -> Result<(), ProviderError> {
        (self.0)(response, model).await
    }

    /// Native AgentSession orders internal consumption before its configured callback.
    pub fn then(self, next: Option<Self>) -> Self {
        let Some(next) = next else { return self };
        Self::new(move |response, model| {
            let first = self.clone();
            let next = next.clone();
            Box::pin(async move {
                first.notify(response.clone(), model.clone()).await?;
                next.notify(response, model).await
            })
        })
    }
}

/// A callback failure follows a dispatched successful POST. Its error text
/// cannot establish an HTTP credential rejection or authorize model replay.
pub(crate) async fn notify_provider_response(
    callback: Option<&ProviderResponseCallback>,
    response: &reqwest::Response,
    model: &Model,
) -> Result<(), ProviderError> {
    if let Some(callback) = callback {
        callback
            .notify(ProviderResponseMetadata::from_response(response), Some(model.clone()))
            .await
            .map_err(|error| ProviderError::Config(format!("provider response callback failed: {error}")))?;
    }
    Ok(())
}
