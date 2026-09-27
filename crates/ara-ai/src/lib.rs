//! ARA model layer: message model, stream protocol, validation and provider
//! adapters. Ported from OMP `packages/ai` at
//! 596f2da7101178214aa27a753529d15e6b7ad91d (MIT, see THIRD_PARTY_NOTICES.md).

pub mod error;
pub mod event;
pub mod json;
pub mod model_tokenizer;
pub mod providers;
pub(crate) mod replay_safe_retry;
pub(crate) mod responses_sse;
pub(crate) mod responses_stream;
pub(crate) mod schema_draft;
pub(crate) mod schema_wire;
pub mod sse;
pub mod transform;
pub mod types;
pub(crate) mod usage_limit;
pub mod validation;

pub use error::ProviderError;
pub use event::{AssistantMessageEvent, AssistantStream, EventSink};
pub use model_tokenizer::{ModelTokenizer, resolve_known_claude_tokenizer};
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

/// Anthropic Messages API-key binding.
pub struct AnthropicMessagesProvider {
    pub client: reqwest::Client,
    pub base: providers::anthropic::StreamOptions,
}

impl ModelProvider for AnthropicMessagesProvider {
    fn stream(&self, model: &Model, context: &Context, options: CallOptions) -> AssistantStream {
        let mut base = self.base.clone();
        base.cancel = options.cancel;
        base.tool_choice = options.tool_choice.or(base.tool_choice);
        base.max_tokens = options.max_tokens.or(base.max_tokens);
        base.temperature = options.temperature.or(base.temperature);
        providers::anthropic::stream(self.client.clone(), model.clone(), context.clone(), base)
    }
}

/// OpenAI-compatible stateless Responses binding.
pub struct OpenAIResponsesProvider {
    pub client: reqwest::Client,
    pub base: providers::openai_responses::StreamOptions,
}

impl ModelProvider for OpenAIResponsesProvider {
    fn stream(&self, model: &Model, context: &Context, options: CallOptions) -> AssistantStream {
        let mut base = self.base.clone();
        base.cancel = options.cancel;
        base.request.tool_choice = options.tool_choice.or(base.request.tool_choice);
        base.request.max_tokens = options.max_tokens.or(base.request.max_tokens);
        base.request.temperature = options.temperature.or(base.request.temperature);
        providers::openai_responses::stream(self.client.clone(), model.clone(), context.clone(), base)
    }
}
