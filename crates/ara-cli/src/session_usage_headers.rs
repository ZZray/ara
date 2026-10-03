//! Reference Host consumption of provider response facts. Fixed OMP
//! `session/session-stats.ts:387-395` at 596f2da7101178214aa27a753529d15e6b7ad91d
//! (MIT; THIRD_PARTY_NOTICES.md). Account selection happens at callback time
//! against the logical Session, rather than against its request lease.

use crate::auth_storage::AuthStorage;
use ara_ai::{
    AssistantStream, CallOptions, Context, Model, ModelProvider, ProviderError, ProviderResponseCallback,
    ProviderResponseMetadata,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct SessionUsageHeaders {
    storage: AuthStorage,
    session_id: Option<String>,
    base_url: Option<String>,
    registry: Option<crate::model_registry::ModelRegistry>,
}

impl SessionUsageHeaders {
    pub fn new(storage: AuthStorage, session_id: Option<String>, base_url: Option<String>) -> Self {
        Self { storage, session_id, base_url, registry: None }
    }

    pub fn with_registry(mut self, registry: crate::model_registry::ModelRegistry) -> Self {
        self.registry = Some(registry);
        self
    }

    pub fn for_session(&self, session_id: &str) -> Self {
        Self { session_id: Some(session_id.to_owned()), ..self.clone() }
    }

    pub fn ingest_provider_usage_headers(
        &self,
        response: &ProviderResponseMetadata,
        model: Option<&Model>,
    ) -> Result<(), ProviderError> {
        let Some(model) = model.filter(|model| !model.provider.is_empty()) else { return Ok(()) };
        let base_url = if let Some(registry) = &self.registry {
            registry
                .get_provider_base_url(&model.provider.as_str().into())
                .map_err(|_| ProviderError::Config("provider usage URL lookup failed".into()))?
                .map(|url| url.to_utf8().map_err(|_| ProviderError::Config("provider usage URL is not UTF-8".into())))
                .transpose()?
        } else {
            self.base_url.clone()
        };
        self.storage
            .ingest_usage_headers(&model.provider, &response.headers, self.session_id.as_deref(), base_url.as_deref())
            .map(|_| ())
            .map_err(|_| ProviderError::Config("provider usage header storage failed".into()))
    }

    pub fn callback(&self) -> ProviderResponseCallback {
        let owner = self.clone();
        ProviderResponseCallback::new(move |response, model| {
            let owner = owner.clone();
            Box::pin(async move { owner.ingest_provider_usage_headers(&response, model.as_ref()) })
        })
    }

    pub(crate) fn attach(&self, options: &mut CallOptions) {
        options.on_response = Some(self.callback().then(options.on_response.take()));
    }

    /// Host binding for the existing custom ModelProvider port (native streamFn).
    /// The provider decides whether and when it has observed a response.
    pub fn bind(&self, provider: Arc<dyn ModelProvider>) -> Arc<dyn ModelProvider> {
        Arc::new(SessionResponseProvider { owner: self.clone(), provider })
    }
}

struct SessionResponseProvider {
    owner: SessionUsageHeaders,
    provider: Arc<dyn ModelProvider>,
}
impl ModelProvider for SessionResponseProvider {
    fn stream(&self, model: &Model, context: &Context, mut options: CallOptions) -> AssistantStream {
        self.owner.attach(&mut options);
        self.provider.stream(model, context, options)
    }
}
