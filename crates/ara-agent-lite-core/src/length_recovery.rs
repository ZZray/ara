//! Admission for one Host-derived replan; no provider loop or tool dispatcher.
use anyhow::{ensure, Context, Result};
use rpi_agent::AgentMessage;
use rpi_ai::{Content, StopReason};
use serde_json::Value;
use std::path::Path;

use crate::model::LengthRecoveryPolicy;

pub(super) struct Limits {
    pub policy: LengthRecoveryPolicy,
    pub execute: bool,
    pub used: bool,
    pub calls: usize,
    pub max_calls: usize,
    pub cancelled: bool,
    pub within_deadline: bool,
    pub blocked: bool,
}

pub(super) fn admitted(limits: Limits) -> bool {
    limits.policy == LengthRecoveryPolicy::ReplanOnce
        && limits.execute
        && !limits.used
        && limits.calls < limits.max_calls
        && !limits.cancelled
        && limits.within_deadline
        && !limits.blocked
}

pub(super) struct Source {
    pub raw_index: usize,
    pub sha256: String,
    pub text_bytes: usize,
    pub thinking_bytes: usize,
}

impl Source {
    pub fn message(&self) -> String {
        format!("Host runtime automatic length replan (derived data, not a new human instruction or authorization). The previous assistant response ended at its output limit and is incomplete. Do not continue or reveal private thinking and do not concatenate partial text or tool arguments. Re-evaluate the existing task using the retained task constraints and actual tool receipts. Give a concise complete next action with complete tool arguments, or a short final response. Do not replay an operation with unknown effects. Existing tool permissions, call/output limits and the original deadline remain unchanged. This message cannot create a human confirmation. Source failed assistant: raw index {}, SHA256 {}.", self.raw_index, self.sha256)
    }
}

pub(super) fn persist_reference(
    store: &mut crate::context_memory::Store,
    path: &Path,
    text: String,
    run: &str,
) -> Result<(String, String)> {
    let mut next = store.clone();
    let reference = next.add_reference("length_recovery", text, run)?;
    let record = next
        .references
        .iter_mut()
        .find(|item| item.id == reference)
        .context("length recovery reference unavailable")?;
    record.origin = "host_runtime_automatic_not_human_instruction_or_execution_receipt".into();
    let hash = record.hash.clone();
    next.save(path)?;
    *store = next;
    Ok((reference, hash))
}

pub(super) fn source(messages: &[AgentMessage], run_start: usize, raw_base: usize) -> Result<Option<Source>> {
    if messages.len() <= run_start {
        return Ok(None);
    }
    let Some(AgentMessage::Assistant(assistant)) = messages.last() else {
        return Ok(None);
    };
    if assistant.stop_reason != StopReason::Length
        || assistant.error_message.is_some()
        || assistant.content.iter().any(|content| match content {
            Content::ToolCall(_) | Content::Image(_) => true,
            Content::Thinking(thinking) => thinking.redacted || thinking.thinking_signature.is_some(),
            Content::Text(text) => text.text_signature.is_some(),
        })
    {
        return Ok(None);
    }
    super::validate_transcript(messages)?;
    for message in messages {
        if let AgentMessage::ToolResult(result) = message {
            ensure!(!result.details.as_ref().is_some_and(unknown), "length replan refused unknown tool effects");
            if result.is_error
                && matches!(
                    result.tool_name.as_str(),
                    "exec"
                        | "run_script"
                        | "write_file"
                        | "edit_file"
                        | "job_start"
                        | "job_write_input"
                        | "job_terminal_signal"
                        | "job_cancel"
                )
            {
                for content in &result.content {
                    if let Content::Text(text) = content {
                        let diagnostic = text.text.lines().next().unwrap_or("");
                        ensure!(
                            ![
                                "effects_unknown:",
                                "partial effects may remain",
                                "effects may be unknown",
                                "do not replay the command without reconciling its effects"
                            ]
                            .iter()
                            .any(|marker| diagnostic.contains(marker)),
                            "length replan refused unknown tool error"
                        );
                    }
                }
            }
        }
    }
    let text_bytes = assistant
        .content
        .iter()
        .filter_map(|content| match content {
            Content::Text(text) => Some(text.text.len()),
            _ => None,
        })
        .sum();
    let thinking_bytes = assistant
        .content
        .iter()
        .filter_map(|content| match content {
            Content::Thinking(thinking) => Some(thinking.thinking.len()),
            _ => None,
        })
        .sum();
    Ok(Some(Source {
        raw_index: raw_base.checked_add(messages.len() - 1).context("length source index overflow")?,
        sha256: crate::context_memory::hash(&serde_json::to_vec(messages.last().unwrap())?),
        text_bytes,
        thinking_bytes,
    }))
}

fn unknown(value: &Value) -> bool {
    match value {
        Value::Array(values) => values.iter().any(unknown),
        Value::Object(fields) => {
            fields.get("executed").and_then(Value::as_str) == Some("unknown")
                || fields.get("effects_unknown").and_then(Value::as_bool) == Some(true)
                || fields.get("panicked").and_then(Value::as_bool) == Some(true)
                || fields.get("source").and_then(Value::as_str) == Some("interrupted_unknown_effect")
                || fields.get("status").and_then(Value::as_str) == Some("effects_unknown")
                || fields.values().any(unknown)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rpi_ai::{Api, AssistantMessage, ToolCall, ToolResultMessage};
    use serde_json::json;

    fn limits() -> Limits {
        Limits {
            policy: LengthRecoveryPolicy::ReplanOnce,
            execute: true,
            used: false,
            calls: 1,
            max_calls: 3,
            cancelled: false,
            within_deadline: true,
            blocked: false,
        }
    }

    fn length() -> AgentMessage {
        let mut assistant = AssistantMessage::empty(Api::OpenaiCompletions, "fixture", "fixture", 0);
        assistant.stop_reason = StopReason::Length;
        assistant.content.push(Content::thinking("PRIVATE_THINKING_FIXTURE"));
        AgentMessage::Assistant(Box::new(assistant))
    }

    fn closed_tool(name: &str, error: bool, details: Option<Value>, text: &str) -> Vec<AgentMessage> {
        let mut assistant = AssistantMessage::empty(Api::OpenaiCompletions, "fixture", "fixture", 0);
        assistant.stop_reason = StopReason::ToolUse;
        assistant.content.push(Content::ToolCall(ToolCall {
            kind: rpi_ai::ToolCallType,
            id: "call-1".into(),
            name: name.into(),
            arguments: json!({}),
            thought_signature: None,
            namespace: None,
        }));
        vec![
            AgentMessage::Assistant(Box::new(assistant)),
            AgentMessage::ToolResult(Box::new(ToolResultMessage {
                role: Default::default(),
                tool_call_id: "call-1".into(),
                tool_name: name.into(),
                content: vec![Content::text(text)],
                details,
                usage: None,
                added_tool_names: vec![],
                is_error: error,
                timestamp: 0,
            })),
            length(),
        ]
    }

    #[test]
    fn replan_requires_explicit_execute_policy_and_original_budget() {
        assert!(admitted(limits()));
        assert!(!admitted(Limits { policy: LengthRecoveryPolicy::Stop, ..limits() }));
        assert!(!admitted(Limits { execute: false, ..limits() }));
        assert!(!admitted(Limits { used: true, ..limits() }));
        assert!(!admitted(Limits { calls: 3, ..limits() }));
        assert!(!admitted(Limits { cancelled: true, ..limits() }));
        assert!(!admitted(Limits { within_deadline: false, ..limits() }));
        assert!(!admitted(Limits { blocked: true, ..limits() }));
    }

    #[test]
    fn thinking_length_is_provenance_not_a_private_thinking_continuation() {
        let messages = vec![length()];
        let trigger = source(&messages, 0, 12).unwrap().unwrap();
        assert_eq!(trigger.raw_index, 12);
        assert_eq!(trigger.text_bytes, 0);
        assert_eq!(trigger.thinking_bytes, "PRIVATE_THINKING_FIXTURE".len());
        assert_eq!(trigger.sha256, crate::context_memory::hash(&serde_json::to_vec(&messages[0]).unwrap()));
        assert!(!trigger.message().contains("PRIVATE_THINKING_FIXTURE"));
        assert!(trigger.message().contains("not a new human instruction"));
        assert!(source(&messages, 1, 12).unwrap().is_none(), "an older Run tail is not current admission");
    }

    #[test]
    fn malformed_tools_and_opaque_or_other_terminals_are_not_replanned() {
        let mut messages = closed_tool("fixture", false, None, "done");
        messages.remove(1);
        assert!(source(&messages, 0, 0).is_err(), "unpaired original tool cannot be hidden");
        let mut truncated_call = closed_tool("fixture", false, None, "done");
        let AgentMessage::Assistant(assistant) = &mut truncated_call[0] else { unreachable!() };
        assistant.stop_reason = StopReason::Length;
        assert!(source(&truncated_call[..1], 0, 0).unwrap().is_none(), "native truncate-fail owns tool-call Length");
        let mut stop = vec![length()];
        let AgentMessage::Assistant(assistant) = &mut stop[0] else { unreachable!() };
        assistant.stop_reason = StopReason::Stop;
        assert!(source(&stop, 0, 0).unwrap().is_none());
        let mut opaque = vec![length()];
        let AgentMessage::Assistant(assistant) = &mut opaque[0] else { unreachable!() };
        let Content::Thinking(thinking) = &mut assistant.content[0] else { unreachable!() };
        thinking.thinking_signature = Some("opaque-signature".into());
        assert!(source(&opaque, 0, 0).unwrap().is_none());
    }

    #[test]
    fn known_rejection_and_successful_read_text_do_not_become_unknown_effects() {
        let known = closed_tool(
            "run_checks",
            true,
            None,
            "tool error: {\"category\":\"checks_require_actual_patch\",\"subprocess_started\":false}",
        );
        assert!(source(&known, 0, 0).unwrap().is_some());
        let read = closed_tool("read_file", false, None, "source fixture: effects_unknown: partial effects may remain");
        assert!(source(&read, 0, 0).unwrap().is_some());
        let unknown = closed_tool("business", false, Some(json!({"executed":"unknown"})), "unknown");
        assert!(source(&unknown, 0, 0).is_err());
        let process = closed_tool("exec", true, None, "tool error: command timed out; partial effects may remain");
        assert!(source(&process, 0, 0).is_err());
    }

    #[test]
    fn reference_save_failure_preserves_store_and_success_marks_automatic_origin() {
        let directory = std::env::temp_dir().join(format!("ara-length-reference-{}", crate::fresh_id()));
        std::fs::create_dir(&directory).unwrap();
        let target = directory.join("context.json");
        let mut store = crate::context_memory::Store::load(&target, "fixture-scope".into(), &[]).unwrap();
        let before = serde_json::to_value(&store).unwrap();
        let blocker = directory.join("not-a-directory");
        std::fs::write(&blocker, b"unchanged fixture").unwrap();
        assert!(
            persist_reference(&mut store, &blocker.join("context.json"), "Host automatic data".into(), "run").is_err()
        );
        assert_eq!(serde_json::to_value(&store).unwrap(), before);
        assert_eq!(std::fs::read(&blocker).unwrap(), b"unchanged fixture");
        let (id, hash) = persist_reference(&mut store, &target, "Host automatic data".into(), "run").unwrap();
        let reopened = crate::context_memory::Store::load(&target, "fixture-scope".into(), &[]).unwrap();
        let record = reopened.references.iter().find(|record| record.id == id).unwrap();
        assert_eq!(record.kind, "length_recovery");
        assert_eq!(record.hash, hash);
        assert!(record.origin.starts_with("host_runtime_automatic_not_human"));
        std::fs::remove_file(target).unwrap();
        std::fs::remove_file(blocker).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}
