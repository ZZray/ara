//! Original Graph Context/request and actual HTTP/SSE producers.
//! Fixed OMP 596f2da (MIT); see THIRD_PARTY_NOTICES.md.
pub(crate) mod callbacks;
pub(crate) mod chain;
mod grammar;
mod history;
mod redaction;
mod request;
mod schema;
mod strict_schema;
#[cfg(test)]
pub(crate) use strict_schema::observe_b91_immutability_family;
use crate::actual_provider::GraphBuilder;
use crate::graph_values::GraphValueFactory;
pub use crate::responses_stream::actual::GraphResponsesFlavor;
use crate::responses_stream::actual::GraphResponsesPreparedRequest;
pub use crate::responses_stream::actual::{GraphResponsesOptions, ResponsesImageMetadataPort, ResponsesUsagePort};
use ara_rpc::value_heap::{ValueError, ValueHandle};
use std::sync::Arc;

/// Host owns diagnostic transport; schema decisions and rewriting stay in AI.
pub trait ResponsesQuarantinePort: Send + Sync {
    fn quarantined(
        &self,
        tool_name: &ValueHandle,
        path: &ara_rpc::WireString,
        factory: &GraphValueFactory,
    ) -> Result<(), ValueError>;
}
pub(crate) fn prepare_graph_responses_module(factory: &GraphValueFactory) -> Result<(), ValueError> {
    let ex = callbacks::Exec { heap: factory.heap(), runtime: factory.runtime() };
    redaction::prepare(factory.heap())?;
    schema::prepare(ex)?;
    strict_schema::prepare(ex)?;
    Ok(())
}

/// Borrowed facade for the same fixed OpenAI schema algorithms. Chat owns
/// request shaping; Responses protocol policy and algorithms are untouched.
pub(crate) fn adapt_chat_schema(
    execution: crate::graph_values::GraphExecution<'_>,
    source: &ValueHandle,
    strict: bool,
) -> Result<(ValueHandle, bool), ValueError> {
    strict_schema::adapt(callbacks::Exec { heap: execution.heap(), runtime: execution.runtime() }, source, strict)
}
pub(crate) fn chat_schema_violation(
    execution: crate::graph_values::GraphExecution<'_>,
    source: &ValueHandle,
    reject_root: bool,
) -> Result<Option<ara_rpc::WireString>, ValueError> {
    schema::violation(
        callbacks::Exec { heap: execution.heap(), runtime: execution.runtime() },
        source,
        &"#".into(),
        reject_root,
    )
}
/// Materialized catalog/protocol facts. No model-name or endpoint heuristics.
/// Defaults are the currently supported ARA request slice; full fixed catalog
/// resolution and these primitive bindings must be joined by the Host.
#[derive(Clone)]
pub struct GraphResponsesRequestPolicy {
    pub system_role_developer: bool,
    pub supports_image_detail_original: bool,
    pub strict_pairing: bool,
    pub repair_orphan_outputs: bool,
    pub preserve_assistant_message_ids: bool,
    pub developer_string_content: bool,
    pub filter_reasoning_history: bool,
    pub requires_reasoning_for_all_turns: bool,
    pub requires_reasoning_for_tool_calls: bool,
    pub escape_control_tokens: bool,
    /// Fixed configureCredentialRedaction default is false; Host materializes
    /// its secrets.enabled setting. The algorithm stays in portable AI.
    pub credential_redaction_enabled: bool,
    pub no_strict: bool,
    pub disable_strict_tools: bool,
    pub supports_strict_mode: bool,
    pub reject_root_object_union: bool,
    pub tool_schema_flavor: Option<ara_rpc::WireString>,
    pub on_quarantine: Option<Arc<dyn ResponsesQuarantinePort>>,
}
impl Default for GraphResponsesRequestPolicy {
    fn default() -> Self {
        Self {
            system_role_developer: false,
            supports_image_detail_original: false,
            strict_pairing: false,
            repair_orphan_outputs: false,
            preserve_assistant_message_ids: false,
            developer_string_content: false,
            filter_reasoning_history: false,
            requires_reasoning_for_all_turns: false,
            requires_reasoning_for_tool_calls: false,
            escape_control_tokens: false,
            credential_redaction_enabled: false,
            no_strict: false,
            disable_strict_tools: false,
            supports_strict_mode: true,
            reject_root_object_union: false,
            tool_schema_flavor: None,
            on_quarantine: None,
        }
    }
}
impl GraphResponsesRequestPolicy {
    pub fn supported_slice(model: &crate::Model, options: &super::RequestOptions) -> Self {
        Self {
            system_role_developer: model.reasoning,
            supports_image_detail_original: options
                .reasoning_policy
                .as_ref()
                .is_some_and(crate::OpenAIReasoningPolicy::supports_original_image_detail),
            repair_orphan_outputs: true,
            no_strict: options.no_strict,
            ..Self::default()
        }
    }
}
pub fn build_request(
    model: &crate::Model,
    context: &crate::GraphProviderContext,
    factory: Arc<GraphValueFactory>,
    options: &super::RequestOptions,
    policy: &GraphResponsesRequestPolicy,
    flavor: GraphResponsesFlavor,
) -> Result<ValueHandle, ValueError> {
    request::build(&GraphBuilder(factory), model, context, options, policy, flavor)
}
pub(crate) fn warm_if_replayable(
    session: &super::ProviderSessionState,
    model: &crate::Model,
    items: &ValueHandle,
    factory: Arc<GraphValueFactory>,
    hash: Option<Arc<dyn crate::actual_provider::HistoryIdHashPort>>,
) -> Result<(), ValueError> {
    let context = crate::GraphProviderContext {
        original_context: None,
        system_prompt: Vec::new(),
        messages: Vec::new(),
        tools: None,
        history_demotion_dialect: None,
        history_id_hash: hash,
        history_id_hash_function: None,
    };
    if history::replayable_native(&GraphBuilder(factory), &context, items)? {
        session.warm(&model.provider);
    }
    Ok(())
}
pub fn stream(
    client: reqwest::Client,
    model: crate::Model,
    context: crate::GraphProviderContext,
    factory: Arc<GraphValueFactory>,
    options: super::StreamOptions,
) -> Result<crate::ActualAssistantStream, ValueError> {
    let policy = GraphResponsesRequestPolicy::supported_slice(&model, &options.request);
    let graph = GraphResponsesOptions { history_id_hash: context.history_id_hash.clone(), ..Default::default() };
    stream_with_policy(client, model, context, factory, options, policy, graph)
}
pub fn stream_with_policy(
    client: reqwest::Client,
    model: crate::Model,
    context: crate::GraphProviderContext,
    factory: Arc<GraphValueFactory>,
    options: super::StreamOptions,
    policy: GraphResponsesRequestPolicy,
    graph: GraphResponsesOptions,
) -> Result<crate::ActualAssistantStream, ValueError> {
    context.check_lineage(&factory)?;
    let projection_model = model.clone();
    let projection_factory = factory.clone();
    let projection_options = options.clone();
    let project = Arc::new(move || {
        let mut request_options = projection_options.request.clone();
        if let Some(session) = &projection_options.session_state {
            request_options.native_history_replay = Some(session.is_warmed(&projection_model.provider));
        }
        let body = build_request(
            &projection_model,
            &context,
            projection_factory.clone(),
            &request_options,
            &policy,
            GraphResponsesFlavor::Compatible,
        )?;
        prepare_compatible(&projection_model, body, projection_factory.clone(), &projection_options)
    });
    crate::responses_stream::actual::stream_with_request_builder(
        client,
        model,
        project,
        factory,
        options,
        GraphResponsesFlavor::Compatible,
        graph,
    )
}
pub fn stream_codex(
    client: reqwest::Client,
    model: crate::Model,
    context: crate::GraphProviderContext,
    factory: Arc<GraphValueFactory>,
    options: super::super::openai_codex_responses::StreamOptions,
) -> Result<crate::ActualAssistantStream, ValueError> {
    let policy = GraphResponsesRequestPolicy::supported_slice(&model, &options.request);
    let graph = GraphResponsesOptions { history_id_hash: context.history_id_hash.clone(), ..Default::default() };
    stream_codex_with_policy(client, model, context, factory, options, policy, graph)
}
pub fn stream_codex_with_policy(
    client: reqwest::Client,
    model: crate::Model,
    context: crate::GraphProviderContext,
    factory: Arc<GraphValueFactory>,
    options: super::super::openai_codex_responses::StreamOptions,
    policy: GraphResponsesRequestPolicy,
    graph: GraphResponsesOptions,
) -> Result<crate::ActualAssistantStream, ValueError> {
    context.check_lineage(&factory)?;
    let projection_model = model.clone();
    let projection_factory = factory.clone();
    let projection_options = options.clone();
    let project = Arc::new(move || {
        let body = build_request(
            &projection_model,
            &context,
            projection_factory.clone(),
            &projection_options.request,
            &policy,
            GraphResponsesFlavor::Codex,
        )?;
        prepare_codex(&projection_model, body, projection_factory.clone(), &projection_options)
    });
    let common = super::StreamOptions {
        request: options.request,
        api_key: options.api_key,
        cancel: options.cancel,
        first_event_timeout: options.first_event_timeout,
        idle_timeout: options.idle_timeout,
        extra_headers: options.extra_headers,
        retry: options.retry,
        session_state: None,
        stateful_responses: false,
        on_response: options.on_response,
    };
    crate::responses_stream::actual::stream_with_request_builder(
        client,
        model,
        project,
        factory,
        common,
        GraphResponsesFlavor::Codex,
        graph,
    )
}
fn prepare_compatible(
    model: &crate::Model,
    body: ValueHandle,
    factory: Arc<GraphValueFactory>,
    options: &super::StreamOptions,
) -> Result<GraphResponsesPreparedRequest, ValueError> {
    let base = model.base_url.trim_end_matches('/');
    if base.is_empty() {
        return Err(ValueError::TypeError("OpenAI Responses request has no base URL".into()));
    }
    factory.heap().adopt(&body)?;
    let mut headers =
        vec![("Content-Type".into(), "application/json".into()), ("Accept".into(), "text/event-stream".into())];
    if let Some(key) = options.api_key.as_deref().filter(|key| !key.is_empty() && *key != "N/A") {
        headers.push(("Authorization".into(), format!("Bearer {key}")));
    }
    headers.extend(options.extra_headers.iter().cloned());
    Ok(GraphResponsesPreparedRequest { url: format!("{base}/responses"), headers, body })
}
pub fn stream_prepared(
    client: reqwest::Client,
    model: crate::Model,
    body: ValueHandle,
    factory: Arc<GraphValueFactory>,
    options: super::StreamOptions,
    graph_options: GraphResponsesOptions,
) -> Result<crate::ActualAssistantStream, ValueError> {
    let request = prepare_compatible(&model, body, factory.clone(), &options)?;
    crate::responses_stream::actual::stream_prepared(
        client,
        model,
        request,
        factory,
        options,
        GraphResponsesFlavor::Compatible,
        graph_options,
    )
}
pub(crate) fn prepare_codex(
    model: &crate::Model,
    body: ValueHandle,
    factory: Arc<GraphValueFactory>,
    options: &super::super::openai_codex_responses::StreamOptions,
) -> Result<GraphResponsesPreparedRequest, ValueError> {
    use super::super::openai_codex_responses::{
        ACCOUNT_HEADER, CLIENT_VERSION, RESIDENCY_HEADER, is_reserved_header, resolve_responses_url,
    };
    factory.heap().adopt(&body)?;
    let token = options
        .api_key
        .as_deref()
        .filter(|key| !key.trim().is_empty())
        .ok_or_else(|| ValueError::TypeError("Codex requires a host OAuth lease".into()))?;
    let accounts =
        options.extra_headers.iter().filter(|(name, _)| name.eq_ignore_ascii_case(ACCOUNT_HEADER)).collect::<Vec<_>>();
    if accounts.len() > 1 || accounts.first().is_some_and(|account| account.1.trim().is_empty()) {
        return Err(ValueError::TypeError("Codex accepts at most one nonempty account header from its host OAuth lease".into()));
    }
    for (name, _) in &options.extra_headers {
        if is_reserved_header(name)
            && !name.eq_ignore_ascii_case(ACCOUNT_HEADER)
            && !name.eq_ignore_ascii_case(RESIDENCY_HEADER)
        {
            return Err(ValueError::TypeError(format!("Codex protocol owns header {name}")));
        }
    }
    let mut headers = options.extra_headers.clone();
    if options.request.responses_lite {
        headers.push(("x-openai-internal-codex-responses-lite".into(), "true".into()));
    }
    headers.extend([
        ("Authorization".into(), format!("Bearer {token}")),
        ("OpenAI-Beta".into(), "responses=experimental".into()),
        ("originator".into(), "omp".into()),
        ("version".into(), CLIENT_VERSION.into()),
        ("x-codex-routing-hint".into(), format!("model={}", model.id)),
        ("accept".into(), "text/event-stream".into()),
        ("content-type".into(), "application/json".into()),
        ("user-agent".into(), concat!("ara/", env!("CARGO_PKG_VERSION")).into()),
    ]);
    if let Some(session) = options.session_id.as_deref().filter(|id| !id.is_empty()) {
        let b = GraphBuilder(factory);
        b.put(&body, "prompt_cache_key", b.text(options.request.prompt_cache_key.as_deref().unwrap_or(session))?)?;
        for name in ["session_id", "conversation_id", "x-client-request-id"] {
            headers.push((name.into(), session.into()));
        }
    }
    Ok(GraphResponsesPreparedRequest { url: resolve_responses_url(&model.base_url), headers, body })
}
pub fn stream_codex_prepared(
    client: reqwest::Client,
    model: crate::Model,
    body: ValueHandle,
    factory: Arc<GraphValueFactory>,
    options: super::super::openai_codex_responses::StreamOptions,
    graph_options: GraphResponsesOptions,
) -> Result<crate::ActualAssistantStream, ValueError> {
    let request = prepare_codex(&model, body, factory.clone(), &options)?;
    let common = super::StreamOptions {
        request: options.request,
        api_key: options.api_key,
        cancel: options.cancel,
        first_event_timeout: options.first_event_timeout,
        idle_timeout: options.idle_timeout,
        extra_headers: options.extra_headers,
        retry: options.retry,
        session_state: None,
        stateful_responses: false,
        on_response: options.on_response,
    };
    crate::responses_stream::actual::stream_prepared(
        client,
        model,
        request,
        factory,
        common,
        GraphResponsesFlavor::Codex,
        graph_options,
    )
}
