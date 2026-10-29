//! ARA model layer: message model, stream protocol, validation and provider
//! adapters. Ported from OMP `packages/ai` at
//! 596f2da7101178214aa27a753529d15e6b7ad91d (MIT, see THIRD_PARTY_NOTICES.md).

pub mod actual_provider;
pub mod dialect;
pub mod error;
pub mod event;
mod graph_history;
pub mod graph_values;
pub mod harmony_leak;
pub mod json;
pub mod model_tokenizer;
pub mod native_error;
pub mod provider_metadata;
pub mod provider_dispatch;
pub mod provider_response;
pub mod providers;
pub mod reasoning;
pub mod relaxed_json;
pub mod remote_compaction;
pub mod remote_compaction_v2;
pub(crate) mod replay_safe_retry;
pub(crate) mod responses_sse;
pub(crate) mod responses_stream;
pub mod retry;
pub mod retry_classification;
pub mod retry_hint;
pub(crate) mod schema_draft;
pub(crate) mod schema_stamps;
pub mod schema_validator;
pub(crate) mod schema_wire;
pub mod sse;
pub mod thinking_loop;
pub mod tool_call_loop_guard;
pub mod tool_schema;
pub mod transform;
pub mod types;
pub(crate) mod usage_limit;
pub mod validation;

pub use actual_provider::GraphProviderContext;
pub use error::ProviderError;
pub use event::{
    ActualAssistantEvent, ActualAssistantStream, ActualStreamFailureReceipt, AssistantMessageEvent, AssistantStream,
    EventSink,
};
pub use model_tokenizer::{ModelTokenizer, resolve_known_claude_tokenizer};
pub use provider_metadata::*;
pub use provider_response::{ProviderResponseCallback, ProviderResponseMetadata};
pub use reasoning::{
    OPENAI_REASONING_COMPAT_KEYS, OpenAIReasoningPolicy, ReasoningEffort, ReasoningRequest, ReasoningSummary,
};
pub use schema_wire::{
    flatten_graph_exclusive_required_root_union, graph_ark_to_wire_schema, graph_tool_wire_schema, graph_tool_wire_schema_in, strip_graph_schema_descriptions_in, is_graph_ark_schema,
    normalize_graph_empty_schemas, strip_graph_schema_descriptions, strip_graph_tool_descriptions,
};
pub use tool_schema::{ToolSchemaError, tool_wire_schema};
pub use types::*;
pub use usage_limit::fallback_cooldown_ms;

use tokio_util::sync::CancellationToken;

/// Per-call request knobs passed by the agent loop to a provider.
#[derive(Clone, Debug, Default)]
pub struct CallOptions {
    pub cancel: CancellationToken,
    /// Per-call map replaces the Host configured provider limits.
    pub max_in_flight_requests: Option<provider_dispatch::ProviderLimits>,
    /// Capped requests await the original bounded transport drain.
    pub wait_for_terminal_drain: bool,
    pub tool_choice: Option<ToolChoice>,
    /// Clear inherited route choice when the call selects an owned dialect.
    pub suppress_native_tool_choice: bool,
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
    /// `Some` replaces the complete route-base reasoning request.
    pub reasoning: Option<ReasoningRequest>,
    /// Responses include items; Some replaces route defaults, including empty.
    pub include: Option<Vec<String>>,
    /// Explicit Codex-only Lite opt-in; other providers ignore this knob.
    pub responses_lite: Option<bool>,
    pub loop_guard: Option<thinking_loop::LoopGuardOptions>,
    pub on_response: Option<ProviderResponseCallback>,
}

/// A model transport port. Hosts bind real adapters; tests bind scripted ones.
/// Implementations must emit exactly one terminal `done`/`error` event and
/// must honour `options.cancel` by ending with an `aborted` error event.
pub trait ModelProvider: Send + Sync {
    fn stream(&self, model: &Model, context: &Context, options: CallOptions) -> AssistantStream;

    /// Production actual producer, explicitly paired with same-lineage Graph
    /// request projection and raw HTTP/SSE parsing. Unsupported providers never
    /// export the request to typed Context or import a typed stream as actual.
    fn stream_actual(
        &self,
        _model: &Model,
        _context: &GraphProviderContext,
        _factory: std::sync::Arc<graph_values::GraphValueFactory>,
        _options: CallOptions,
    ) -> Result<ActualAssistantStream, ara_rpc::value_heap::ValueError> {
        Err(ara_rpc::value_heap::ValueError::UnsupportedOperation("Actual production model provider"))
    }
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
        opts.tool_choice =
            if options.suppress_native_tool_choice { None } else { options.tool_choice.or(opts.tool_choice) };
        opts.max_tokens = options.max_tokens.or(opts.max_tokens);
        opts.temperature = options.temperature.or(opts.temperature);
        opts.reasoning = options.reasoning.or(opts.reasoning);
        opts.on_response = options.on_response.or(opts.on_response);
        opts.wait_for_terminal_drain = options.wait_for_terminal_drain;
        providers::openai_completions::stream(self.client.clone(), model.clone(), context.clone(), opts)
    }
    fn stream_actual(
        &self,
        model: &Model,
        context: &GraphProviderContext,
        factory: std::sync::Arc<graph_values::GraphValueFactory>,
        options: CallOptions,
    ) -> Result<ActualAssistantStream, ara_rpc::value_heap::ValueError> {
        let mut opts = self.base.clone();
        opts.cancel = options.cancel;
        opts.tool_choice =
            if options.suppress_native_tool_choice { None } else { options.tool_choice.or(opts.tool_choice) };
        opts.max_tokens = options.max_tokens.or(opts.max_tokens);
        opts.temperature = options.temperature.or(opts.temperature);
        opts.reasoning = options.reasoning.or(opts.reasoning);
        opts.on_response = options.on_response.or(opts.on_response);
        opts.wait_for_terminal_drain = options.wait_for_terminal_drain;
        providers::openai_completions::actual::stream(
            self.client.clone(),
            model.clone(),
            context.clone(),
            factory,
            opts,
        )
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
        base.tool_choice =
            if options.suppress_native_tool_choice { None } else { options.tool_choice.or(base.tool_choice) };
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
        base.request.tool_choice =
            if options.suppress_native_tool_choice { None } else { options.tool_choice.or(base.request.tool_choice) };
        base.request.max_tokens = options.max_tokens.or(base.request.max_tokens);
        base.request.temperature = options.temperature.or(base.request.temperature);
        base.request.reasoning = options.reasoning.or(base.request.reasoning);
        if let Some(include) = options.include {
            base.request.include = include;
        }
        base.on_response = options.on_response.or(base.on_response);
        providers::openai_responses::stream(self.client.clone(), model.clone(), context.clone(), base)
    }
    fn stream_actual(
        &self,
        model: &Model,
        context: &GraphProviderContext,
        factory: std::sync::Arc<graph_values::GraphValueFactory>,
        options: CallOptions,
    ) -> Result<ActualAssistantStream, ara_rpc::value_heap::ValueError> {
        let mut base = self.base.clone();
        base.cancel = options.cancel;
        base.request.tool_choice =
            if options.suppress_native_tool_choice { None } else { options.tool_choice.or(base.request.tool_choice) };
        base.request.max_tokens = options.max_tokens.or(base.request.max_tokens);
        base.request.temperature = options.temperature.or(base.request.temperature);
        base.request.reasoning = options.reasoning.or(base.request.reasoning);
        if let Some(include) = options.include {
            base.request.include = include;
        }
        base.on_response = options.on_response.or(base.on_response);
        providers::openai_responses::actual::stream(self.client.clone(), model.clone(), context.clone(), factory, base)
    }
}
