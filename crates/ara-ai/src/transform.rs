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
//! normalization per target, image stripping for non-vision models.

use crate::types::{
    AssistantBlock, AssistantMessage, Message, StopReason, ToolCall, ToolResultMessage, UserBlock, UserContent,
    UserMessage, now_ms,
};
use std::collections::{HashMap, HashSet, VecDeque};

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

fn append_duplicate_suffix(id: &str, suffix: &str) -> String {
    // OMP caps each segment so a suffix remains valid for strict replay
    // providers. Composite IDs receive the suffix on each segment.
    id.split('|')
        .map(|segment| {
            let keep = 64usize.saturating_sub(suffix.chars().count());
            format!("{}{}", segment.chars().take(keep).collect::<String>(), suffix)
        })
        .collect::<Vec<_>>()
        .join("|")
}

/// OMP's first pass gives repeated calls distinct IDs before the second pass
/// pulls delayed results. A new assistant turn supersedes an older pending
/// rewrite for the same ID, so its real result cannot be stolen by that turn.
fn deduplicate_tool_call_ids(messages: &[Message]) -> Vec<Message> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut pending: HashMap<String, VecDeque<Option<String>>> = HashMap::new();
    let mut out = Vec::with_capacity(messages.len());
    for message in messages {
        match message {
            Message::Assistant(assistant) => {
                let mut assistant = assistant.clone();
                let mut touched = HashSet::new();
                for block in &mut assistant.content {
                    let AssistantBlock::ToolCall(call) = block else { continue };
                    let original = call.id.clone();
                    if touched.insert(original.clone()) {
                        pending.remove(&original);
                    }
                    let count = seen.get(&original).copied().unwrap_or(0);
                    if count == 0 {
                        seen.insert(original.clone(), 1);
                        pending.entry(original).or_default().push_back(None);
                        continue;
                    }
                    let mut duplicate_index = count;
                    let replacement = loop {
                        let candidate = append_duplicate_suffix(&original, &format!("_dup{duplicate_index}"));
                        if !seen.contains_key(&candidate) {
                            break candidate;
                        }
                        duplicate_index += 1;
                    };
                    seen.insert(original.clone(), duplicate_index + 1);
                    seen.insert(replacement.clone(), 1);
                    pending.entry(original).or_default().push_back(Some(replacement.clone()));
                    call.id = replacement;
                }
                out.push(Message::Assistant(assistant));
            }
            Message::ToolResult(result) => {
                let mut result = result.clone();
                let original = result.tool_call_id.clone();
                if let Some(queue) = pending.get_mut(&original) {
                    if let Some(Some(replacement)) = queue.pop_front() {
                        result.tool_call_id = replacement;
                    }
                    if queue.is_empty() {
                        pending.remove(&original);
                    }
                }
                out.push(Message::ToolResult(result));
            }
            _ => out.push(message.clone()),
        }
    }
    out
}

fn take_real_result(
    id: &str,
    after_index: usize,
    messages: &[Message],
    result_indices: &HashMap<String, Vec<usize>>,
    consumed: &mut [bool],
) -> Option<Message> {
    for &index in result_indices.get(id)? {
        if index > after_index && !consumed[index] {
            consumed[index] = true;
            return Some(messages[index].clone());
        }
    }
    None
}

pub fn transform_messages(messages: &[Message]) -> Vec<Message> {
    let messages = deduplicate_tool_call_ids(messages);
    let mut result_indices: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, message) in messages.iter().enumerate() {
        if let Message::ToolResult(result) = message {
            result_indices.entry(result.tool_call_id.clone()).or_default().push(index);
        }
    }
    let mut consumed = vec![false; messages.len()];
    let valid_ids: HashSet<&str> =
        messages.iter().filter_map(Message::as_assistant).flat_map(|m| m.tool_calls().map(|c| c.id.as_str())).collect();
    let mut out: Vec<Message> = Vec::with_capacity(messages.len());
    let mut pending: Vec<ToolCall> = Vec::new();
    let mut pending_start = 0;
    let mut pending_aborted: HashMap<String, ToolCall> = HashMap::new();
    let mut pending_aborted_order: Vec<String> = Vec::new();
    let mut pending_aborted_ts: Option<i64> = None;
    let mut pending_aborted_start = 0;
    let mut resolved: HashSet<String> = HashSet::new();

    let flush = |out: &mut Vec<Message>,
                 pending: &mut Vec<ToolCall>,
                 pending_aborted: &mut HashMap<String, ToolCall>,
                 order: &mut Vec<String>,
                 aborted_ts: &mut Option<i64>,
                 resolved: &mut HashSet<String>,
                 ts: i64,
                 pending_start: usize,
                 aborted_start: usize,
                 consumed: &mut [bool]| {
        for call in pending.drain(..) {
            if resolved.insert(call.id.clone()) {
                out.push(
                    take_real_result(&call.id, pending_start, &messages, &result_indices, consumed)
                        .unwrap_or_else(|| synthetic_result(&call, "No result provided", ts)),
                );
            }
        }
        if let Some(ats) = aborted_ts.take() {
            for id in order.drain(..) {
                if let Some(call) = pending_aborted.remove(&id)
                    && resolved.insert(call.id.clone())
                {
                    out.push(
                        take_real_result(&call.id, aborted_start, &messages, &result_indices, consumed)
                            .unwrap_or_else(|| synthetic_result(&call, "aborted", ats)),
                    );
                }
            }
            pending_aborted.clear();
        }
    };

    for (index, msg) in messages.iter().enumerate() {
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
                    pending_start,
                    pending_aborted_start,
                    &mut consumed,
                );
                if is_truncated_empty_assistant(a) {
                    continue;
                }
                let calls: Vec<ToolCall> = a.tool_calls().cloned().collect();
                if matches!(a.stop_reason, StopReason::Error | StopReason::Aborted) {
                    pending_aborted_order = calls.iter().map(|c| c.id.clone()).collect();
                    pending_aborted = calls.into_iter().map(|c| (c.id.clone(), c)).collect();
                    pending_aborted_ts = Some(a.timestamp);
                    pending_aborted_start = index;
                } else {
                    pending = calls;
                    pending_start = index;
                }
                out.push(msg.clone());
            }
            Message::ToolResult(r) => {
                if consumed[index] {
                    continue;
                }
                if resolved.contains(&r.tool_call_id) {
                    continue;
                }
                if pending_aborted.remove(&r.tool_call_id).is_some() {
                    pending_aborted_order.retain(|id| id != &r.tool_call_id);
                    resolved.insert(r.tool_call_id.clone());
                    consumed[index] = true;
                    out.push(msg.clone());
                    continue;
                }
                if pending.iter().any(|c| c.id == r.tool_call_id) {
                    resolved.insert(r.tool_call_id.clone());
                    consumed[index] = true;
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
                    pending_start,
                    pending_aborted_start,
                    &mut consumed,
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
                    pending_start,
                    pending_aborted_start,
                    &mut consumed,
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
        pending_start,
        pending_aborted_start,
        &mut consumed,
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
        result_text(id, "ok")
    }

    fn result_text(id: &str, text: &str) -> Message {
        Message::ToolResult(ToolResultMessage {
            tool_call_id: id.into(),
            tool_name: "read".into(),
            content: vec![UserBlock::text(text)],
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

    fn developer(text: &str) -> Message {
        Message::Developer(crate::types::DeveloperMessage { content: UserContent::Text(text.into()), timestamp: 1 })
    }

    #[test]
    fn delayed_real_result_precedes_intervening_guidance() {
        let msgs = vec![assistant(&["a"], StopReason::ToolUse), developer("guidance"), result_text("a", "real output")];
        assert_eq!(texts(&transform_messages(&msgs)), vec!["assistant", "result:a:real output", "developer"]);
    }

    #[test]
    fn aborted_call_uses_later_real_result() {
        let msgs = vec![
            assistant(&["a"], StopReason::Aborted),
            Message::User(UserMessage::text("continue")),
            result_text("a", "partial output"),
        ];
        assert_eq!(texts(&transform_messages(&msgs)), vec!["assistant", "result:a:partial output", "user:continue"]);
    }

    #[test]
    fn earlier_orphan_is_not_taken_for_later_call() {
        let msgs = vec![
            result_text("a", "old output"),
            Message::User(UserMessage::text("again")),
            assistant(&["a"], StopReason::ToolUse),
            developer("guidance"),
            result_text("a", "new output"),
        ];
        assert_eq!(
            texts(&transform_messages(&msgs)),
            vec!["user:again", "assistant", "result:a:new output", "developer"]
        );
    }

    #[test]
    fn reused_id_keeps_each_result_with_its_call() {
        let msgs = vec![
            assistant(&["a"], StopReason::ToolUse),
            result_text("a", "first"),
            assistant(&["a"], StopReason::ToolUse),
            developer("guidance"),
            result_text("a", "second"),
        ];
        assert_eq!(
            texts(&transform_messages(&msgs)),
            vec!["assistant", "result:a:first", "assistant", "result:a_dup1:second", "developer"]
        );
        let msgs = vec![
            assistant(&["a"], StopReason::ToolUse),
            developer("first boundary"),
            assistant(&["a"], StopReason::ToolUse),
            result_text("a", "second only"),
        ];
        assert_eq!(
            texts(&transform_messages(&msgs)),
            vec!["assistant", "result:a:No result provided", "developer", "assistant", "result:a_dup1:second only"]
        );
    }

    #[test]
    fn delayed_parallel_result_keeps_real_payload_once() {
        let msgs = vec![
            assistant(&["a", "b"], StopReason::ToolUse),
            result_text("b", "immediate"),
            Message::User(UserMessage::text("next")),
            result_text("a", "late"),
            result_text("a", "duplicate"),
        ];
        assert_eq!(
            texts(&transform_messages(&msgs)),
            vec!["assistant", "result:b:immediate", "result:a:late", "user:next"]
        );
    }

    #[test]
    fn same_turn_duplicate_ids_avoid_existing_suffix_and_preserve_input() {
        let long_id = format!("toolu_{}", "a".repeat(58));
        let msgs = vec![
            assistant(&["a", "a_dup1", "a"], StopReason::ToolUse),
            result_text("a", "first"),
            result_text("a_dup1", "reserved"),
            result_text("a", "third"),
            assistant(&[&long_id], StopReason::ToolUse),
            result_text(&long_id, "long first"),
            assistant(&[&long_id], StopReason::ToolUse),
            result_text(&long_id, "long second"),
        ];
        let original = msgs.clone();
        let output = transform_messages(&msgs);
        assert_eq!(msgs, original, "provider conversion must not alter retained history");
        assert_eq!(
            texts(&output),
            vec![
                "assistant".into(),
                "result:a:first".into(),
                "result:a_dup1:reserved".into(),
                "result:a_dup2:third".into(),
                "assistant".into(),
                format!("result:{long_id}:long first"),
                "assistant".into(),
                format!("result:toolu_{}_dup1:long second", "a".repeat(53)),
            ]
        );
        for assistant in output.iter().filter_map(Message::as_assistant) {
            for call in assistant.tool_calls() {
                assert!(call.id.len() <= 64, "{}", call.id);
            }
        }
    }

    #[test]
    fn moved_result_keeps_error_and_original_timestamp() {
        let mut late = result_text("a", "failure details");
        if let Message::ToolResult(result) = &mut late {
            result.is_error = true;
            result.timestamp = 42;
        }
        let output = transform_messages(&[
            assistant(&["a"], StopReason::ToolUse),
            Message::User(UserMessage::text("later")),
            late,
        ]);
        let Message::ToolResult(result) = &output[1] else { panic!("expected real result immediately after call") };
        assert_eq!(result.content, vec![UserBlock::text("failure details")]);
        assert!(result.is_error);
        assert_eq!(result.timestamp, 42);
    }
}
