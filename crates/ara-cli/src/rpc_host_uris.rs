// Ported from OMP host-uris.ts / internal-urls/parse.ts at
// 596f2da7101178214aa27a753529d15e6b7ad91d. MIT notices: THIRD_PARTY.md.
//! Instance-bound Host content URI routing and bidirectional requests.

use ara_agent::ToolError;
use ara_rpc::{WireString, WireValue};
use ara_tools::{ContentUriPort, ContentUriRoute, UriResource};
use async_trait::async_trait;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub type BridgeOutput = Arc<dyn Fn(WireValue) + Send + Sync>;

#[derive(Clone)]
struct SchemeDefinition {
    scheme: String,
    writable: bool,
    immutable: bool,
}

struct Failure {
    message: String,
    dispatched: bool,
}

struct Pending {
    reply: oneshot::Sender<Result<WireValue, Failure>>,
    dispatched: bool,
}

#[derive(Default)]
struct State {
    // Vec preserves the insertion position of a normalized duplicate, as Map
    // does in fixed OMP. Replacing its value does not move the scheme.
    definitions: Vec<SchemeDefinition>,
    removed: HashSet<String>,
    pending: HashMap<WireString, Pending>,
    connection_closed: Option<String>,
}

pub struct UriBridge {
    output: BridgeOutput,
    state: Mutex<State>,
}

impl UriBridge {
    pub fn new(output: BridgeOutput) -> Arc<Self> {
        Arc::new(Self { output, state: Mutex::new(State::default()) })
    }

    #[cfg(test)]
    fn schemes(&self) -> Vec<String> {
        self.state.lock().unwrap().definitions.iter().map(|definition| definition.scheme.clone()).collect()
    }

    /// Complete validation precedes publication. Only `security` is reserved;
    /// a Host may explicitly replace `skill` and other native namespaces.
    pub fn set_schemes(&self, schemes: &WireValue) -> Result<Vec<String>, String> {
        let schemes = schemes.as_array().ok_or("Host URI schemes must be an array")?;
        let mut definitions: Vec<SchemeDefinition> = Vec::new();
        for raw in schemes {
            let original = string(raw.get("scheme")).unwrap_or_default();
            let scheme = ara_prompt::js::trim(&original).to_lowercase();
            if scheme.is_empty() {
                return Err("Host URI scheme must be a non-empty string".into());
            }
            if !valid_scheme(&scheme) {
                return Err(format!("Host URI scheme contains invalid characters: {original}"));
            }
            if scheme == "security" {
                return Err(format!("Host URI scheme is reserved by OMP: {scheme}://"));
            }
            let definition = SchemeDefinition {
                scheme: scheme.clone(),
                writable: matches!(raw.get("writable"), Some(WireValue::Bool(true))),
                immutable: matches!(raw.get("immutable"), Some(WireValue::Bool(true))),
            };
            if let Some(previous) = definitions.iter_mut().find(|definition| definition.scheme == scheme) {
                *previous = definition;
            } else {
                definitions.push(definition);
            }
        }
        let names = definitions.iter().map(|definition| definition.scheme.clone()).collect::<Vec<_>>();
        let mut state = self.state.lock().unwrap();
        let dropped = state
            .definitions
            .iter()
            .filter(|definition| !names.contains(&definition.scheme))
            .map(|definition| definition.scheme.clone())
            .collect::<Vec<_>>();
        state.removed.extend(dropped);
        for name in &names {
            state.removed.remove(name);
        }
        state.definitions = definitions;
        // Pending requests retain their original operation/URL. Replacing a
        // scheme does not cancel them; read defaults use the current definition.
        Ok(names)
    }

    /// Guard checks only type and string ID, exactly as fixed rpc-mode does.
    /// Unknown or late IDs are consumed without an ordinary command ACK.
    pub fn consume(&self, frame: &WireValue) -> bool {
        if !frame.get("type").and_then(WireValue::as_string).is_some_and(|kind| kind.equals_ascii("host_uri_result")) {
            return false;
        }
        let Some(id) = frame.get("id").and_then(WireValue::as_string) else { return false };
        let pending = self.state.lock().unwrap().pending.remove(id);
        if let Some(pending) = pending {
            let _ = pending.reply.send(Ok(frame.clone()));
        }
        true
    }

    /// Unregisters and rejects current waits, without permanently closing the
    /// bridge. Dropped explicit overrides never restore a native handler.
    #[cfg(test)]
    fn clear(&self, message: &str) {
        let pending = {
            let mut state = self.state.lock().unwrap();
            let definitions = std::mem::take(&mut state.definitions);
            state.removed.extend(definitions.into_iter().map(|definition| definition.scheme));
            std::mem::take(&mut state.pending)
        };
        Self::reject(pending, message);
    }

    /// Reference RPC connection policy, separate from OMP's reusable `clear`.
    /// EOF must prevent an accepted Run from creating a fresh wait after its
    /// peer has gone. Close and request admission share this one state lock.
    pub fn close_connection(&self, message: &str) {
        let (pending, message) = {
            let mut state = self.state.lock().unwrap();
            let message = state.connection_closed.get_or_insert_with(|| message.into()).clone();
            let definitions = std::mem::take(&mut state.definitions);
            state.removed.extend(definitions.into_iter().map(|definition| definition.scheme));
            (std::mem::take(&mut state.pending), message)
        };
        Self::reject(pending, &message);
    }

    fn reject(pending: HashMap<WireString, Pending>, message: &str) {
        for pending in pending.into_values() {
            let _ = pending.reply.send(Err(Failure { message: message.into(), dispatched: pending.dispatched }));
        }
    }

    /// Direct requests stay callable after clear, matching the fixed bridge.
    /// Ordinary read/write tools check the route before entering these methods.
    pub async fn request_read(&self, url: &str, cancel: CancellationToken) -> Result<UriResource, ToolError> {
        let (scheme, href) = canonical_internal_url(url)?;
        let result = self.dispatch("read", &href, None, cancel).await?;
        if result.get("isError").is_some_and(wire_truthy) {
            return Err(ToolError(result_error(&result, "read", &href)));
        }
        let content = optional_string(result.get("content"), "content")?.unwrap_or_default();
        let content_type =
            optional_string(result.get("contentType"), "contentType")?.unwrap_or_else(|| "text/plain".into());
        let notes = match result.get("notes") {
            None | Some(WireValue::Null) => Vec::new(),
            Some(WireValue::Array(notes)) => notes
                .iter()
                .map(|note| {
                    optional_string(Some(note), "notes item")
                        .and_then(|note| note.ok_or_else(|| invalid_read("notes item must be a string")))
                })
                .collect::<Result<Vec<_>, _>>()?,
            Some(_) => return Err(invalid_read("notes must be an array of strings")),
        };
        // Read this after completion, not from a dispatch-time snapshot. false
        // is a valid per-result override even when the scheme is immutable.
        let default_immutable = self
            .state
            .lock()
            .unwrap()
            .definitions
            .iter()
            .find(|definition| definition.scheme == scheme)
            .is_some_and(|definition| definition.immutable);
        let immutable = match result.get("immutable") {
            Some(WireValue::Bool(value)) => *value,
            _ => default_immutable,
        };
        Ok(UriResource { content, content_type, notes, immutable })
    }

    pub async fn request_write(&self, url: &str, content: &str, cancel: CancellationToken) -> Result<(), ToolError> {
        let (_, href) = canonical_internal_url(url)?;
        let result = self.dispatch("write", &href, Some(content), cancel).await?;
        if result.get("isError").is_some_and(wire_truthy) {
            Err(ToolError(result_error(&result, "write", &href)))
        } else {
            // The fixed write completion ignores all successful payload fields.
            Ok(())
        }
    }

    async fn dispatch(
        &self,
        operation: &str,
        url: &str,
        content: Option<&str>,
        cancel: CancellationToken,
    ) -> Result<WireValue, ToolError> {
        let aborted = format!("Host URI {operation} for {url} was aborted");
        if cancel.is_cancelled() {
            return Err(ToolError(format!("{aborted} before dispatch; not executed")));
        }
        let id = WireString::from(uuid::Uuid::now_v7().to_string());
        let (reply, mut received) = oneshot::channel();
        {
            let mut state = self.state.lock().unwrap();
            if let Some(message) = &state.connection_closed {
                return Err(ToolError(format!("{message} before dispatch; not executed")));
            }
            if cancel.is_cancelled() {
                return Err(ToolError(format!("{aborted} before dispatch; not executed")));
            }
            state.pending.insert(id.clone(), Pending { reply, dispatched: false });
        }
        let _guard = RequestGuard { bridge: self, id: id.clone() };
        let dispatch = {
            let mut state = self.state.lock().unwrap();
            if cancel.is_cancelled() {
                if let Some(pending) = state.pending.remove(&id) {
                    let _ = pending.reply.send(Err(Failure { message: aborted.clone(), dispatched: false }));
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
            let mut frame = WireValue::object(vec![
                ("type", WireValue::String("host_uri_request".into())),
                ("id", WireValue::String(id.clone())),
                ("operation", WireValue::String(operation.into())),
                ("url", WireValue::String(url.into())),
            ]);
            if operation == "write" {
                frame.insert("content", WireValue::String(content.unwrap_or("").into()));
            }
            (self.output)(frame);
        }
        let reply = tokio::select! {
            biased;
            reply = &mut received => reply,
            _ = cancel.cancelled() => {
                let pending = self.state.lock().unwrap().pending.remove(&id);
                if let Some(pending) = pending {
                    if pending.dispatched { self.emit_cancel(&id); }
                    let _ = pending.reply.send(Err(Failure { message: aborted, dispatched: pending.dispatched }));
                }
                received.await
            }
        };
        match reply {
            Ok(Ok(frame)) => Ok(frame),
            Ok(Err(failure)) => Err(ToolError(if failure.dispatched {
                format!("{} after dispatch; effect unknown; do not replay automatically", failure.message)
            } else {
                format!("{} before dispatch; not executed", failure.message)
            })),
            Err(_) => Err(ToolError(
                "Host URI request owner dropped after dispatch; effect unknown; do not replay automatically".into(),
            )),
        }
    }

    fn emit_cancel(&self, id: &WireString) {
        (self.output)(WireValue::object(vec![
            ("type", WireValue::String("host_uri_cancel".into())),
            ("id", WireValue::String(uuid::Uuid::now_v7().to_string().into())),
            ("targetId", WireValue::String(id.clone())),
        ]));
    }
}

struct RequestGuard<'a> {
    bridge: &'a UriBridge,
    id: WireString,
}

impl Drop for RequestGuard<'_> {
    fn drop(&mut self) {
        let pending = self.bridge.state.lock().unwrap().pending.remove(&self.id);
        if pending.is_some_and(|pending| pending.dispatched) {
            self.bridge.emit_cancel(&self.id);
        }
    }
}

#[async_trait]
impl ContentUriPort for UriBridge {
    fn route(&self, url: &str) -> ContentUriRoute {
        let Some(scheme) = extract_scheme(url) else { return ContentUriRoute::Unregistered };
        let state = self.state.lock().unwrap();
        if let Some(definition) = state.definitions.iter().find(|definition| definition.scheme == scheme) {
            ContentUriRoute::Registered { writable: definition.writable }
        } else if state.removed.contains(&scheme) {
            ContentUriRoute::Removed
        } else {
            ContentUriRoute::Unregistered
        }
    }

    async fn read(&self, url: &str, cancel: CancellationToken) -> Result<UriResource, ToolError> {
        self.request_read(url, cancel).await
    }

    async fn write(&self, url: &str, content: &str, cancel: CancellationToken) -> Result<(), ToolError> {
        self.request_write(url, content, cancel).await
    }
}

fn valid_scheme(scheme: &str) -> bool {
    let mut bytes = scheme.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"+.-".contains(&byte))
}

fn extract_scheme(url: &str) -> Option<String> {
    let (scheme, _) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    valid_scheme(&scheme).then_some(scheme)
}

/// WHATWG-compatible URL serialization, with the fixed internal parser's raw
/// fallback for namespaced authorities whose colon is not a valid URL port.
fn canonical_internal_url(url: &str) -> Result<(String, String), ToolError> {
    let scheme = extract_scheme(url).ok_or_else(|| ToolError(format!("Invalid URL: {url}")))?;
    let href = reqwest::Url::parse(url).map(|url| url.to_string()).unwrap_or_else(|_| url.to_owned());
    Ok((scheme, href))
}

fn string(value: Option<&WireValue>) -> Option<String> {
    value?.as_string()?.to_utf8().ok()
}

fn optional_string(value: Option<&WireValue>, field: &str) -> Result<Option<String>, ToolError> {
    match value {
        None | Some(WireValue::Null) => Ok(None),
        Some(value) => string(Some(value)).map(Some).ok_or_else(|| invalid_read(&format!("{field} must be a string"))),
    }
}

fn invalid_read(message: &str) -> ToolError {
    ToolError(format!(
        "Host URI read response cannot enter typed Core after dispatch; effect unknown; do not replay automatically: {message}"
    ))
}

fn result_error(result: &WireValue, operation: &str, url: &str) -> String {
    string(result.get("error"))
        .filter(|error| !error.is_empty())
        .or_else(|| string(result.get("content")).filter(|content| !content.is_empty()))
        .unwrap_or_else(|| format!("Host URI {operation} failed for {url}"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use tokio::sync::mpsc;

    fn wire(value: Value) -> WireValue {
        WireValue::parse(&value.to_string()).unwrap()
    }

    fn fixture() -> (Arc<UriBridge>, mpsc::UnboundedReceiver<WireValue>) {
        let (output, frames) = mpsc::unbounded_channel();
        (
            UriBridge::new(Arc::new(move |frame| {
                let _ = output.send(frame);
            })),
            frames,
        )
    }

    async fn next(frames: &mut mpsc::UnboundedReceiver<WireValue>) -> WireValue {
        tokio::time::timeout(std::time::Duration::from_secs(2), frames.recv()).await.unwrap().unwrap()
    }

    fn id(frame: &WireValue) -> String {
        string(frame.get("id")).unwrap()
    }

    #[test]
    fn normalization_is_atomic_and_duplicate_replacement_retains_map_position() {
        let (bridge, _) = fixture();
        assert_eq!(
            bridge
                .set_schemes(&wire(json!([
                    {"scheme":"\u{feff} DB \u{a0}","writable":true},
                    {"scheme":"notes","immutable":true},
                    {"scheme":"db","writable":false},
                    {"scheme":"skill","writable":true},
                ])))
                .unwrap(),
            vec!["db", "notes", "skill"]
        );
        assert_eq!(bridge.route("DB://item"), ContentUriRoute::Registered { writable: false });
        assert_eq!(bridge.route("skill://plugin:name"), ContentUriRoute::Registered { writable: true });
        assert_eq!(
            bridge.set_schemes(&wire(json!([{"scheme":"new"},{"scheme":"SECURITY"}]))).unwrap_err(),
            "Host URI scheme is reserved by OMP: security://"
        );
        assert_eq!(bridge.schemes(), vec!["db", "notes", "skill"]);
        assert!(
            bridge
                .set_schemes(&wire(json!([{"scheme":"db:"}])))
                .unwrap_err()
                .starts_with("Host URI scheme contains invalid characters:")
        );
        assert!(bridge.set_schemes(&wire(json!([{"scheme":" "}]))).is_err());
        assert_eq!(bridge.schemes(), vec!["db", "notes", "skill"]);
        bridge.set_schemes(&wire(json!([{"scheme":"db"}]))).unwrap();
        assert_eq!(bridge.route("skill://plugin:name"), ContentUriRoute::Removed);
        assert_eq!(bridge.route("native://item"), ContentUriRoute::Unregistered);
        bridge.clear("cleanup");
        assert_eq!(bridge.route("db://item"), ContentUriRoute::Removed);
        bridge.set_schemes(&wire(json!([{"scheme":"skill"}]))).unwrap();
        assert_eq!(bridge.route("skill://plugin:name"), ContentUriRoute::Registered { writable: false });
    }

    #[test]
    fn uri_guard_only_checks_type_and_string_id_and_url_has_namespaced_fallback() {
        let (bridge, _) = fixture();
        assert!(bridge.consume(&wire(json!({"type":"host_uri_result","id":"late","content":7}))));
        assert!(bridge.consume(&WireValue::parse(r#"{"type":"host_uri_result","id":"\ud800"}"#).unwrap()));
        assert!(!bridge.consume(&wire(json!({"type":"host_uri_result","id":1}))));
        assert!(!bridge.consume(&wire(json!({"type":"host_tool_result","id":"x"}))));
        let (scheme, href) = canonical_internal_url("DB://Item/hello world?q=甲").unwrap();
        assert_eq!(scheme, "db");
        assert_eq!(href, "db://Item/hello%20world?q=%E7%94%B2");
        assert_eq!(canonical_internal_url("skill://plugin:name/图 文.md").unwrap().1, "skill://plugin:name/图 文.md");
        assert_eq!(canonical_internal_url("db://Item/file:raw:2-4").unwrap().1, "db://Item/file:raw:2-4");
        assert!(canonical_internal_url("db:item").is_err());
        assert_eq!(bridge.route("C:\\local\\path"), ContentUriRoute::Unregistered);
    }

    #[tokio::test]
    async fn read_preserves_utf8_content_notes_type_and_uses_current_immutable_definition() {
        let (bridge, mut frames) = fixture();
        bridge.set_schemes(&wire(json!([{"scheme":"db"}]))).unwrap();
        let request_bridge = bridge.clone();
        let pending =
            tokio::spawn(async move { request_bridge.read("DB://Item/hello world", CancellationToken::new()).await });
        let call = next(&mut frames).await;
        assert_eq!(string(call.get("type")).unwrap(), "host_uri_request");
        assert_eq!(string(call.get("operation")).unwrap(), "read");
        assert_eq!(string(call.get("url")).unwrap(), "db://Item/hello%20world");
        assert!(call.get("content").is_none());
        bridge.set_schemes(&wire(json!([{"scheme":"db","immutable":true}]))).unwrap();
        assert!(bridge.consume(&wire(json!({"type":"host_uri_result","id":id(&call),
            "content":"甲\nβ","contentType":"text/markdown","notes":["source note"]}))));
        let resource = pending.await.unwrap().unwrap();
        assert_eq!(resource.content, "甲\nβ");
        assert_eq!(resource.content.len(), 6);
        assert_eq!(resource.content_type, "text/markdown");
        assert_eq!(resource.notes, vec!["source note"]);
        assert!(resource.immutable);

        let request_bridge = bridge.clone();
        let pending = tokio::spawn(async move { request_bridge.read("db://Item", CancellationToken::new()).await });
        let call = next(&mut frames).await;
        bridge.consume(&wire(json!({"type":"host_uri_result","id":id(&call),"immutable":false})));
        let resource = pending.await.unwrap().unwrap();
        assert_eq!(resource.content, "");
        assert_eq!(resource.content_type, "text/plain");
        assert!(resource.notes.is_empty());
        assert!(!resource.immutable);
    }

    #[tokio::test]
    async fn write_retains_admitted_handler_and_ignores_success_payload() {
        let (bridge, mut frames) = fixture();
        bridge.set_schemes(&wire(json!([{"scheme":"db","writable":true}]))).unwrap();
        assert_eq!(bridge.route("db://Item"), ContentUriRoute::Registered { writable: true });
        let request_bridge = bridge.clone();
        let pending =
            tokio::spawn(async move { request_bridge.write("DB://Item", "甲 body", CancellationToken::new()).await });
        let call = next(&mut frames).await;
        assert_eq!(string(call.get("operation")).unwrap(), "write");
        assert_eq!(string(call.get("content")).unwrap(), "甲 body");
        bridge.set_schemes(&wire(json!([{"scheme":"db","writable":false}]))).unwrap();
        bridge.consume(&wire(json!({"type":"host_uri_result","id":id(&call),"content":["ignored"],"notes":7})));
        pending.await.unwrap().unwrap();
        assert_eq!(bridge.route("db://Item"), ContentUriRoute::Registered { writable: false });
    }

    #[tokio::test]
    async fn errors_prefer_error_then_content_then_fixed_fallback() {
        let (bridge, mut frames) = fixture();
        for (error, content, expected) in [
            (json!("preferred"), json!("content"), "preferred"),
            (json!(""), json!("content"), "content"),
            (Value::Null, Value::Null, "Host URI read failed for db://Item"),
        ] {
            let request_bridge = bridge.clone();
            let pending =
                tokio::spawn(async move { request_bridge.request_read("db://Item", CancellationToken::new()).await });
            let call = next(&mut frames).await;
            bridge.consume(&wire(
                json!({"type":"host_uri_result","id":id(&call),"isError":true,"error":error,"content":content}),
            ));
            assert_eq!(pending.await.unwrap().unwrap_err(), ToolError(expected.into()));
        }
    }

    #[tokio::test]
    async fn preabort_has_no_request_and_postabort_emits_one_cancel_with_unknown_effect() {
        let (bridge, mut frames) = fixture();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let error = bridge.request_read("db://Item", cancel).await.unwrap_err();
        assert!(error.0.contains("before dispatch; not executed"));
        assert!(frames.try_recv().is_err());
        let cancel = CancellationToken::new();
        let run_cancel = cancel.clone();
        let request_bridge = bridge.clone();
        let pending = tokio::spawn(async move { request_bridge.request_write("db://Item", "body", run_cancel).await });
        let call = next(&mut frames).await;
        cancel.cancel();
        let cancelled = next(&mut frames).await;
        assert_eq!(string(cancelled.get("type")).unwrap(), "host_uri_cancel");
        assert_eq!(string(cancelled.get("targetId")).unwrap(), id(&call));
        assert_ne!(id(&cancelled), id(&call));
        let error = pending.await.unwrap().unwrap_err();
        assert!(error.0.contains("after dispatch; effect unknown; do not replay automatically"));
        assert!(bridge.consume(&wire(json!({"type":"host_uri_result","id":id(&call)}))));
        assert!(bridge.state.lock().unwrap().pending.is_empty());
        assert!(frames.try_recv().is_err());
    }

    #[tokio::test]
    async fn clear_rejects_pending_but_future_direct_requests_still_dispatch_and_can_settle() {
        let (bridge, mut frames) = fixture();
        bridge.set_schemes(&wire(json!([{"scheme":"skill","immutable":true}]))).unwrap();
        let request_bridge = bridge.clone();
        let pending =
            tokio::spawn(
                async move { request_bridge.request_read("skill://plugin:name", CancellationToken::new()).await },
            );
        next(&mut frames).await;
        bridge.clear("EOF");
        assert!(pending.await.unwrap().unwrap_err().0.contains("effect unknown"));
        assert_eq!(bridge.route("skill://plugin:name"), ContentUriRoute::Removed);
        let request_bridge = bridge.clone();
        let pending =
            tokio::spawn(
                async move { request_bridge.request_read("skill://plugin:name", CancellationToken::new()).await },
            );
        let call = next(&mut frames).await;
        bridge.consume(&wire(json!({"type":"host_uri_result","id":id(&call),"content":"later"})));
        assert!(!pending.await.unwrap().unwrap().immutable);
    }

    #[tokio::test]
    async fn connection_close_rejects_active_and_future_requests_with_first_reason() {
        let (bridge, mut frames) = fixture();
        bridge.set_schemes(&wire(json!([{"scheme":"db","writable":true}]))).unwrap();
        let request_bridge = bridge.clone();
        let pending =
            tokio::spawn(async move { request_bridge.request_read("db://Item", CancellationToken::new()).await });
        let call = next(&mut frames).await;
        bridge.close_connection("RPC peer EOF");
        bridge.close_connection("later reason");
        let error = pending.await.unwrap().unwrap_err();
        assert_eq!(error.0, "RPC peer EOF after dispatch; effect unknown; do not replay automatically");
        assert_eq!(bridge.route("db://Item"), ContentUriRoute::Removed);
        assert!(bridge.consume(&wire(json!({"type":"host_uri_result","id":id(&call),"content":"late"}))));
        assert!(bridge.state.lock().unwrap().pending.is_empty());
        // Registration remains an ordinary configuration operation, but it
        // cannot reopen the dead connection or admit a new pending request.
        bridge.set_schemes(&wire(json!([{"scheme":"db","writable":true}]))).unwrap();
        assert_eq!(bridge.route("db://Item"), ContentUriRoute::Registered { writable: true });
        let error = bridge.request_read("db://Item", CancellationToken::new()).await.unwrap_err();
        assert_eq!(error.0, "RPC peer EOF before dispatch; not executed");
        bridge.clear("ordinary cleanup");
        let error = bridge.request_read("db://Item", CancellationToken::new()).await.unwrap_err();
        assert_eq!(error.0, "RPC peer EOF before dispatch; not executed");
        assert!(bridge.state.lock().unwrap().pending.is_empty());
        assert!(frames.try_recv().is_err());
    }

    #[tokio::test]
    async fn route_admitted_write_then_connection_close_refuses_without_dispatch() {
        let (bridge, mut frames) = fixture();
        bridge.set_schemes(&wire(json!([{"scheme":"db","writable":true}]))).unwrap();
        // The tool may already have retained a writable handler before EOF.
        assert_eq!(bridge.route("db://Item"), ContentUriRoute::Registered { writable: true });
        bridge.close_connection("RPC peer EOF");
        let error = bridge.write("db://Item", "new body", CancellationToken::new()).await.unwrap_err();
        assert_eq!(error.0, "RPC peer EOF before dispatch; not executed");
        assert!(bridge.state.lock().unwrap().pending.is_empty());
        assert!(frames.try_recv().is_err());
    }

    #[tokio::test]
    async fn dropped_request_cleans_waiter_and_settled_reply_wins_ready_cancellation() {
        let (bridge, mut frames) = fixture();
        let request_bridge = bridge.clone();
        let pending =
            tokio::spawn(async move { request_bridge.request_read("db://Item", CancellationToken::new()).await });
        let call = next(&mut frames).await;
        pending.abort();
        assert_eq!(string(next(&mut frames).await.get("targetId")).unwrap(), id(&call));
        assert!(pending.await.unwrap_err().is_cancelled());
        assert!(bridge.state.lock().unwrap().pending.is_empty());

        let cancel = CancellationToken::new();
        let run_cancel = cancel.clone();
        let request_bridge = bridge.clone();
        let pending = tokio::spawn(async move { request_bridge.request_read("db://Item", run_cancel).await });
        let call = next(&mut frames).await;
        bridge.consume(&wire(json!({"type":"host_uri_result","id":id(&call),"content":"known receipt"})));
        cancel.cancel();
        assert_eq!(pending.await.unwrap().unwrap().content, "known receipt");
        assert!(frames.try_recv().is_err());
    }
}
