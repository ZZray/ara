// Ported from OMP host-tools.ts / rpc-mode.ts at
// 596f2da7101178214aa27a753529d15e6b7ad91d. MIT notices: THIRD_PARTY.md.
//! Connection-owned Host tool requests. Registry publication belongs to Host.

use ara_agent::{AgentTool, ToolError, ToolOutput, UpdateFn};
use ara_ai::{JsonObject, Tool, UserBlock};
use ara_rpc::{WireString, WireValue};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub type BridgeOutput = Arc<dyn Fn(WireValue) + Send + Sync>;

#[derive(Clone, Debug, PartialEq)]
pub struct HostToolDefinition {
    pub name: String,
    pub label: String,
    pub hidden: bool,
    pub load_mode: String,
    pub schema: Tool,
}

fn string(value: Option<&WireValue>) -> Option<String> {
    value?.as_string()?.to_utf8().ok()
}

fn default_load_mode(name: &str) -> &'static str {
    // The fixed source uses `name in ESSENTIAL_BUILTIN_TOOL_NAMES`, so ordinary
    // Object.prototype members also count as present in that plain object.
    if matches!(
        name,
        "read"
            | "write"
            | "bash"
            | "edit"
            | "glob"
            | "computer"
            | "eval"
            | "task"
            | "hub"
            | "learn"
            | "manage_skill"
            | "constructor"
            | "__defineGetter__"
            | "__defineSetter__"
            | "hasOwnProperty"
            | "__lookupGetter__"
            | "__lookupSetter__"
            | "isPrototypeOf"
            | "propertyIsEnumerable"
            | "toString"
            | "valueOf"
            | "__proto__"
            | "toLocaleString"
    ) {
        "essential"
    } else {
        "discoverable"
    }
}

/// The caller owns duplicate/collision validation and atomic registry refresh.
pub fn normalize_host_tool_definitions(tools: &WireValue) -> Result<Vec<HostToolDefinition>, String> {
    let tools = tools.as_array().ok_or("Host tools must be an array")?;
    tools
        .iter()
        .enumerate()
        .map(|(index, raw)| {
            let name = string(raw.get("name")).unwrap_or_default();
            let name = ara_prompt::js::trim(&name).to_owned();
            if name.is_empty() {
                return Err(format!("Host tool at index {index} must provide a non-empty name"));
            }
            let description = string(raw.get("description")).unwrap_or_default();
            let description = ara_prompt::js::trim(&description).to_owned();
            if description.is_empty() {
                return Err(format!("Host tool \"{name}\" must provide a non-empty description"));
            }
            let parameters = raw
                .get("parameters")
                .filter(|value| value.is_object())
                .ok_or_else(|| format!("Host tool \"{name}\" must provide a JSON Schema object"))?;
            let parameters: Value = serde_json::from_str(&parameters.stringify())
                .map_err(|error| format!("Host tool \"{name}\" schema cannot enter typed Core: {error}"))?;
            let label = string(raw.get("label")).unwrap_or_default();
            let label = ara_prompt::js::trim(&label);
            let load_mode = string(raw.get("loadMode"))
                .filter(|mode| !mode.is_empty())
                .unwrap_or_else(|| default_load_mode(&name).to_owned());
            Ok(HostToolDefinition {
                label: if label.is_empty() { name.clone() } else { label.to_owned() },
                hidden: matches!(raw.get("hidden"), Some(WireValue::Bool(true))),
                load_mode,
                schema: Tool { name: name.clone(), description, parameters },
                name,
            })
        })
        .collect()
}

enum Reply {
    Output(ToolOutput),
    HostError(String),
    Interrupted { message: String, dispatched: bool },
}

struct Pending {
    reply: oneshot::Sender<Reply>,
    update: UpdateFn,
    dispatched: bool,
}

#[derive(Default)]
struct State {
    pending: HashMap<WireString, Pending>,
    closed: Option<String>,
}

pub struct ToolBridge {
    output: BridgeOutput,
    state: Mutex<State>,
}

impl ToolBridge {
    pub fn new(output: BridgeOutput) -> Arc<Self> {
        Arc::new(Self { output, state: Mutex::new(State::default()) })
    }

    /// Adapters capture their definition. A Host registry replacement must not
    /// retarget calls already admitted from an older model-response snapshot.
    pub fn adapters(self: &Arc<Self>, definitions: &[HostToolDefinition]) -> Vec<Arc<dyn AgentTool>> {
        definitions
            .iter()
            .map(|definition| {
                Arc::new(HostToolAdapter { definition: Arc::new(definition.clone()), bridge: self.clone() })
                    as Arc<dyn AgentTool>
            })
            .collect()
    }

    /// Exactly the fixed structural guard; unknown/late valid IDs are consumed
    /// without ACK. Malformed recognized-looking input remains ordinary input.
    pub fn consume(&self, frame: &WireValue) -> bool {
        let Some(kind) = frame.get("type").and_then(WireValue::as_string) else { return false };
        let result_key = if kind.equals_ascii("host_tool_result") {
            "result"
        } else if kind.equals_ascii("host_tool_update") {
            "partialResult"
        } else {
            return false;
        };
        let Some(id) = frame.get("id").and_then(WireValue::as_string) else { return false };
        let Some(result) =
            frame.get(result_key).filter(|result| result.get("content").and_then(WireValue::as_array).is_some())
        else {
            return false;
        };
        if result_key == "partialResult" {
            let update = self.state.lock().unwrap().pending.get(id).map(|pending| pending.update.clone());
            if let Some(update) = update {
                // User callbacks may themselves settle or clear the bridge.
                if let Ok(output) = decode_output(result) {
                    update(output);
                }
            }
        } else {
            let pending = self.state.lock().unwrap().pending.remove(id);
            if let Some(pending) = pending {
                let reply = if frame.get("isError").is_some_and(wire_truthy) {
                    let text = result
                        .get("content")
                        .and_then(WireValue::as_array)
                        .unwrap()
                        .iter()
                        .filter(|block| {
                            block
                                .get("type")
                                .and_then(WireValue::as_string)
                                .is_some_and(|kind| kind.equals_ascii("text"))
                        })
                        .filter_map(|block| string(block.get("text")))
                        .collect::<Vec<_>>()
                        .join("\n");
                    let text = ara_prompt::js::trim(&text);
                    Reply::HostError(if text.is_empty() { "Host tool execution failed".into() } else { text.into() })
                } else {
                    match decode_output(result) {
                        Ok(output) => Reply::Output(output),
                        Err(message) => Reply::Interrupted { message, dispatched: pending.dispatched },
                    }
                };
                let _ = pending.reply.send(reply);
            }
        }
        true
    }

    #[cfg(test)]
    fn reject_all_pending(&self, message: &str) {
        let pending = std::mem::take(&mut self.state.lock().unwrap().pending);
        for pending in pending.into_values() {
            let _ = pending.reply.send(Reply::Interrupted { message: message.into(), dispatched: pending.dispatched });
        }
    }

    /// EOF closes future requests as well as active requests. The first close
    /// reason wins, matching the fixed bridge's closedError.
    pub fn close(&self, message: &str) {
        let (pending, message) = {
            let mut state = self.state.lock().unwrap();
            let message = state.closed.get_or_insert_with(|| message.into()).clone();
            (std::mem::take(&mut state.pending), message)
        };
        for pending in pending.into_values() {
            let _ = pending.reply.send(Reply::Interrupted { message: message.clone(), dispatched: pending.dispatched });
        }
    }

    async fn execute(
        self: &Arc<Self>,
        definition: &HostToolDefinition,
        call_id: &str,
        args: JsonObject,
        cancel: CancellationToken,
        update: UpdateFn,
    ) -> Result<ToolOutput, ToolError> {
        let aborted = format!("Host tool \"{}\" was aborted", definition.name);
        if cancel.is_cancelled() {
            return Ok(not_dispatched(aborted));
        }
        let mut id = uuid::Uuid::now_v7().to_string();
        while id == call_id {
            id = uuid::Uuid::now_v7().to_string();
        }
        let id = WireString::from(id);
        let (reply, mut received) = oneshot::channel();
        {
            let mut state = self.state.lock().unwrap();
            if let Some(message) = &state.closed {
                return Ok(not_dispatched(message.clone()));
            }
            if cancel.is_cancelled() {
                return Ok(not_dispatched(aborted));
            }
            state.pending.insert(id.clone(), Pending { reply, update, dispatched: false });
        }
        let _guard = RequestGuard { bridge: self.clone(), id: id.clone() };
        // Mark the publication boundary while holding the pending owner. Never
        // call the output actor under that lock (it can synchronously reply).
        let dispatch = {
            let mut state = self.state.lock().unwrap();
            if cancel.is_cancelled() {
                if let Some(pending) = state.pending.remove(&id) {
                    let _ = pending.reply.send(Reply::Interrupted { message: aborted.clone(), dispatched: false });
                }
                false
            } else if let Some(pending) = state.pending.get_mut(&id) {
                pending.dispatched = true;
                true
            } else {
                false
            }
        };
        if dispatch {
            (self.output)(WireValue::object(vec![
                ("type", WireValue::String("host_tool_call".into())),
                ("id", WireValue::String(id.clone())),
                ("toolCallId", WireValue::String(call_id.into())),
                ("toolName", WireValue::String(definition.name.clone().into())),
                ("arguments", WireValue::parse(&Value::Object(args).to_string()).expect("typed arguments serialize")),
            ]));
        }
        let reply = tokio::select! {
            biased;
            reply = &mut received => reply,
            _ = cancel.cancelled() => {
                let pending = self.state.lock().unwrap().pending.remove(&id);
                if let Some(pending) = pending {
                    if pending.dispatched { self.emit_cancel(&id); }
                    let _ = pending.reply.send(Reply::Interrupted { message: aborted, dispatched: pending.dispatched });
                }
                // If another path removed it first, await that path's winning
                // reply rather than replacing a known result with cancellation.
                received.await
            }
        };
        match reply {
            Ok(Reply::Output(output)) => Ok(output),
            Ok(Reply::HostError(error)) => Err(ToolError(error)),
            Ok(Reply::Interrupted { message, dispatched: true }) => Ok(unknown_effect(message)),
            Ok(Reply::Interrupted { message, dispatched: false }) => Ok(not_dispatched(message)),
            Err(_) => Ok(unknown_effect("Host tool request owner dropped after publication".into())),
        }
    }

    fn emit_cancel(&self, id: &WireString) {
        (self.output)(WireValue::object(vec![
            ("type", WireValue::String("host_tool_cancel".into())),
            ("id", WireValue::String(uuid::Uuid::now_v7().to_string().into())),
            ("targetId", WireValue::String(id.clone())),
        ]));
    }
}

struct RequestGuard {
    bridge: Arc<ToolBridge>,
    id: WireString,
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        let pending = self.bridge.state.lock().unwrap().pending.remove(&self.id);
        if pending.is_some_and(|pending| pending.dispatched) {
            self.bridge.emit_cancel(&self.id);
        }
    }
}

struct HostToolAdapter {
    definition: Arc<HostToolDefinition>,
    bridge: Arc<ToolBridge>,
}

#[async_trait]
impl AgentTool for HostToolAdapter {
    fn definition(&self) -> Tool {
        self.definition.schema.clone()
    }

    async fn execute(
        &self,
        call_id: &str,
        args: JsonObject,
        cancel: CancellationToken,
        update: UpdateFn,
    ) -> Result<ToolOutput, ToolError> {
        self.bridge.execute(&self.definition, call_id, args, cancel, update).await
    }
}

fn decode_output(result: &WireValue) -> Result<ToolOutput, String> {
    let content: Vec<UserBlock> = serde_json::from_str(&result.get("content").expect("guarded content").stringify())
        .map_err(|error| format!("Host tool content cannot enter typed Core: {error}"))?;
    let details = result
        .get("details")
        .map(|details| serde_json::from_str(&details.stringify()))
        .transpose()
        .map_err(|error| format!("Host tool details cannot enter typed Core: {error}"))?;
    Ok(ToolOutput { content, details, is_error: false })
}

fn wire_truthy(value: &WireValue) -> bool {
    match value {
        WireValue::Null => false,
        WireValue::Bool(value) => *value,
        WireValue::Number(value) => *value != 0.0 && !value.is_nan(),
        WireValue::String(value) => !value.is_empty(),
        WireValue::Array(_) | WireValue::Object(_) => true,
    }
}

fn unknown_effect(message: String) -> ToolOutput {
    ToolOutput::error(format!("Host tool call effect unknown; do not replay automatically: {message}"))
        .with_details(json!({"__synthetic":true,"source":"interrupted_unknown_effect","executed":"unknown","transport":"rpc_host_tool"}))
}

fn not_dispatched(message: String) -> ToolOutput {
    ToolOutput::error(message).with_details(
        json!({"__synthetic":true,"source":"rpc_host_tool_preflight","executed":false,"transport":"rpc_host_tool"}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn wire(value: Value) -> WireValue {
        WireValue::parse(&value.to_string()).unwrap()
    }

    fn fixture() -> (Arc<ToolBridge>, mpsc::UnboundedReceiver<WireValue>) {
        let (output, frames) = mpsc::unbounded_channel();
        (
            ToolBridge::new(Arc::new(move |frame| {
                let _ = output.send(frame);
            })),
            frames,
        )
    }

    fn tool(bridge: &Arc<ToolBridge>, name: &str) -> Arc<dyn AgentTool> {
        let definitions = normalize_host_tool_definitions(&wire(json!([
            {"name":name,"description":"Host action","parameters":{"type":"object"}}
        ])))
        .unwrap();
        bridge.adapters(&definitions).pop().unwrap()
    }

    async fn next(frames: &mut mpsc::UnboundedReceiver<WireValue>) -> WireValue {
        tokio::time::timeout(std::time::Duration::from_secs(2), frames.recv()).await.unwrap().unwrap()
    }

    fn id(frame: &WireValue) -> String {
        string(frame.get("id")).unwrap()
    }

    #[test]
    fn normalization_follows_js_trim_metadata_and_object_schema() {
        let normalized = normalize_host_tool_definitions(&wire(json!([
            {"name":"\u{feff} read\u{a0}","label":"\t UI \n","description":"\u{feff} desc \u{a0}","parameters":{},"hidden":true},
            {"name":"custom","label":" ","description":"desc","parameters":{},"hidden":1},
            {"name":"write","description":"desc","parameters":{},"loadMode":"discoverable"}
        ]))).unwrap();
        assert_eq!(normalized[0].name, "read");
        assert_eq!(normalized[0].label, "UI");
        assert_eq!(normalized[0].schema.description, "desc");
        assert!(normalized[0].hidden);
        assert_eq!(normalized[0].load_mode, "essential");
        assert_eq!(normalized[1].label, "custom");
        assert!(!normalized[1].hidden);
        assert_eq!(normalized[1].load_mode, "discoverable");
        assert_eq!(normalized[2].load_mode, "discoverable");
        for schema in [Value::Null, json!([]), json!("schema")] {
            assert_eq!(
                normalize_host_tool_definitions(&wire(json!([
                    {"name":"x","description":"desc","parameters":schema}
                ])))
                .unwrap_err(),
                "Host tool \"x\" must provide a JSON Schema object"
            );
        }
        assert_eq!(
            normalize_host_tool_definitions(&wire(json!([{"name":" "}]))).unwrap_err(),
            "Host tool at index 0 must provide a non-empty name"
        );
        assert_eq!(
            normalize_host_tool_definitions(&wire(json!([{"name":"x","description":" "}]))).unwrap_err(),
            "Host tool \"x\" must provide a non-empty description"
        );
    }

    #[test]
    fn sidechannel_guard_does_not_depend_on_known_ids_or_typed_blocks() {
        let (bridge, _) = fixture();
        assert!(bridge.consume(&wire(json!({"type":"host_tool_result","id":"late","result":{"content":[null]}}))));
        assert!(bridge.consume(
            &WireValue::parse(r#"{"type":"host_tool_update","id":"\ud800","partialResult":{"content":[]}}"#).unwrap()
        ));
        for invalid in [
            json!({"type":"host_tool_result","id":1,"result":{"content":[]}}),
            json!({"type":"host_tool_result","id":"x","result":{"content":null}}),
            json!({"type":"host_tool_update","id":"x","result":{"content":[]}}),
            json!({"type":"prompt","id":"x","result":{"content":[]}}),
        ] {
            assert!(!bridge.consume(&wire(invalid)));
        }
    }

    #[tokio::test]
    async fn result_and_updates_preserve_typed_text_images_details_and_fresh_id() {
        let (bridge, mut frames) = fixture();
        let action = tool(&bridge, "original");
        let updates = Arc::new(Mutex::new(Vec::new()));
        let target = updates.clone();
        let pending = tokio::spawn(async move {
            action
                .execute(
                    "model-call",
                    json!({"value":7}).as_object().unwrap().clone(),
                    CancellationToken::new(),
                    Arc::new(move |output| target.lock().unwrap().push(output)),
                )
                .await
        });
        let call = next(&mut frames).await;
        assert_eq!(string(call.get("type")).unwrap(), "host_tool_call");
        assert_ne!(id(&call), "model-call");
        assert_eq!(string(call.get("toolCallId")).unwrap(), "model-call");
        assert_eq!(string(call.get("toolName")).unwrap(), "original");
        assert_eq!(call.get("arguments").unwrap().stringify(), r#"{"value":7}"#);
        let request_id = id(&call);
        assert!(bridge.consume(&wire(json!({"type":"host_tool_update","id":request_id,
            "partialResult":{"content":[{"type":"text","text":"partial"}],"details":{"fraction":0.5}}}))));
        assert_eq!(updates.lock().unwrap()[0].details, Some(json!({"fraction":0.5})));
        assert!(bridge.consume(&wire(json!({"type":"host_tool_result","id":request_id,
            "result":{"content":[{"type":"text","text":"done","textSignature":"signed"},
                {"type":"image","data":"AA==","mimeType":"image/png"}],"details":{"proof":true}}}))));
        let output = pending.await.unwrap().unwrap();
        assert_eq!(output.content.len(), 2);
        assert!(matches!(&output.content[1], UserBlock::Image(image) if image.mime_type == "image/png"));
        assert_eq!(output.details, Some(json!({"proof":true})));
        assert!(!output.is_error);
        assert!(bridge.consume(&wire(json!({"type":"host_tool_result","id":request_id,"result":{"content":[]}}))));
        assert!(bridge.state.lock().unwrap().pending.is_empty());
        assert!(frames.try_recv().is_err());
    }

    #[tokio::test]
    async fn update_callback_can_reenter_bridge_and_host_errors_join_js_trimmed_text() {
        let (bridge, mut frames) = fixture();
        let action = tool(&bridge, "x");
        let callback_bridge = bridge.clone();
        let pending = tokio::spawn(async move {
            action
                .execute(
                    "model",
                    JsonObject::new(),
                    CancellationToken::new(),
                    Arc::new(move |_| {
                        callback_bridge.reject_all_pending("reentrant update");
                    }),
                )
                .await
        });
        let call = next(&mut frames).await;
        assert!(
            bridge.consume(&wire(json!({"type":"host_tool_update","id":id(&call),"partialResult":{"content":[]}})))
        );
        let output = pending.await.unwrap().unwrap();
        assert_eq!(output.details.unwrap()["executed"], "unknown");

        let action = tool(&bridge, "x");
        let pending = tokio::spawn(async move {
            action.execute("model", JsonObject::new(), CancellationToken::new(), Arc::new(|_| {})).await
        });
        let call = next(&mut frames).await;
        assert!(bridge.consume(&wire(json!({"type":"host_tool_result","id":id(&call),"isError":true,
            "result":{"content":[{"type":"text","text":"\u{feff} first"},null,
                {"type":"image","data":"ignored"},{"type":"text","text":"last\u{a0}"}]}}))));
        assert_eq!(pending.await.unwrap().unwrap_err(), ToolError("first\nlast".into()));
    }

    #[tokio::test]
    async fn preabort_has_no_frame_postabort_cancels_once_and_records_unknown() {
        let (bridge, mut frames) = fixture();
        let action = tool(&bridge, "x");
        let cancel = CancellationToken::new();
        cancel.cancel();
        let output = action.execute("pre", JsonObject::new(), cancel, Arc::new(|_| {})).await.unwrap();
        assert_eq!(output.details.unwrap()["executed"], false);
        assert!(frames.try_recv().is_err());
        let cancel = CancellationToken::new();
        let run_cancel = cancel.clone();
        let pending =
            tokio::spawn(async move { action.execute("post", JsonObject::new(), run_cancel, Arc::new(|_| {})).await });
        let call = next(&mut frames).await;
        cancel.cancel();
        let cancelled = next(&mut frames).await;
        assert_eq!(string(cancelled.get("type")).unwrap(), "host_tool_cancel");
        assert_eq!(string(cancelled.get("targetId")).unwrap(), id(&call));
        assert_ne!(id(&cancelled), id(&call));
        let output = pending.await.unwrap().unwrap();
        assert!(output.is_error);
        assert_eq!(output.details.unwrap()["executed"], "unknown");
        assert!(bridge.consume(&wire(json!({"type":"host_tool_result","id":id(&call),"result":{"content":[]}}))));
        assert!(frames.try_recv().is_err());
    }

    #[tokio::test]
    async fn close_rejects_active_and_future_calls_and_drop_cleans_pending() {
        let (bridge, mut frames) = fixture();
        let action = tool(&bridge, "x");
        let old = action.clone();
        let pending = tokio::spawn(async move {
            old.execute("old", JsonObject::new(), CancellationToken::new(), Arc::new(|_| {})).await
        });
        next(&mut frames).await;
        bridge.close("disconnected");
        bridge.close("later close");
        assert_eq!(pending.await.unwrap().unwrap().details.unwrap()["executed"], "unknown");
        let output =
            action.execute("future", JsonObject::new(), CancellationToken::new(), Arc::new(|_| {})).await.unwrap();
        assert_eq!(output.details.unwrap()["executed"], false);
        assert!(frames.try_recv().is_err());

        let (bridge, mut frames) = fixture();
        let action = tool(&bridge, "x");
        let pending = tokio::spawn(async move {
            action.execute("drop", JsonObject::new(), CancellationToken::new(), Arc::new(|_| {})).await
        });
        let call = next(&mut frames).await;
        pending.abort();
        let cancelled = next(&mut frames).await;
        assert_eq!(string(cancelled.get("targetId")).unwrap(), id(&call));
        assert!(pending.await.unwrap_err().is_cancelled());
        assert!(bridge.state.lock().unwrap().pending.is_empty());
        assert!(frames.try_recv().is_err());
    }

    #[tokio::test]
    async fn adapter_keeps_old_definition_and_reject_pending_does_not_close_bridge() {
        let (bridge, mut frames) = fixture();
        let old = tool(&bridge, "old");
        let _replacement = tool(&bridge, "new");
        let pending = tokio::spawn(async move {
            old.execute("old", JsonObject::new(), CancellationToken::new(), Arc::new(|_| {})).await
        });
        let call = next(&mut frames).await;
        assert_eq!(string(call.get("toolName")).unwrap(), "old");
        bridge.reject_all_pending("refresh rejected");
        assert_eq!(pending.await.unwrap().unwrap().details.unwrap()["executed"], "unknown");
        let new = tool(&bridge, "new");
        let pending = tokio::spawn(async move {
            new.execute("new", JsonObject::new(), CancellationToken::new(), Arc::new(|_| {})).await
        });
        let call = next(&mut frames).await;
        assert_eq!(string(call.get("toolName")).unwrap(), "new");
        bridge.consume(&wire(json!({"type":"host_tool_result","id":id(&call),"result":{"content":[]}})));
        assert!(!pending.await.unwrap().unwrap().is_error);
    }
}
