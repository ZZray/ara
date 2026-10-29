//! Host-owned, read-only restoration for legacy text messages and soft summaries.
//!
//! This deliberately does not implement the CLI's Graph/native converter. The
//! journal remains the source authority; provider fragments are derived views.

use ara_agent::AgentMessage;
use ara_ai::{AssistantBlock, Message, Model, UserBlock, UserContent};
use ara_session::{
    AgentEntrySource, AgentMessageBody, CompactedContextItem, Entry, NativeMessageSource, RawAgentContextMessage,
    SessionJournal,
};
use serde_json::Value;
use std::{error::Error, io, sync::Arc};

type BridgeResult<T> = Result<T, Box<dyn Error>>;

fn rejected(message: &str) -> Box<dyn Error> {
    io::Error::new(io::ErrorKind::InvalidData, message).into()
}

fn text_blocks(content: &UserContent) -> bool {
    match content {
        UserContent::Text(_) => true,
        UserContent::Blocks(blocks) => blocks.iter().all(|block| matches!(block, UserBlock::Text(_))),
    }
}

fn check_message(message: &Message) -> BridgeResult<()> {
    let text_only = match message {
        Message::User(user) => user.provider_payload.is_none() && text_blocks(&user.content),
        Message::Developer(developer) => developer.provider_payload.is_none() && text_blocks(&developer.content),
        Message::Assistant(assistant) => {
            assistant.provider_payload.is_none()
                && assistant.content.iter().all(|block| !matches!(block, AssistantBlock::Image(_)))
        }
        Message::ToolResult(result) => {
            if let Some(details) = &result.details
                && (details.get("executed").and_then(Value::as_str) == Some("unknown")
                    || details.get("panicked").and_then(Value::as_bool) == Some(true)
                    || (details.get("__synthetic").and_then(Value::as_bool) == Some(true)
                        && details.get("source").and_then(Value::as_str) == Some("interrupted_unknown_effect")))
            {
                return Err(rejected("Session contains a tool result with unknown effects"));
            }
            result.content.iter().all(|block| matches!(block, UserBlock::Text(_)))
        }
    };
    if !text_only {
        return Err(rejected("this bridge does not support images or provider-native payloads"));
    }
    Ok(())
}

fn check_soft_entry(entry: &Entry) -> BridgeResult<()> {
    let raw = &entry.raw;
    if raw.get("method").filter(|value| !value.is_null()).and_then(Value::as_str).unwrap_or("soft") != "soft"
        || ["preserveData", "providerPayload", "providerReplayThroughEntryId", "shortSummary", "blocks", "images"]
            .iter()
            .any(|field| raw.get(*field).is_some_and(|value| !value.is_null()))
        || raw.get("fromExtension").and_then(Value::as_bool) == Some(true)
    {
        return Err(rejected("this bridge supports only plain soft compaction"));
    }
    if !raw.get("sourceEntryIds").and_then(Value::as_array).is_some_and(|ids| !ids.is_empty()) {
        return Err(rejected("soft summary has no verifiable source entry IDs"));
    }
    Ok(())
}

fn restore_record(
    journal: &SessionJournal,
    model: &Model,
    record: RawAgentContextMessage,
) -> BridgeResult<Vec<AgentMessage>> {
    // Reopen represents ordinary durable JSON as Persisted Wire owners too.
    // Use the existing strict, lossless observation; never lower live Graph
    // owners or silently reconstruct unsupported data through serialization.
    if matches!(&record.body, AgentMessageBody::Native(NativeMessageSource::Live { .. }))
        || matches!(
            &record.source,
            AgentEntrySource::Native { body: NativeMessageSource::Live { .. }, .. }
                | AgentEntrySource::Snapcompact { .. }
        )
    {
        return Err(rejected("live Graph or archive owners require their own Host converter"));
    }
    let body = record.body.checked_legacy()?;
    let source = record.source.checked_legacy()?;
    let source_id = source.get("id").and_then(Value::as_str).ok_or_else(|| rejected("source has no entry ID"))?;
    let entry = journal
        .entries()
        .iter()
        .find(|entry| entry.id == source_id)
        .ok_or_else(|| rejected("source entry is outside the Session"))?;
    let original = entry.original_source().checked_legacy()?;
    if source != original {
        return Err(rejected("record source differs from its original Session entry"));
    }
    let fragments = if body.get("role").and_then(Value::as_str) == Some("compactionSummary") {
        if entry.kind != "compaction" {
            return Err(rejected("summary source is not a compaction entry"));
        }
        let checked = journal
            .checked_agent_message_projection_for_route(&body, source_id, model, false, false)?
            .ok_or_else(|| rejected("summary body has no current Session source grant"))?;
        let cached = record.checked_projection.as_ref().ok_or_else(|| rejected("summary has no checked projection"))?;
        if *cached != checked || checked.len() != 1 || !matches!(&checked[0], Message::User(_)) {
            return Err(rejected("summary projection differs from its checked lower-trust User view"));
        }
        checked
    } else {
        if record.checked_projection.is_some() {
            return Err(rejected("ordinary message has an unexpected summary projection"));
        }
        let expected = entry
            .agent_message_body()?
            .ok_or_else(|| rejected("ordinary source has no supported body"))?
            .checked_legacy()?;
        if body != expected {
            return Err(rejected("ordinary message body differs from its Session source"));
        }
        Entry::project_agent_message_body(&body)?
    };
    let source = Arc::new(original);
    fragments
        .into_iter()
        .map(|message| {
            check_message(&message)?;
            Ok(AgentMessage::from_model_with_source(message, Some(source.clone())))
        })
        .collect()
}

/// Restore a checked text/soft-compaction view without editing the journal or Agent.
///
/// `settled` is the Host's admission declaration after checking its own durable
/// effect receipts. It is not proof that all external effects have settled. A
/// false declaration is refused, and explicit unknown-effect Session receipts
/// are refused even when the declaration is true. The caller must retain an
/// exclusive Session lease through restoration and the subsequent Run.
pub fn restore(journal: &SessionJournal, model: &Model, settled: bool) -> BridgeResult<Vec<AgentMessage>> {
    if !settled {
        return Err(rejected("Host has not established settlement from its effect receipts"));
    }
    // Do not inherit raw_agent_context's tolerant fallback for malformed history.
    let projection = journal.compacted_context_projection_for_route(model, false)?;
    for item in &projection.items {
        match item {
            CompactedContextItem::Summary(summary) => {
                if summary.method != "soft"
                    || !summary.source_entry_ids.as_ref().is_some_and(|ids| !ids.is_empty())
                    || summary.archive.is_some()
                    || summary.archive_image_boundary.is_some()
                    || summary.preserve_source.is_some()
                    || summary.preserve_data.as_ref().is_some_and(|value| !value.is_null())
                    || summary.remote.is_some()
                {
                    return Err(rejected("unsupported or unverifiable summary"));
                }
            }
            CompactedContextItem::Message(message) => check_message(&message.message)?,
        }
    }
    // Strict projection above validated the parent chain before branch() is used.
    // Inspect the active original records too, including summarized sources.
    let branch = journal.branch();
    let start = branch.iter().rposition(|entry| entry.kind == "reset_boundary").map_or(0, |index| index + 1);
    for entry in &branch[start..] {
        if matches!(&entry.native_message, Some(NativeMessageSource::Live { .. })) || entry.snapcompact_source.is_some()
        {
            return Err(rejected("live Graph or archive records require their own Host converter"));
        }
        if entry.kind == "compaction" {
            check_soft_entry(entry)?;
        }
        if let Some(messages) = entry.model_messages()? {
            for message in &messages {
                check_message(message)?;
            }
        }
    }
    let mut messages = Vec::new();
    for record in journal.raw_agent_context_for_route_with_images(model, false, false)? {
        messages.extend(restore_record(journal, model, record)?);
    }
    Ok(messages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_ai::{AssistantMessage, JsonObject, StopReason, ToolCall, ToolResultMessage, UserMessage};
    use serde_json::json;
    use std::fs;

    fn model() -> Model {
        Model {
            id: "fixture".into(),
            api: "openai-completions".into(),
            provider: "fixture".into(),
            base_url: "http://fixture.invalid/v1".into(),
            reasoning: false,
            max_tokens: None,
            context_window: None,
            tokenizer: None,
        }
    }

    fn turn(journal: &mut SessionJournal, label: &str) -> [String; 2] {
        let mut assistant = AssistantMessage::empty("openai-completions", "fixture", "fixture");
        assistant.content.push(AssistantBlock::text(label));
        [
            journal.append_message(&Message::User(UserMessage::text(label))).unwrap(),
            journal.append_message(&Message::Assistant(assistant)).unwrap(),
        ]
    }

    fn compacted() -> (tempfile::TempDir, SessionJournal, String, Vec<String>) {
        let directory = tempfile::tempdir().unwrap();
        let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
        let replaced = turn(&mut journal, "User correction: output must be UTF-8; source README:3.");
        let kept = turn(&mut journal, "Unfinished: implement decoder and run tests.");
        let snapshot = journal.projected_compaction_snapshot().unwrap();
        let summary = journal
            .commit_projected_compaction(
                &snapshot,
                "Correction: UTF-8 (README:3). Pending: decoder and tests.",
                &kept[0],
                &replaced,
                100,
            )
            .unwrap();
        (directory, journal, summary, replaced.into_iter().chain(kept).collect())
    }

    fn rewrite_entry(journal: &SessionJournal, id: &str, edit: impl Fn(&mut Value)) {
        let raw = fs::read_to_string(journal.path()).unwrap();
        let mut changed = false;
        let lines = raw
            .lines()
            .map(|line| {
                let mut value: Value = serde_json::from_str(line).unwrap();
                if value.get("id").and_then(Value::as_str) == Some(id) {
                    edit(&mut value);
                    changed = true;
                }
                serde_json::to_string(&value).unwrap()
            })
            .collect::<Vec<_>>();
        assert!(changed);
        fs::write(journal.path(), format!("{}\n", lines.join("\n"))).unwrap();
    }

    #[test]
    fn compaction_reopen_retains_original_sources_and_lower_trust_summary() {
        let (_directory, journal, summary_id, original_ids) = compacted();
        let original_entries = journal.entries().to_vec();
        let bytes = fs::read(journal.path()).unwrap();
        let reopened = SessionJournal::open(journal.path()).unwrap();
        let messages = restore(&reopened, &model(), true).unwrap();
        assert_eq!(reopened.session_id(), journal.session_id());
        assert_eq!(reopened.entries(), original_entries);
        assert_eq!(messages.len(), 3);
        let Some(Message::User(summary)) = messages[0].checked_model_snapshot().unwrap() else {
            panic!("summary must remain a lower-trust User projection");
        };
        assert!(summary.content.plain_text().contains("Correction: UTF-8"));
        let source = messages[0].source().unwrap();
        assert_eq!(source["id"], summary_id);
        assert_eq!(source["type"], "compaction");
        assert_eq!(source["sourceEntryIds"], json!(&original_ids[..2]));
        for (message, id) in messages[1..].iter().zip(&original_ids[2..]) {
            let entry = reopened.entries().iter().find(|entry| &entry.id == id).unwrap();
            assert_eq!(message.source().as_deref(), Some(&entry.raw));
        }
        assert_eq!(fs::read(journal.path()).unwrap(), bytes);
    }

    #[test]
    fn malformed_parent_duplicate_id_and_bad_summary_sources_fail_without_writes() {
        for fault in ["missing-parent", "duplicate-id", "bad-source-id", "missing-source-ids"] {
            let (_directory, journal, summary_id, original_ids) = compacted();
            match fault {
                "missing-parent" => rewrite_entry(&journal, &summary_id, |value| value["parentId"] = json!("absent")),
                "duplicate-id" => rewrite_entry(&journal, &summary_id, |value| value["id"] = json!(&original_ids[0])),
                "bad-source-id" => {
                    rewrite_entry(&journal, &summary_id, |value| value["sourceEntryIds"] = json!(["absent"]))
                }
                "missing-source-ids" => rewrite_entry(&journal, &summary_id, |value| {
                    value.as_object_mut().unwrap().remove("sourceEntryIds");
                }),
                _ => unreachable!(),
            }
            let before = fs::read(journal.path()).unwrap();
            let reopened = SessionJournal::open(journal.path()).unwrap();
            assert!(restore(&reopened, &model(), true).is_err(), "{fault}");
            assert_eq!(fs::read(journal.path()).unwrap(), before, "{fault}");
        }
    }

    #[test]
    fn paired_unknown_receipt_and_unsettled_host_are_refused() {
        for details in [
            json!({"__synthetic":true,"source":"interrupted_unknown_effect","executed":"unknown"}),
            json!({"executed":"unknown"}),
            json!({"__synthetic":true,"source":"interrupted_unknown_effect"}),
            json!({"panicked":true}),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
            journal.append_message(&Message::User(UserMessage::text("run tool"))).unwrap();
            let mut assistant = AssistantMessage::empty("openai-completions", "fixture", "fixture");
            assistant.content.push(AssistantBlock::ToolCall(ToolCall {
                id: "call-1".into(),
                name: "test".into(),
                arguments: JsonObject::new(),
                ..Default::default()
            }));
            assistant.stop_reason = StopReason::ToolUse;
            journal.append_message(&Message::Assistant(assistant)).unwrap();
            journal
                .append_message(&Message::ToolResult(ToolResultMessage {
                    tool_call_id: "call-1".into(),
                    tool_name: "test".into(),
                    content: vec![UserBlock::text("unknown")],
                    details: Some(details),
                    is_error: true,
                    timestamp: 1,
                    provider_metadata: None,
                    useless: false,
                }))
                .unwrap();
            let before = fs::read(journal.path()).unwrap();
            let reopened = SessionJournal::open(journal.path()).unwrap();
            assert!(restore(&reopened, &model(), true).is_err());
            assert_eq!(fs::read(journal.path()).unwrap(), before);
        }
        let (_directory, journal, _, _) = compacted();
        let before = fs::read(journal.path()).unwrap();
        assert!(restore(&journal, &model(), false).is_err());
        assert_eq!(fs::read(journal.path()).unwrap(), before);
    }

    #[test]
    fn tampered_summary_body_source_or_projection_has_no_grant() {
        let (_directory, journal, _, original_ids) = compacted();
        let records = journal.raw_agent_context_for_route_with_images(&model(), false, false).unwrap();
        let summary = records[0].clone();
        let before = fs::read(journal.path()).unwrap();
        let mut changed_body = summary.clone();
        let AgentMessageBody::Legacy(body) = &mut changed_body.body else { panic!("legacy fixture") };
        body["summary"] = json!("forged summary");
        let source_id = match &changed_body.source {
            AgentEntrySource::Legacy(source) => source["id"].as_str().unwrap(),
            _ => panic!("legacy fixture"),
        };
        assert!(
            journal
                .checked_agent_message_projection_for_route(body, source_id, &model(), false, false)
                .unwrap()
                .is_none()
        );
        assert!(restore_record(&journal, &model(), changed_body).is_err());
        let mut changed_source = summary.clone();
        changed_source.source =
            journal.entries().iter().find(|entry| entry.id == original_ids[2]).unwrap().original_source();
        assert!(restore_record(&journal, &model(), changed_source).is_err());
        let mut changed_projection = summary;
        changed_projection.checked_projection = Some(vec![Message::User(UserMessage::text("forged projection"))]);
        assert!(restore_record(&journal, &model(), changed_projection).is_err());
        assert_eq!(fs::read(journal.path()).unwrap(), before);
    }
}
