//! Host binding for native-tool actual OpenAI provider contexts.
//! RPC Session registration is owned by the Host; this adapter holds no
//! SessionValues, runtime, transcript or transport registry between calls.
use ara_agent::{
    ActualProviderContext, ActualProviderError, OwnedProviderContextPort, ProviderContextError, SessionValues,
};
use ara_ai::{
    ActualAssistantStream, AssistantStream, CallOptions, Context, GraphProviderContext, Model, ModelProvider,
};
use ara_rpc::value_heap::{
    CompiledFunctionForm, CompiledFunctionSpec, ValueError, ValueHandle, ValueHeap, ValueRuntime, ValueSnapshot,
};
use std::sync::Arc;

#[derive(Debug)]
struct NativeHistoryIdHash;
impl ara_ai::actual_provider::HistoryIdHashPort for NativeHistoryIdHash {
    fn hash_string_base36(
        &self,
        value: &ara_rpc::WireString,
    ) -> Result<ara_rpc::WireString, ara_rpc::value_heap::ValueError> {
        Ok(crate::bun_hash::hash_string_base36(value).into())
    }
}

fn native_history_hash(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    arguments: &[ValueHandle],
    _: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let Some(source) = arguments.first() else {
        return Err(ValueError::UnsupportedOperation("Fixed Bun.hash string input is missing"));
    };
    let ValueSnapshot::String(source) = heap.snapshot(source)? else {
        return Err(ValueError::UnsupportedOperation("Fixed Bun.hash binding supports the generated string seed"));
    };
    heap.bigint_decimal(&crate::bun_hash::hash_string(&source).to_string())
}

static NATIVE_HISTORY_HASH: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "ara-cli.bun-1.4.0.hash.string",
    name: "hash",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: native_history_hash,
};

pub(crate) fn native_history_hash_function(values: &SessionValues) -> Result<ValueHandle, ValueError> {
    values.heap().module_compiled_function("ara-cli.fixed-bun", "hash.string", &NATIVE_HISTORY_HASH)
}

/// Host resolves explicit configuration/env precedence. Model classification
/// runs here only when the Host explicitly selects the automatic policy.
#[derive(Clone, Copy, Debug)]
pub enum ActualOwnedDialectSelector {
    Explicit(ara_ai::dialect::Dialect),
    ModelPreferred,
}

struct OwnedProjectionBinding {
    selector: ActualOwnedDialectSelector,
    json: ValueHandle,
    date: ValueHandle,
}

#[derive(Default)]
pub struct ActualNativeProviderPort {
    projection: Option<OwnedProjectionBinding>,
    owned_stream_globals: Option<ValueHandle>,
    abort_on_fabrication: Option<bool>,
}
impl ActualNativeProviderPort {
    /// Retain only same-realm global objects, never a Session/runtime owner.
    /// Default ports serve native requests and reject unbound owned projection.
    pub fn with_owned_projection(selector: ActualOwnedDialectSelector, json: ValueHandle, date: ValueHandle) -> Self {
        Self { projection: Some(OwnedProjectionBinding { selector, json, date }), ..Self::default() }
    }
    /// Current same-realm global environment supplies the original String
    /// binding at the source consumer, including later Host replacement.
    pub fn with_owned_stream_globals(mut self, globals: ValueHandle) -> Self {
        self.owned_stream_globals = Some(globals);
        self
    }
    pub fn with_abort_on_fabrication(mut self, abort: bool) -> Self {
        self.abort_on_fabrication = Some(abort);
        self
    }
}
impl OwnedProviderContextPort for ActualNativeProviderPort {
    fn project_context_actual(
        &self,
        _model: &Model,
        context: ActualProviderContext,
        _side_request: bool,
        values: &SessionValues,
    ) -> Result<ActualProviderContext, ActualProviderError> {
        context.check_lineage(values)?;
        Err(ActualProviderError::Unsupported("native provider port has no owned-dialect Graph encoder"))
    }
    fn project_context_original_actual(
        &self,
        model: &Model,
        mut context: ActualProviderContext,
        original_tools: ValueHandle,
        _side_request: bool,
        values: &SessionValues,
    ) -> Result<ActualProviderContext, ActualProviderError> {
        context.check_lineage(values)?;
        values.check_handle(&original_tools)?;
        let binding = self
            .projection
            .as_ref()
            .ok_or(ActualProviderError::Unsupported("Host-owned dialect selection and original JSON/Date globals"))?;
        values.check_handle(&binding.json)?;
        values.check_handle(&binding.date)?;
        let dialect = match binding.selector {
            ActualOwnedDialectSelector::Explicit(dialect) => dialect,
            ActualOwnedDialectSelector::ModelPreferred => ara_ai::actual_provider::graph_history_demotion_dialect(
                crate::model_identity::preferred_dialect(&model.id),
            )
            .ok_or(ActualProviderError::Unsupported("Host preferred owned dialect"))?,
        };
        let original = context
            .original_context
            .as_ref()
            .ok_or(ActualProviderError::Unsupported("post-hook original provider Context"))?;
        let projected = ara_ai::dialect::actual_projection::project_owned_context(
            values.factory(),
            original,
            &original_tools,
            dialect,
            &binding.json,
            &binding.date,
        )?;
        // Compatibility Vec fields are observations. Do not re-Get projected
        // fields or reconstruct original messages merely to refresh them.
        context.original_context = Some(projected);
        context.tools = None;
        Ok(context)
    }
    fn stream_prepared_actual(
        &self,
        _provider: &dyn ModelProvider,
        _model: &Model,
        _context: &ActualProviderContext,
        _tools: Arc<[ValueHandle]>,
        _values: Arc<SessionValues>,
        _options: CallOptions,
    ) -> Result<ActualAssistantStream, ActualProviderError> {
        Err(ActualProviderError::Unsupported("legacy tool-slice actual stream; use original collection entry"))
    }
    fn stream_prepared_original_actual(
        &self,
        provider: &dyn ModelProvider,
        model: &Model,
        context: &ActualProviderContext,
        original_tools: Option<ValueHandle>,
        values: Arc<SessionValues>,
        mut options: CallOptions,
    ) -> Result<ActualAssistantStream, ActualProviderError> {
        context.check_lineage(&values)?;
        if let Some(original_tools) = &original_tools {
            values.check_handle(original_tools)?;
        }
        let original_context = context
            .original_context
            .clone()
            .ok_or(ActualProviderError::Unsupported("original actual provider Context"))?;
        let history_demotion_dialect = ara_ai::actual_provider::graph_history_demotion_dialect(
            crate::model_identity::preferred_dialect(&model.id),
        );
        let request = GraphProviderContext {
            original_context: Some(original_context),
            system_prompt: context.system_prompt.clone(),
            messages: context.messages.clone(),
            // Legacy observations cannot replace the post-hook source root.
            // AI reads root.tools at its original consumer positions.
            tools: None,
            history_demotion_dialect,
            history_id_hash: Some(Arc::new(NativeHistoryIdHash)),
            history_id_hash_function: Some(native_history_hash_function(&values)?),
        };
        let Some(original_tools) = original_tools else {
            return provider
                .stream_actual(model, &request, values.factory().clone(), options)
                .map_err(ActualProviderError::Value);
        };
        let binding =
            self.projection.as_ref().ok_or(ActualProviderError::Unsupported("Host owned-dialect selection"))?;
        let dialect = match binding.selector {
            ActualOwnedDialectSelector::Explicit(dialect) => dialect,
            ActualOwnedDialectSelector::ModelPreferred => {
                history_demotion_dialect.ok_or(ActualProviderError::Unsupported("Host preferred owned dialect"))?
            }
        };
        if !matches!(
            dialect,
            ara_ai::dialect::Dialect::Deepseek | ara_ai::dialect::Dialect::Hermes | ara_ai::dialect::Dialect::Qwen3
        ) {
            return Err(ActualProviderError::Unsupported("Actual owned stream for this fixed dialect"));
        }
        let globals = self
            .owned_stream_globals
            .as_ref()
            .ok_or(ActualProviderError::Unsupported("Host original owned-stream globals"))?;
        values.check_handle(globals)?;
        let outer_cancel = options.cancel.clone();
        let provider_abort = outer_cancel.child_token();
        options.cancel = provider_abort.clone();
        let inner = provider.stream_actual(model, &request, values.factory().clone(), options)?;
        ara_ai::dialect::owned_stream::wrap_owned_stream(
            inner,
            values.factory().clone(),
            globals.clone(),
            original_tools,
            dialect,
            provider_abort,
            outer_cancel,
            self.abort_on_fabrication.unwrap_or(true),
        )
        .map_err(ActualProviderError::Value)
    }
    fn project_context(
        &self,
        _model: &Model,
        context: Context,
        _side_request: bool,
    ) -> Result<Context, ProviderContextError> {
        Ok(context)
    }
    fn stream(
        &self,
        provider: &dyn ModelProvider,
        model: &Model,
        context: &Context,
        options: CallOptions,
    ) -> AssistantStream {
        provider.stream(model, context, options)
    }
}
