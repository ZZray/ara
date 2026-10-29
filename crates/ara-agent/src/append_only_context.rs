//! Fixed OMP append-only context (`packages/agent/src/append-only-context.ts`).
//! OMP commit 596f2da7101178214aa27a753529d15e6b7ad91d, MIT, Copyright OMP contributors.
//! Rust owns the cached projections; it does not memoize addresses of transcript clones.

pub mod actual;

use std::sync::{Arc, Mutex};

use ara_ai::{Context, Message, Tool};
use ara_prompt::js::{entries, json_stringify, number_to_string};
use serde_json::{Map, Value};

use crate::provider_context::{ProviderContextError, normalize_tools};
use crate::tool::AgentTool;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BuildOptions {
    pub intent_tracing: bool,
    pub prune_tool_descriptions: Option<bool>,
    /// Host-prepared ARA_NO_INTENT == "1"; execution tracing stays enabled.
    pub suppress_intent_schema: bool,
}

pub type SharedAppendOnlyContextManager = Arc<Mutex<AppendOnlyContextManager>>;

#[derive(Clone, Debug)]
pub struct StablePrefixSnapshot {
    pub system_prompt: Vec<String>,
    pub tools: Vec<Tool>,
    pub fingerprint: String,
}

#[derive(Clone, Debug, Default)]
pub struct StablePrefix {
    snapshot: Option<StablePrefixSnapshot>,
    version: u64,
    /// Original tools stay in the actual prefix owner, not an empty typed copy.
    original_fingerprint: Option<String>,
    typed_key: Option<TypedPrefixKey>,
}

// The legacy typed API owns serde values, not Native schema references. Its
// compatibility key compares ordered values without pretending clones have
// object identity. The original Graph path below uses actual reference IDs.
#[derive(Clone, Debug)]
struct TypedToolKey {
    fields: Value,
    mode: crate::tool::ToolIntentMode,
    derived_owner: Option<std::sync::Weak<dyn AgentTool>>,
}
#[derive(Clone, Debug)]
struct TypedPrefixKey {
    prompt: Vec<String>,
    tools: Vec<TypedToolKey>,
    options: BuildOptions,
}
impl TypedPrefixKey {
    fn matches(&self, prompt: &[String], tools: &[TypedToolKey], options: &BuildOptions) -> bool {
        self.prompt == prompt && self.options == *options && self.tools.len() == tools.len()
            && self.tools.iter().zip(tools).all(|(left, right)| {
                left.mode == right.mode && same_js_projection(&left.fields, &right.fields)
                    && match (&left.derived_owner, &right.derived_owner) {
                        (None, None) => true,
                        (Some(left), Some(right)) => std::sync::Weak::ptr_eq(left, right),
                        _ => false,
                    }
            })
    }
}
fn typed_tool_keys(tools: &[Arc<dyn AgentTool>]) -> Result<Vec<TypedToolKey>, ProviderContextError> {
    tools.iter().map(|tool| {
        let fields = serde_json::to_value(tool.protocol_definition())
            .map_err(|error| ProviderContextError::Schema(error.to_string()))?;
        let mode = tool.intent_mode();
        Ok(TypedToolKey { fields, mode, derived_owner: (mode == crate::tool::ToolIntentMode::Derived)
            .then(|| Arc::downgrade(tool)) })
    }).collect()
}

impl StablePrefix {
    pub fn fingerprint(&self) -> &str {
        self.original_fingerprint.as_deref().unwrap_or_else(||self.snapshot.as_ref().map_or("<unbuilt>", |snapshot| snapshot.fingerprint.as_str()))
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn built(&self) -> bool {
        self.snapshot.is_some() || self.original_fingerprint.is_some()
    }

    pub fn build(
        &mut self,
        system_prompt: &[String],
        tools: &[Arc<dyn AgentTool>],
        options: &BuildOptions,
    ) -> Result<bool, ProviderContextError> {
        let previous = self.snapshot.as_ref().map(|snapshot| snapshot.fingerprint.clone());
        let tool_keys = typed_tool_keys(tools)?;
        if previous.is_some() && self.original_fingerprint.is_none()
            && self.typed_key.as_ref().is_some_and(|key| key.matches(system_prompt, &tool_keys, options)) {
            return Ok(false);
        }
        let tools = normalize_tools(tools, options)?;
        let fingerprint = prefix_fingerprint(system_prompt, &tools, options);
        self.typed_key = Some(TypedPrefixKey { prompt: system_prompt.to_vec(), tools: tool_keys, options: *options });
        if previous.as_deref() == Some(fingerprint.as_str()) {
            return Ok(false);
        }
        self.snapshot = Some(StablePrefixSnapshot { system_prompt: system_prompt.to_vec(), tools, fingerprint });
        self.original_fingerprint = None;
        self.version += 1;
        Ok(true)
    }

    pub fn invalidate(&mut self) {
        self.snapshot = None;
        self.original_fingerprint = None;
        self.typed_key = None;
    }

    pub fn to_context(&self) -> Result<(Vec<String>, Vec<Tool>), ProviderContextError> {
        if self.original_fingerprint.is_some() {
            return Err(crate::ActualProviderError::Unsupported("original prefix cannot be projected as typed tools").into());
        }
        let snapshot = self.snapshot.as_ref().ok_or(ProviderContextError::PrefixNotBuilt)?;
        Ok((snapshot.system_prompt.clone(), snapshot.tools.clone()))
    }
}

#[derive(Clone, Debug, Default)]
pub struct AppendOnlyLog {
    entries: Vec<Message>,
}

impl AppendOnlyLog {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn append(&mut self, message: Message) {
        self.entries.push(message);
    }

    pub fn extend(&mut self, messages: impl IntoIterator<Item = Message>) {
        self.entries.extend(messages);
    }

    pub fn replace_tail(&mut self, replacement: Message) {
        if let Some(tail) = self.entries.last_mut() {
            *tail = replacement;
        }
    }

    pub fn to_messages(&self) -> Vec<Message> {
        self.entries.clone()
    }

    pub fn entries(&self) -> &[Message] {
        &self.entries
    }

    pub fn truncate(&mut self, count: isize) {
        self.entries.truncate(count.max(0) as usize);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[derive(Clone, Debug)]
struct DigestMemo {
    projection: Value,
    digest: u32,
}

#[derive(Clone, Debug, Default)]
pub struct AppendOnlyContextManager {
    prefix: StablePrefix,
    log: AppendOnlyLog,
    last_sync_count: usize,
    message_digests: Vec<u32>,
    /// Memo is per owned synchronization slot, never shared across Sessions.
    digest_memo: Vec<DigestMemo>,
    /// Original-domain state for this same manager; callbacks never hold its mutex.
    actual: Option<actual::SharedActualAppendOnlyState>,
}

impl AppendOnlyContextManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn prefix(&self) -> &StablePrefix {
        &self.prefix
    }

    pub fn prefix_mut(&mut self) -> &mut StablePrefix {
        &mut self.prefix
    }

    pub fn log(&self) -> &AppendOnlyLog {
        &self.log
    }

    pub fn log_mut(&mut self) -> &mut AppendOnlyLog {
        &mut self.log
    }

    pub fn sync_cursor(&self) -> usize {
        self.last_sync_count
    }

    pub fn build(
        &mut self,
        system_prompt: &[String],
        tools: &[Arc<dyn AgentTool>],
        options: &BuildOptions,
    ) -> Result<Context, ProviderContextError> {
        self.prefix.build(system_prompt, tools, options)?;
        self.to_context()
    }

    pub fn to_context(&self) -> Result<Context, ProviderContextError> {
        let (system_prompt, tools) = self.prefix.to_context()?;
        Ok(Context { system_prompt, messages: self.log.to_messages(), tools: Some(tools) })
    }

    pub fn sync_messages(&mut self, messages: &[Message]) -> Result<(), ProviderContextError> {
        if messages.len() < self.last_sync_count {
            self.reset_sync_cursor();
        }
        // Projection checks see owned rewrites, including a providerPayload-only change.
        // Unchanged slots retain their hash and skip JSON.stringify of old content.
        let mut digests = Vec::with_capacity(messages.len());
        for (index, message) in messages.iter().enumerate() {
            let value =
                serde_json::to_value(message).map_err(|error| ProviderContextError::Schema(error.to_string()))?;
            let projection = message_digest_projection(&value).expect("typed Message is an object");
            let digest = if let Some(memo) =
                self.digest_memo.get(index).filter(|memo| same_js_projection(&memo.projection, &projection))
            {
                memo.digest
            } else {
                hash_utf16(&json_stringify(&projection))
            };
            let memo = DigestMemo { projection, digest };
            if index < self.digest_memo.len() {
                self.digest_memo[index] = memo;
            } else {
                self.digest_memo.push(memo);
            }
            digests.push(digest);
        }
        self.digest_memo.truncate(messages.len());
        if self.last_sync_count > 0 {
            let bound = self.last_sync_count.min(messages.len());
            let stable = (0..bound)
                .find(|index| self.message_digests[*index] != digests[*index])
                .unwrap_or(bound)
                .min(self.log.len());
            if stable < self.last_sync_count {
                self.log.truncate(stable as isize);
                self.last_sync_count = stable;
                self.message_digests.truncate(stable);
            }
        }
        for index in self.last_sync_count..messages.len() {
            self.log.append(messages[index].clone());
            self.message_digests.push(digests[index]);
        }
        self.last_sync_count = messages.len();
        Ok(())
    }

    pub fn invalidate_for_model_change(&mut self) {
        self.invalidate();
        self.reset_sync_cursor();
    }

    pub fn reset_sync_cursor(&mut self) {
        self.log.clear();
        self.last_sync_count = 0;
        self.message_digests.clear();
        self.digest_memo.clear();
        if let Some(state) = &self.actual {
            actual::reset_sync_cursor(state);
        }
    }

    pub fn append_message(&mut self, message: Message) {
        self.log.append(message);
    }

    pub fn replace_tail_message(&mut self, message: Message) {
        self.log.replace_tail(message);
    }

    pub fn invalidate(&mut self) {
        self.prefix.invalidate();
        if let Some(state) = &self.actual {
            actual::invalidate_prefix(state);
        }
    }

    pub fn reset(
        &mut self,
        system_prompt: &[String],
        tools: &[Arc<dyn AgentTool>],
        options: &BuildOptions,
    ) -> Result<(), ProviderContextError> {
        self.invalidate();
        self.reset_sync_cursor();
        self.prefix.build(system_prompt, tools, options)?;
        Ok(())
    }
}

pub fn hash_utf16(value: &str) -> u32 {
    value.encode_utf16().fold(0_u32, |hash, unit| hash.wrapping_mul(31).wrapping_add(u32::from(unit)))
}

fn base36(mut number: u32) -> String {
    if number == 0 {
        return "0".into();
    }
    let mut digits = Vec::new();
    while number > 0 {
        let digit = (number % 36) as u8;
        digits.push(if digit < 10 { b'0' + digit } else { b'a' + digit - 10 });
        number /= 36;
    }
    digits.reverse();
    String::from_utf8(digits).expect("base36 contains ASCII")
}

pub fn prefix_fingerprint(system_prompt: &[String], tools: &[Tool], options: &BuildOptions) -> String {
    let tools = tools
        .iter()
        .map(|tool| {
            let mut value = Map::new();
            value.insert("n".into(), Value::String(tool.name.clone()));
            value.insert("d".into(), Value::String(tool.description.clone()));
            value.insert("p".into(), tool.parameters.clone());
            if let Some(strict) = tool.strict {
                value.insert("s".into(), Value::Bool(strict));
            }
            if let Some(format) = &tool.custom_format {
                value.insert("cf".into(), serde_json::to_value(format).expect("custom format is JSON"));
            }
            if let Some(name) = &tool.custom_wire_name {
                value.insert("cw".into(), Value::String(name.clone()));
            }
            Value::Object(value)
        })
        .collect();
    let mut value = Map::new();
    value.insert("s".into(), Value::Array(system_prompt.iter().cloned().map(Value::String).collect()));
    value.insert("t".into(), Value::Array(tools));
    value.insert("i".into(), Value::Bool(options.intent_tracing));
    if let Some(prune) = options.prune_tool_descriptions {
        value.insert("pd".into(), Value::Bool(prune));
    }
    base36(hash_utf16(&json_stringify(&Value::Object(value))))
}

/// Exact Native selector for source fixtures. Array is a JS object, primitives/null are not.
pub fn message_digest_projection(value: &Value) -> Option<Value> {
    if !matches!(value, Value::Object(_) | Value::Array(_)) {
        return None;
    }
    let field = |names: &[&str]| {
        names.iter().find_map(|name| value.get(*name).filter(|value| !value.is_null())).cloned().unwrap_or(Value::Null)
    };
    let mut payload = Map::new();
    for (target, sources) in [
        ("r", &["role"][..]),
        ("c", &["content"][..]),
        ("pp", &["providerPayload"][..]),
        ("tc", &["toolCalls", "tool_calls"][..]),
        ("tcid", &["toolCallId", "tool_call_id"][..]),
        ("tn", &["toolName", "name"][..]),
        ("err", &["isError"][..]),
        ("id", &["id"][..]),
    ] {
        payload.insert(target.into(), field(sources));
    }
    Some(Value::Object(payload))
}

pub fn message_digest(value: &Value) -> u32 {
    message_digest_projection(value).map_or(0, |projection| hash_utf16(&json_stringify(&projection)))
}

/// serde_json object equality ignores insertion order; Native stringify does not.
/// Compare the exact JS projection order without serializing unchanged content.
fn same_js_projection(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(left), Value::Bool(right)) => left == right,
        (Value::String(left), Value::String(right)) => left == right,
        (Value::Number(left), Value::Number(right)) => number_to_string(left) == number_to_string(right),
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len() && left.iter().zip(right).all(|(left, right)| same_js_projection(left, right))
        }
        (Value::Object(left), Value::Object(right)) => {
            let left = entries(left);
            let right = entries(right);
            left.len() == right.len()
                && left.iter().zip(right.iter()).all(|((left_key, left), (right_key, right))| {
                    left_key == right_key && same_js_projection(left, right)
                })
        }
        _ => false,
    }
}
