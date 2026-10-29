//! Provider-context normalization and side-request seam from fixed OMP agent/AI sources.
//! OMP commit 596f2da7101178214aa27a753529d15e6b7ad91d, MIT, Copyright OMP contributors.

mod actual;
pub(crate) mod graph_tools;
pub use actual::{ActualProviderContext, ActualProviderError, SessionValues};
pub(crate) use actual::{PreparedAssistantStream, PreparedRequestContext, PreparedStreamEvent};

use std::fmt;
use std::sync::Arc;

use ara_ai::{
    AssistantBlock, AssistantMessage, AssistantMessageEvent, CallOptions, Context, JsonObject, Message, Model,
    ModelProvider, Tool, ToolCall, ToolExample, tool_wire_schema,
};
use ara_prompt::js::{entries, json_stringify, number_to_string};
use serde_json::{Map, Value, json};

use crate::agent_loop::{AgentConfig, ExecutionSnapshot};
use crate::append_only_context::{BuildOptions, SharedAppendOnlyContextManager};
use crate::tool::{ActualToolSource, AgentTool, ToolIntentMode};

#[derive(Clone, Debug)]
pub enum ProviderContextError {
    Schema(String),
    PrefixNotBuilt,
    UnsupportedOwnedDialect,
    HostOwnedMutation,
    Actual(Arc<ActualProviderError>),
}

impl fmt::Display for ProviderContextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Actual(error) => error.fmt(formatter),
            Self::Schema(message) => formatter.write_str(message),
            Self::PrefixNotBuilt => formatter.write_str("StablePrefix.to_context called before build"),
            Self::UnsupportedOwnedDialect => {
                formatter.write_str("owned dialect requires a paired projection/stream port")
            }
            Self::HostOwnedMutation => {
                formatter.write_str("provider-context mutation must be published by its Host owner")
            }
        }
    }
}

impl std::error::Error for ProviderContextError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Actual(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}
impl PartialEq for ProviderContextError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Schema(left), Self::Schema(right)) => left == right,
            (Self::Actual(left), Self::Actual(right)) => Arc::ptr_eq(left, right),
            (Self::PrefixNotBuilt, Self::PrefixNotBuilt)
            | (Self::UnsupportedOwnedDialect, Self::UnsupportedOwnedDialect)
            | (Self::HostOwnedMutation, Self::HostOwnedMutation) => true,
            _ => false,
        }
    }
}
impl Eq for ProviderContextError {}
impl From<ActualProviderError> for ProviderContextError {
    fn from(error: ActualProviderError) -> Self {
        Self::Actual(Arc::new(error))
    }
}

/// Projection and canonical stream decoding belong to the same owned-dialect adapter.
pub trait OwnedProviderContextPort: Send + Sync {
    /// Pair actual projection/stream explicitly; legacy methods never receive
    /// a Graph export masquerading as the original argument/history domain.
    fn project_context_actual(
        &self,
        _model: &Model,
        _context: ActualProviderContext,
        _side_request: bool,
        _values: &SessionValues,
    ) -> Result<ActualProviderContext, ActualProviderError> {
        Err(ActualProviderError::Unsupported("owned actual context projection"))
    }
    /// The source collection captured after the original tools admission Gets.
    /// Legacy ports keep their prior behavior; actual encoders must consume this
    /// handle instead of reading Context.tools for a fourth time.
    fn project_context_original_actual(
        &self,
        model: &Model,
        context: ActualProviderContext,
        _original_tools: ara_rpc::value_heap::ValueHandle,
        side_request: bool,
        values: &SessionValues,
    ) -> Result<ActualProviderContext, ActualProviderError> {
        self.project_context_actual(model, context, side_request, values)
    }
    fn stream_prepared_actual(
        &self,
        _provider: &dyn ModelProvider,
        _model: &Model,
        _context: &ActualProviderContext,
        _tools: Arc<[ara_rpc::value_heap::ValueHandle]>,
        _values: Arc<SessionValues>,
        _options: CallOptions,
    ) -> Result<ara_ai::ActualAssistantStream, ActualProviderError> {
        Err(ActualProviderError::Unsupported("owned actual stream"))
    }

    /// The exact post-transform source collection, rather than a rebuilt
    /// member slice. Root must join the actual loop/receipt consumers before
    /// this additive source-only entry can be exercised.
    fn stream_prepared_original_actual(
        &self,
        _provider: &dyn ModelProvider,
        _model: &Model,
        _context: &ActualProviderContext,
        _original_tools: Option<ara_rpc::value_heap::ValueHandle>,
        _values: Arc<SessionValues>,
        _options: CallOptions,
    ) -> Result<ara_ai::ActualAssistantStream, ActualProviderError> {
        Err(ActualProviderError::Unsupported("owned original-collection actual stream"))
    }

    fn project_context(
        &self,
        model: &Model,
        context: Context,
        side_request: bool,
    ) -> Result<Context, ProviderContextError>;
    fn stream(
        &self,
        provider: &dyn ModelProvider,
        model: &Model,
        context: &Context,
        options: CallOptions,
    ) -> tokio::sync::mpsc::Receiver<AssistantMessageEvent>;

    /// The tools are the immutable post-hook snapshot admitted for this call.
    /// Existing SDK adapters retain their original stream implementation.
    fn stream_prepared(
        &self,
        provider: &dyn ModelProvider,
        model: &Model,
        context: &Context,
        _tools: Arc<[Tool]>,
        options: CallOptions,
    ) -> tokio::sync::mpsc::Receiver<AssistantMessageEvent> {
        self.stream(provider, model, context, options)
    }
}

#[derive(Clone, Default)]
pub struct ProviderContextBinding {
    pub build_options: BuildOptions,
    pub append_only_context: Option<SharedAppendOnlyContextManager>,
    pub owned_dialect: bool,
    pub owned_port: Option<Arc<dyn OwnedProviderContextPort>>,
    /// Captured through the existing ExecutionSnapshot binding only.
    pub actual_values: Option<Arc<SessionValues>>,
}

impl ProviderContextBinding {
    pub(crate) fn checked_owned_port(
        &self,
    ) -> Result<Option<&Arc<dyn OwnedProviderContextPort>>, ProviderContextError> {
        if self.owned_dialect {
            self.owned_port.as_ref().map(Some).ok_or(ProviderContextError::UnsupportedOwnedDialect)
        } else {
            Ok(None)
        }
    }

    pub(crate) fn effective_build_options(&self) -> BuildOptions {
        let mut options = self.build_options;
        // Native prepareProviderCall coerces the optional setting to a boolean.
        // Direct StablePrefix/Manager APIs still preserve absent versus false.
        options.prune_tool_descriptions = Some(options.prune_tool_descriptions == Some(true) && !self.owned_dialect);
        options
    }
}

pub fn normalize_messages_for_provider(messages: &[Message], model: &Model) -> Vec<Message> {
    let mut normalized = messages.to_vec();
    if model.provider == "cerebras" {
        for message in &mut normalized {
            if let Message::Assistant(assistant) = message {
                assistant.content.retain(|block| !matches!(block, AssistantBlock::Thinking(_)));
            }
        }
    }
    normalized
}

pub fn normalize_tools(
    tools: &[Arc<dyn AgentTool>],
    options: &BuildOptions,
) -> Result<Vec<Tool>, ProviderContextError> {
    tools
        .iter()
        .map(|tool| {
            let mut definition = tool.protocol_definition();
            let mode = tool.intent_mode();
            let inject = options.intent_tracing
                && !options.suppress_intent_schema
                && matches!(mode, ToolIntentMode::Require | ToolIntentMode::Optional)
                && !tool_owns_intent(&definition)?;
            let prune = options.prune_tool_descriptions == Some(true);
            let mut parameters = tool_wire_schema(&definition.parameters)
                .map_err(|error| ProviderContextError::Schema(error.to_string()))?;
            if prune {
                strip_schema_descriptions(&mut parameters);
            }
            if inject {
                parameters = inject_intent(parameters, mode == ToolIntentMode::Require, !prune);
            }
            definition.parameters = parameters;
            if prune {
                definition.description.clear();
            } else {
                let examples = render_tool_examples(&definition, inject);
                if !examples.is_empty() {
                    definition.description.push_str("\n\n");
                    definition.description.push_str(&examples);
                }
            }
            Ok(definition)
        })
        .collect()
}

fn inject_intent(schema: Value, require: bool, describe: bool) -> Value {
    let Value::Object(mut record) = schema else {
        return schema;
    };
    let has_properties = record.get("properties").is_some_and(Value::is_object);
    if !has_properties {
        for union in ["anyOf", "oneOf"] {
            if let Some(Value::Array(variants)) = record.get(union) {
                let variants =
                    variants.iter().cloned().map(|variant| inject_intent(variant, require, describe)).collect();
                record.insert(union.into(), Value::Array(variants));
                return Value::Object(record);
            }
        }
    }
    let mut properties = record.get("properties").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut required: Vec<Value> = record
        .get("required")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter(|item| item.is_string()).cloned().collect())
        .unwrap_or_default();
    if let Some(intent) = properties.shift_remove("i") {
        let needs_reorder = record
            .get("properties")
            .and_then(Value::as_object)
            .and_then(|properties| entries(properties).first().map(|(key, _)| key.as_str()))
            != Some("i");
        let needs_required = require && !required.iter().any(|item| item.as_str() == Some("i"));
        if !needs_reorder && !needs_required {
            return Value::Object(record);
        }
        if needs_reorder {
            let mut ordered = Map::new();
            ordered.insert("i".into(), intent);
            ordered.extend(properties);
            record.insert("properties".into(), Value::Object(ordered));
        }
        if needs_required {
            required.push(Value::String("i".into()));
            record.insert("required".into(), Value::Array(required));
        }
        return Value::Object(record);
    }
    let mut ordered = Map::new();
    ordered.insert(
        "i".into(),
        if describe { json!({"type":"string","description":"concise intent"}) } else { json!({"type":"string"}) },
    );
    ordered.extend(properties);
    record.insert("properties".into(), Value::Object(ordered));
    if require {
        required.push(Value::String("i".into()));
        record.insert("required".into(), Value::Array(required));
    }
    Value::Object(record)
}

fn strip_schema_descriptions(value: &mut Value) {
    if let Value::Array(items) = value {
        for item in items {
            strip_schema_descriptions(item);
        }
        return;
    }
    let Value::Object(record) = value else { return };
    record.shift_remove("description");
    for key in [
        "additionalProperties",
        "unevaluatedProperties",
        "unevaluatedItems",
        "items",
        "additionalItems",
        "contains",
        "propertyNames",
        "contentSchema",
        "if",
        "then",
        "else",
        "not",
        "anyOf",
        "oneOf",
        "allOf",
        "prefixItems",
    ] {
        if let Some(child) = record.get_mut(key) {
            strip_schema_descriptions(child);
        }
    }
    for key in ["properties", "patternProperties", "$defs", "definitions", "dependentSchemas"] {
        if let Some(Value::Object(children)) = record.get_mut(key) {
            for child in children.values_mut() {
                strip_schema_descriptions(child);
            }
        }
    }
}

fn py_string(value: &str) -> String {
    let escaped =
        value.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r").replace('\t', "\\t");
    format!("\"{escaped}\"")
}

fn py_value(value: &Value, top_level: bool) -> String {
    match value {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(number) => number_to_string(number),
        Value::String(value) => {
            if top_level
                && value.contains('\n')
                && !value.contains("\"\"\"")
                && !value.starts_with('"')
                && !value.ends_with('"')
                && !value.ends_with('\\')
            {
                format!("\"\"\"{value}\"\"\"")
            } else {
                py_string(value)
            }
        }
        Value::Array(items) => {
            format!("[{}]", items.iter().map(|item| py_value(item, false)).collect::<Vec<_>>().join(", "))
        }
        Value::Object(record) => format!(
            "{{{}}}",
            entries(record)
                .iter()
                .map(|(key, value)| format!("{}: {}", py_string(key), py_value(value, false)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn render_example_call(name: &str, arguments: &JsonObject, inject_intent: bool) -> String {
    if arguments.len() == 1
        && let Some(value) = arguments.values().next().and_then(Value::as_str)
    {
        let intent = if inject_intent { " i=\"…\"" } else { "" };
        return format!("<example{intent}>\n{value}\n</example>");
    }
    let mut final_arguments = Map::new();
    if inject_intent {
        final_arguments.insert("i".into(), Value::String("…".into()));
    }
    final_arguments.extend(arguments.clone());
    let kwargs = entries(&final_arguments)
        .iter()
        .map(|(key, value)| format!("{key}={}", py_value(value, true)))
        .collect::<Vec<_>>()
        .join(", ");
    format!("<example>\n{name}({kwargs})\n</example>")
}

pub fn render_tool_examples(tool: &Tool, inject_intent: bool) -> String {
    let Some(examples) = tool.examples.as_ref().filter(|examples| !examples.is_empty()) else {
        return String::new();
    };
    let parts = examples
        .iter()
        .map(|example| {
            let (caption, content) = match example {
                ToolExample::Call { call, caption } => {
                    (caption.as_deref(), render_example_call(&tool.name, call, inject_intent))
                }
                ToolExample::Compare { good, bad, caption } => (
                    caption.as_deref(),
                    format!(
                        "WRONG:\n{}\nRIGHT:\n{}",
                        render_example_call(&tool.name, bad, inject_intent),
                        render_example_call(&tool.name, good, inject_intent)
                    ),
                ),
                ToolExample::Note { caption, note } => {
                    let head = if caption.is_empty() {
                        String::new()
                    } else {
                        format!("# {caption}").trim_end_matches(js_whitespace).to_string()
                    };
                    return if let Some(note) = note.as_ref().filter(|note| !note.is_empty()) {
                        format!("{head}\n{note}")
                    } else {
                        head
                    };
                }
            };
            let head = caption
                .filter(|caption| !caption.is_empty())
                .map(|caption| format!("# {caption}\n"))
                .unwrap_or_default();
            format!("{head}{content}")
        })
        .collect::<Vec<_>>();
    format!("<examples>\n{}\n</examples>", parts.join("\n"))
}

/// Fixed OMP examples.ts renderToolExamplesJsdoc; no intent placeholder in inventories.
pub fn render_tool_examples_jsdoc(tool: &Tool) -> String {
    let Some(examples) = tool.examples.as_ref().filter(|examples| !examples.is_empty()) else {
        return String::new();
    };
    let render_call = |arguments: &JsonObject| {
        if arguments.len() == 1
            && let Some(value) = arguments.values().next().and_then(Value::as_str)
        {
            return value.to_string();
        }
        let kwargs = entries(arguments)
            .iter()
            .map(|(key, value)| format!("{key}={}", py_value(value, true)))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{}({kwargs})", tool.name)
    };
    examples
        .iter()
        .map(|example| {
            let (caption, body) = match example {
                ToolExample::Call { call, caption } => (caption.as_deref(), Some(render_call(call))),
                ToolExample::Compare { good, bad, caption } => {
                    (caption.as_deref(), Some(format!("WRONG:\n{}\nRIGHT:\n{}", render_call(bad), render_call(good))))
                }
                ToolExample::Note { caption, note } => {
                    (Some(caption.as_str()), note.as_ref().filter(|note| !note.is_empty()).cloned())
                }
            };
            let head = caption
                .filter(|caption| !caption.is_empty())
                .map(|caption| format!("@example {}", json_stringify(&Value::String(caption.into()))))
                .unwrap_or_else(|| "@example".into());
            body.map_or(head.clone(), |body| format!("{head}\n{body}"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Resolve ownership on the unmodified authored wire schema, before pruning or injection.
pub(crate) fn tool_owns_intent(definition: &Tool) -> Result<bool, ProviderContextError> {
    let schema = tool_wire_schema(&definition.parameters)
        .map_err(|error| ProviderContextError::Schema(error.to_string()))?;
    Ok(ara_ai::schema_validator::schema_defines_property(&schema, "i"))
}

pub(crate) fn stripped_arguments(call: &ToolCall, tracing: bool) -> JsonObject {
    let mut arguments = call.arguments.clone();
    if tracing {
        arguments.shift_remove("i");
    }
    arguments
}

fn extract_intent(arguments: &JsonObject) -> Option<String> {
    let intent = arguments.get("i")?.as_str()?.trim_matches(js_whitespace);
    let intent = intent.trim_end_matches('.').trim_end_matches(js_whitespace);
    (!intent.is_empty()).then(|| intent.to_string())
}

/// Annotate the owned final Assistant before MessageEnd makes its journal snapshot immutable.
pub(crate) fn annotate_intents(message: &mut AssistantMessage, tools: &[Arc<dyn AgentTool>], tracing: bool) {
    if !tracing {
        return;
    }
    let definitions: Vec<_> = tools.iter().map(|tool| (tool, tool.protocol_definition())).collect();
    for block in &mut message.content {
        let AssistantBlock::ToolCall(call) = block else {
            continue;
        };
        let tool = definitions.iter().find(|(_, definition)| definition.name == call.name).or_else(|| {
            definitions
                .iter()
                .find(|(_, definition)| definition.custom_wire_name.as_deref() == Some(call.name.as_str()))
        });
        // A malformed schema is reported by preparation; do not consume a
        // possible tool-owned value while publishing the final assistant.
        let owns_intent = match tool.map(|(_, definition)| tool_owns_intent(definition)).transpose() {
            Ok(owns) => owns.unwrap_or(false),
            Err(_) => continue,
        };
        annotate_call_intent(call, tool.map(|(tool, _)| tool.as_ref()), tracing, owns_intent);
    }
}

/// Used after exact/fallback resolution in the one prepare pass. Resume keeps
/// an already-derived intent so a callback with state does not run twice.
pub(crate) fn annotate_call_intent(call: &mut ToolCall, tool: Option<&dyn AgentTool>, tracing: bool, owns_intent: bool) {
    if !tracing {
        return;
    }
    if !owns_intent {
        if let Some(intent) = extract_intent(&call.arguments) {
            call.intent = Some(intent);
            return;
        }
    }
    if call.intent.is_some() {
        return;
    }
    if let Some(tool) = tool.filter(|tool| tool.intent_mode() == ToolIntentMode::Derived) {
        let arguments = stripped_arguments(call, !owns_intent);
        if let Ok(Some(intent)) =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tool.derive_intent(&arguments)))
        {
            let intent = intent.trim_matches(js_whitespace);
            if !intent.is_empty() {
                call.intent = Some(intent.to_string());
            }
        }
    }
}

fn js_whitespace(character: char) -> bool {
    matches!(character, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}'
        | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}'
        | '\u{3000}' | '\u{feff}')
}

pub(crate) struct PreparedProviderCall {
    pub context: PreparedRequestContext,
    pub suppress_native_tool_choice: bool,
    pub owned: Option<PreparedOwnedCall>,
}

pub(crate) struct PreparedOwnedCall {
    pub port: Arc<dyn OwnedProviderContextPort>,
    pub tools: Arc<[Tool]>,
    pub actual_tools: Option<Arc<[ara_rpc::value_heap::ValueHandle]>>,
    /// Retained original post-hook collection. Native methods/holes remain
    /// on this handle; actual_tools is only the separate legacy observation.
    pub original_post_hook_tools: Option<ara_rpc::value_heap::ValueHandle>,
    pub actual_tool_source: Option<Arc<dyn ActualToolSource>>,
    pub values: Option<Arc<SessionValues>>,
}

pub(crate) async fn main_request_context(
    config: &AgentConfig,
    messages: &[Message],
    binding: Option<&ProviderContextBinding>,
) -> Result<PreparedProviderCall, ProviderContextError> {
    let default_binding = ProviderContextBinding::default();
    let binding = binding.unwrap_or(&default_binding);
    let port = binding.checked_owned_port()?;
    let options = binding.effective_build_options();
    let messages = normalize_messages_for_provider(messages, &config.model);
    let context = if let Some(manager) = &binding.append_only_context {
        let mut manager = manager.lock().unwrap();
        manager.sync_messages(&messages)?;
        manager.build(&config.system_prompt, &config.tools, &options)?
    } else {
        Context {
            system_prompt: config.system_prompt.clone(),
            messages,
            tools: Some(normalize_tools(&config.tools, &options)?),
        }
    };
    let context = config.hooks.transform_provider_context(context, &config.model).await;
    if let Some(port) = port.filter(|_| context.tools.as_ref().is_some_and(|tools| !tools.is_empty())) {
        let tools: Arc<[Tool]> = context.tools.as_deref().expect("nonempty post-hook tools").to_vec().into();
        let mut projected = port.project_context(&config.model, context, false)?;
        projected.tools = None;
        return Ok(PreparedProviderCall {
            context: PreparedRequestContext::Typed(projected),
            suppress_native_tool_choice: binding.owned_dialect,
            owned: Some(PreparedOwnedCall {
                port: port.clone(),
                tools,
                actual_tools: None,
                original_post_hook_tools: None,
                actual_tool_source: None,
                values: None,
            }),
        });
    }
    Ok(PreparedProviderCall {
        context: PreparedRequestContext::Typed(context),
        suppress_native_tool_choice: binding.owned_dialect,
        owned: None,
    })
}

/// Host tool definitions enter through their existing typed metadata contract;
/// Graph arguments and messages never export to serde in this request path.
pub(crate) async fn main_actual_request_context(
    config: &AgentConfig,
    messages: Vec<ara_rpc::value_heap::ValueHandle>,
    binding: &ProviderContextBinding,
    values: Arc<SessionValues>,
) -> Result<PreparedProviderCall, ProviderContextError> {
    if config.model.provider == "cerebras" {
        return Err(ActualProviderError::Unsupported("actual Cerebras thinking normalization").into());
    }
    if !config.hooks.supports_actual_turn_end() {
        return Err(ActualProviderError::Unsupported("actual turn-end consumer").into());
    }
    let port = binding.owned_port.as_ref().ok_or(ActualProviderError::Unsupported("actual provider stream port"))?;
    for message in &messages {
        values.check_handle(message).map_err(ActualProviderError::from)?;
    }
    // Capture only the original Host capability after admission/lineage checks.
    // It remains separate from normalized/post-transform wire definitions.
    let actual_tool_source = config.hooks.actual_tool_source(&values)?;
    let context = if let Some(manager) = &binding.append_only_context {
        // Original prepareProviderCall order: sync/digest before tool normalization/build.
        if let Some(source) = actual_tool_source.as_ref() {
            crate::append_only_context::actual::prepare_append_only_original_actual(
                manager, &config.system_prompt, &config.tools, &messages,
                &binding.effective_build_options(), &values, source.as_ref(),
            )?
        } else { crate::append_only_context::actual::prepare_append_only_actual(
            manager,
            &config.system_prompt,
            &config.tools,
            &messages,
            &binding.effective_build_options(),
            &values,
        )? }
    } else {
        let (tools, tool_array) = if let Some(source) = actual_tool_source.as_ref() {
            let original = source.current_tools(&values)?.unwrap_or(values.heap().undefined().map_err(ActualProviderError::from)?);
            values.check_handle(&original).map_err(ActualProviderError::from)?;
            let normalized = graph_tools::normalize_original_tools(&values,&original,&binding.effective_build_options()).map_err(ActualProviderError::from)?;
            if matches!(values.heap().kind(&normalized).map_err(ActualProviderError::from)?,ara_rpc::value_heap::ValueKind::Undefined|ara_rpc::value_heap::ValueKind::Null) {
                (None,normalized)
            } else {
            let length=values.heap().array_length(&normalized,values.runtime()).map_err(ActualProviderError::from)?;
            let mut observed=Vec::new();for index in 0..length{
                observed.push(values.heap().get(&normalized,&index.to_string().as_str().into(),values.runtime()).map_err(ActualProviderError::from)?);
            }
            (Some(observed),normalized)
            }
        } else {
        let normalized = normalize_tools(&config.tools, &binding.effective_build_options())?;
        let mut tools = Vec::with_capacity(normalized.len());
        for definition in normalized {
            let source =
                serde_json::to_value(definition).map_err(|error| ActualProviderError::Callback(error.to_string()))?;
            tools.push(values.heap().import_serde(&source).map_err(ActualProviderError::from)?);
        }
        let tool_array=values.heap().array(tools.clone()).map_err(ActualProviderError::from)?;
        (Some(tools),tool_array)
        };
        // Fresh SDK request producer, corresponding to the source Context object
        // literal. These arrays contain the admitted original member handles;
        // they do not claim to be an arbitrary Host convertToLlm/normalizeTools
        // method result or to preserve that unconnected upstream container.
        let system_prompt =
            config.system_prompt.iter().map(|text| ara_rpc::WireString::from(text.as_str())).collect::<Vec<_>>();
        let system_prompt_values = system_prompt
            .iter()
            .cloned()
            .map(|text| values.heap().string(text))
            .collect::<Result<Vec<_>, _>>()
            .map_err(ActualProviderError::from)?;
        let original = values.heap().object().map_err(ActualProviderError::from)?;
        let prompt_array = values.heap().array(system_prompt_values).map_err(ActualProviderError::from)?;
        let message_array = values.heap().array(messages.clone()).map_err(ActualProviderError::from)?;
        for (key, value) in [("systemPrompt", prompt_array), ("messages", message_array), ("tools", tool_array)] {
            values
                .heap()
                .define_own_data(&original, key.into(), value, values.runtime())
                .map_err(ActualProviderError::from)?;
        }
        ActualProviderContext { original_context: Some(original), system_prompt, messages, tools: tools.map(Into::into) }
    };
    let context = config.hooks.transform_provider_context_actual(context, &config.model, &values).await?;
    context.check_lineage(&values)?;
    // Compatibility observations are not original AI/executable inputs.
    let legacy_tools = context.tools.clone();
    let mut original_post_hook_tools = None;
    let mut projected = context;
    if binding.owned_dialect {
        let original = projected
            .original_context
            .as_ref()
            .ok_or(ActualProviderError::Unsupported("post-hook original provider Context"))?;
        let first_tools =
            values.heap().get(original, &"tools".into(), values.runtime()).map_err(ActualProviderError::from)?;
        if values.heap().truthy(&first_tools).map_err(ActualProviderError::from)? {
            let length_tools =
                values.heap().get(original, &"tools".into(), values.runtime()).map_err(ActualProviderError::from)?;
            let length = values
                .heap()
                .get(&length_tools, &"length".into(), values.runtime())
                .map_err(ActualProviderError::from)?;
            if values.heap().to_number(&length, values.runtime()).map_err(ActualProviderError::from)? > 0.0 {
                let captured = values
                    .heap()
                    .get(original, &"tools".into(), values.runtime())
                    .map_err(ActualProviderError::from)?;
                values.check_handle(&captured).map_err(ActualProviderError::from)?;
                original_post_hook_tools = Some(captured.clone());
                projected = port.project_context_original_actual(&config.model, projected, captured, false, &values)?;
                projected.check_lineage(&values)?;
                projected.tools = None;
            }
        }
    }
    Ok(PreparedProviderCall {
        context: PreparedRequestContext::Actual { context: projected, values: values.clone() },
        suppress_native_tool_choice: binding.owned_dialect,
        owned: Some(PreparedOwnedCall {
            port: port.clone(),
            tools: Arc::from([]),
            actual_tools: legacy_tools,
            original_post_hook_tools,
            actual_tool_source,
            values: Some(values),
        }),
    })
}

/// One side-call request in the original Session realm. This carries no tool
/// execution grant and never publishes or synchronizes the main prefix cache.
pub struct PreparedActualSideRequest {
    pub(crate) context: ActualProviderContext,
    pub(crate) values: Arc<SessionValues>,
    port: Arc<dyn OwnedProviderContextPort>,
}

impl PreparedActualSideRequest {
    pub(crate) fn stream(
        &self,
        provider: &dyn ModelProvider,
        model: &Model,
        options: CallOptions,
    ) -> Result<ara_ai::ActualAssistantStream, ActualProviderError> {
        self.context.check_lineage(&self.values)?;
        // Fixed buildSideRequestContext supplies [] for an owned dialect.
        // No main-turn owned-tool projection, wrapper or execute source runs.
        self.port.stream_prepared_original_actual(provider, model, &self.context, None, self.values.clone(), options)
    }
}

/// Graph counterpart of fixed Agent.buildSideRequestContext. The caller has
/// converted the complete original history, including the trailing raw User.
pub async fn build_actual_side_request_context_from_snapshot(
    config: &AgentConfig,
    snapshot: ExecutionSnapshot,
    messages: Vec<ara_rpc::value_heap::ValueHandle>,
    system_prompt_override: Option<Vec<String>>,
) -> Result<PreparedActualSideRequest, ProviderContextError> {
    let binding = snapshot.provider_context.unwrap_or_default();
    let values = binding.actual_values.clone().ok_or(ActualProviderError::Unsupported("actual side Session values"))?;
    let port = binding.owned_port.clone().ok_or(ActualProviderError::Unsupported("actual side stream port"))?;
    let model = snapshot.model.as_ref().map_or(&config.model, |selected| &selected.model);
    if model.provider == "cerebras" {
        return Err(ActualProviderError::Unsupported("actual Cerebras thinking normalization").into());
    }
    for message in &messages {
        values.check_handle(message).map_err(ActualProviderError::from)?;
    }
    let definitions = if binding.owned_dialect {
        Vec::new()
    } else {
        normalize_tools(&snapshot.tools, &binding.effective_build_options())?
    };
    let mut tools = Vec::with_capacity(definitions.len());
    for definition in definitions {
        let definition =
            serde_json::to_value(definition).map_err(|error| ActualProviderError::Callback(error.to_string()))?;
        tools.push(values.heap().import_serde(&definition).map_err(ActualProviderError::from)?);
    }
    let system_prompt = system_prompt_override
        .unwrap_or(snapshot.system_prompt)
        .iter()
        .map(|text| ara_rpc::WireString::from(text.as_str()))
        .collect::<Vec<_>>();
    let prompt_values = system_prompt
        .iter()
        .cloned()
        .map(|text| values.heap().string(text))
        .collect::<Result<Vec<_>, _>>()
        .map_err(ActualProviderError::from)?;
    // These fresh SDK literal arrays retain the converted original members.
    // Neither a budget DTO nor an exported/reimported history enters here.
    let original = values.heap().object().map_err(ActualProviderError::from)?;
    for (key, value) in [
        ("systemPrompt", values.heap().array(prompt_values)),
        ("messages", values.heap().array(messages.clone())),
        ("tools", values.heap().array(tools.clone())),
    ] {
        values
            .heap()
            .define_own_data(&original, key.into(), value.map_err(ActualProviderError::from)?, values.runtime())
            .map_err(ActualProviderError::from)?;
    }
    let context =
        ActualProviderContext { original_context: Some(original), system_prompt, messages, tools: Some(tools.into()) };
    let context = config.hooks.transform_provider_context_actual(context, model, &values).await?;
    context.check_lineage(&values)?;
    Ok(PreparedActualSideRequest { context, values, port })
}

pub async fn build_side_request_context_from_snapshot(
    config: &AgentConfig,
    snapshot: ExecutionSnapshot,
    messages: Vec<Message>,
    system_prompt_override: Option<Vec<String>>,
) -> Result<Context, ProviderContextError> {
    let binding = snapshot.provider_context.unwrap_or_default();
    if binding.actual_values.is_some() {
        return Err(ActualProviderError::Unsupported("actual side request").into());
    }
    binding.checked_owned_port()?;
    let model = snapshot.model.as_ref().map_or(&config.model, |execution| &execution.model);
    let options = binding.effective_build_options();
    let context = Context {
        system_prompt: system_prompt_override.unwrap_or(snapshot.system_prompt),
        messages: normalize_messages_for_provider(&messages, model),
        tools: if binding.owned_dialect { Some(Vec::new()) } else { Some(normalize_tools(&snapshot.tools, &options)?) },
    };
    let context = config.hooks.transform_provider_context(context, model).await;
    Ok(context)
}

pub async fn build_side_request_context(
    config: &AgentConfig,
    messages: Vec<Message>,
    system_prompt_override: Option<Vec<String>>,
) -> Result<Context, ProviderContextError> {
    let snapshot = config.hooks.execution_snapshot().unwrap_or_else(|| ExecutionSnapshot::from_config(config));
    build_side_request_context_from_snapshot(config, snapshot, messages, system_prompt_override).await
}
