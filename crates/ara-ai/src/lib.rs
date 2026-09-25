//! ARA model layer: message model, stream protocol, validation and provider
//! adapters. Ported from OMP `packages/ai` at
//! 596f2da7101178214aa27a753529d15e6b7ad91d (MIT, see THIRD_PARTY_NOTICES.md).

pub mod error;
pub mod event;
pub mod json;
pub mod providers;
pub mod sse;
pub mod transform;
pub mod types;
pub mod validation;

pub use error::ProviderError;
pub use event::{AssistantMessageEvent, AssistantStream, EventSink};
pub use types::*;

use tokio_util::sync::CancellationToken;

/// Per-call request knobs passed by the agent loop to a provider.
#[derive(Clone, Debug, Default)]
pub struct CallOptions {
    pub cancel: CancellationToken,
    pub tool_choice: Option<ToolChoice>,
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
}

/// A model transport port. Hosts bind real adapters; tests bind scripted ones.
/// Implementations must emit exactly one terminal `done`/`error` event and
/// must honour `options.cancel` by ending with an `aborted` error event.
pub trait ModelProvider: Send + Sync {
    fn stream(&self, model: &Model, context: &Context, options: CallOptions) -> AssistantStream;
}

/// OpenAI-compatible Chat Completions binding.
pub struct OpenAICompletionsProvider {
    pub client: reqwest::Client,
    pub base: providers::openai_completions::StreamOptions,
}

impl ModelProvider for OpenAICompletionsProvider {
    fn stream(&self, model: &Model, context: &Context, options: CallOptions) -> AssistantStream {
        let mut opts = self.base.clone();
        opts.cancel = options.cancel;
        opts.tool_choice = options.tool_choice.or(opts.tool_choice);
        opts.max_tokens = options.max_tokens.or(opts.max_tokens);
        opts.temperature = options.temperature.or(opts.temperature);
        providers::openai_completions::stream(self.client.clone(), model.clone(), context.clone(), opts)
    }
}
