//! Provider-bound history repair (subset of OMP
//! `packages/ai/src/providers/transform-messages.ts`, second pass).
//!
//! Guarantees every assistant tool call is immediately followed by exactly one
//! result: missing results become `No result provided` (or `aborted` after an
//! errored/aborted turn), duplicate results are dropped, and orphan results
//! whose call is gone are demoted to a user-role `<stale-tool-result>` note so
//! stale tool output never gains instruction priority. Truncated
//! (`length`/`error`/`aborted`) assistant turns with neither text nor tool
//! calls are dropped.
//!
//! Not ported yet: cross-provider thinking/signature rewrites, tool-call id
//! normalization per target, pulling a non-contiguous real result into its
//! call's window (`takeRealToolResult`), image stripping for non-vision models.

use crate::types::{
    AssistantBlock, AssistantMessage, Message, StopReason, ToolCall, ToolResultMessage, UserBlock, UserContent,
    UserMessage, now_ms,
};
use std::collections::{HashMap, HashSet};

fn is_truncated_empty_assistant(msg: &AssistantMessage) -> bool {
    matches!(msg.stop_reason, StopReason::Length | StopReason::Error | StopReason::Aborted)
        && !msg.content.iter().any(|b| match b {
            AssistantBlock::Text(t) => !t.text.trim().is_empty(),
            AssistantBlock::ToolCall(_) => true,
            _ => false,
        })
}

fn synthetic_result(call: &ToolCall, text: &str, timestamp: i64) -> Message {
    Message::ToolResult(ToolResultMessage {
        tool_call_id: call.id.clone(),
        tool_name: call.name.clone(),
        content: vec![UserBlock::text(text)],
        details: None,
        is_error: true,
        timestamp,
    })
}

pub fn transform_messages(messages: &[Message]) -> Vec<Message> {
    let valid_ids: HashSet<&str> =
        messages.iter().filter_map(Message::as_assistant).flat_map(|m| m.tool_calls().map(|c| c.id.as_str())).collect();
    let mut out: Vec<Message> = Vec::with_capacity(messages.len());
    let mut pending: Vec<ToolCall> = Vec::new();
    let mut pending_aborted: HashMap<String, ToolCall> = HashMap::new();
    let mut pending_aborted_order: Vec<String> = Vec::new();
    let mut pending_aborted_ts: Option<i64> = None;
    let mut resolved: HashSet<String> = HashSet::new();

    let flush = |out: &mut Vec<Message>,
                 pending: &mut Vec<ToolCall>,
                 pending_aborted: &mut HashMap<String, ToolCall>,
                 order: &mut Vec<String>,
                 aborted_ts: &mut Option<i64>,
                 resolved: &mut HashSet<String>,
                 ts: i64| {
        for call in pending.drain(..) {
            if resolved.insert(call.id.clone()) {
                out.push(synthetic_result(&call, "No result provided", ts));
            }
        }
        if let Some(ats) = aborted_ts.take() {
            for id in order.drain(..) {
                if let Some(call) = pending_aborted.remove(&id)
                    && resolved.insert(call.id.clone())
                {
                    out.push(synthetic_result(&call, "aborted", ats));
                }
            }
            pending_aborted.clear();
        }
    };

    for msg in messages {
        match msg {
            Message::Assistant(a) => {
                flush(
                    &mut out,
                    &mut pending,
                    &mut pending_aborted,
                    &mut pending_aborted_order,
                    &mut pending_aborted_ts,
                    &mut resolved,
                    a.timestamp,
                );
                if is_truncated_empty_assistant(a) {
                    continue;
                }
                let calls: Vec<ToolCall> = a.tool_calls().cloned().collect();
                if matches!(a.stop_reason, StopReason::Error | StopReason::Aborted) {
                    pending_aborted_order = calls.iter().map(|c| c.id.clone()).collect();
                    pending_aborted = calls.into_iter().map(|c| (c.id.clone(), c)).collect();
                    pending_aborted_ts = Some(a.timestamp);
                } else {
                    pending = calls;
                }
                out.push(msg.clone());
            }
            Message::ToolResult(r) => {
                if resolved.contains(&r.tool_call_id) {
                    continue;
                }
                if pending_aborted.remove(&r.tool_call_id).is_some() {
                    pending_aborted_order.retain(|id| id != &r.tool_call_id);
                    resolved.insert(r.tool_call_id.clone());
                    out.push(msg.clone());
                    continue;
                }
                if pending.iter().any(|c| c.id == r.tool_call_id) {
                    resolved.insert(r.tool_call_id.clone());
                    out.push(msg.clone());
                    continue;
                }
                if !valid_ids.contains(r.tool_call_id.as_str()) {
                    let window_open = pending.iter().any(|c| !resolved.contains(&c.id)) || !pending_aborted.is_empty();
                    if window_open {
                        continue;
                    }
                    let texts: Vec<&str> = r
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            UserBlock::Text(t) if !t.text.trim().is_empty() => Some(t.text.as_str()),
                            _ => None,
                        })
                        .collect();
                    if !texts.is_empty() {
                        let err = if r.is_error { " is-error=\"true\"" } else { "" };
                        out.push(Message::User(UserMessage {
                            content: UserContent::Text(format!(
                                "<stale-tool-result tool=\"{}\" id=\"{}\"{err}>\n{}\n</stale-tool-result>",
                                r.tool_name,
                                r.tool_call_id,
                                texts.join("\n")
                            )),
                            synthetic: None,
                            timestamp: r.timestamp,
                        }));
                    }
                }
                // Result for a call outside the open window: dropped (see module docs).
            }
            Message::User(u) => {
                flush(
                    &mut out,
                    &mut pending,
                    &mut pending_aborted,
                    &mut pending_aborted_order,
                    &mut pending_aborted_ts,
                    &mut resolved,
                    u.timestamp,
                );
                out.push(msg.clone());
            }
            Message::Developer(d) => {
                flush(
                    &mut out,
                    &mut pending,
                    &mut pending_aborted,
                    &mut pending_aborted_order,
                    &mut pending_aborted_ts,
                    &mut resolved,
                    d.timestamp,
                );
                out.push(msg.clone());
            }
        }
    }
    flush(
        &mut out,
        &mut pending,
        &mut pending_aborted,
        &mut pending_aborted_order,
        &mut pending_aborted_ts,
        &mut resolved,
        now_ms(),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::JsonObject;

    fn assistant(calls: &[&str], stop: StopReason) -> Message {
        let mut m = AssistantMessage::empty("openai-completions", "p", "m");
        m.stop_reason = stop;
        for id in calls {
            m.content.push(AssistantBlock::ToolCall(ToolCall {
                id: id.to_string(),
                name: "read".into(),
                arguments: JsonObject::new(),
                thought_signature: None,
            }));
        }
        Message::Assistant(m)
    }

    fn result(id: &str) -> Message {
        Message::ToolResult(ToolResultMessage {
            tool_call_id: id.into(),
            tool_name: "read".into(),
            content: vec![UserBlock::text("ok")],
            details: None,
            is_error: false,
            timestamp: 1,
        })
    }

    fn texts(msgs: &[Message]) -> Vec<String> {
        msgs.iter()
            .map(|m| match m {
                Message::ToolResult(r) => format!(
                    "result:{}:{}",
                    r.tool_call_id,
                    match &r.content[0] {
                        UserBlock::Text(t) => t.text.clone(),
                        _ => String::new(),
                    }
                ),
                Message::User(u) => format!("user:{}", u.content.plain_text()),
                other => other.role().to_string(),
            })
            .collect()
    }

    #[test]
    fn missing_results_are_synthesized_and_duplicates_dropped() {
        let msgs = vec![
            assistant(&["a", "b"], StopReason::ToolUse),
            result("a"),
            result("a"),
            Message::User(UserMessage::text("next")),
        ];
        assert_eq!(
            texts(&transform_messages(&msgs)),
            vec!["assistant", "result:a:ok", "result:b:No result provided", "user:next"]
        );
    }

    #[test]
    fn aborted_turn_gets_aborted_placeholders() {
        let msgs = vec![assistant(&["a"], StopReason::Aborted), Message::User(UserMessage::text("again"))];
        assert_eq!(texts(&transform_messages(&msgs)), vec!["assistant", "result:a:aborted", "user:again"]);
    }

    #[test]
    fn orphan_result_becomes_stale_note() {
        let msgs = vec![Message::User(UserMessage::text("q")), result("gone")];
        let out = texts(&transform_messages(&msgs));
        assert_eq!(out[1], "user:<stale-tool-result tool=\"read\" id=\"gone\">\nok\n</stale-tool-result>");
    }

    #[test]
    fn truncated_empty_assistant_is_dropped() {
        let msgs = vec![Message::User(UserMessage::text("q")), assistant(&[], StopReason::Error)];
        assert_eq!(texts(&transform_messages(&msgs)), vec!["user:q"]);
    }
}
