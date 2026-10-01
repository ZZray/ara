use ara_ai::{
    AssistantBlock, AssistantMessage, AssistantRetryRecovery, JsonObject, Message, StopReason, ThinkingContent,
    ToolCall, ToolResultMessage, UserMessage,
};
use ara_session::{
    BashExecutionMessage, DISCARDED_ENTRY_BRANCH_MARKER, FailedAssistantRecoveryOutcome, SessionJournal,
};
use serde_json::{Value, json};

fn recovery(status: &str) -> AssistantRetryRecovery {
    AssistantRetryRecovery {
        kind: "auto-retry".into(),
        status: status.into(),
        attempt: 1,
        recovery: "plain".into(),
        note: "error; retried".into(),
        recovered_at: None,
        superseded_by: None,
    }
}

fn failed() -> Message {
    let mut message = AssistantMessage::empty("openai-completions", "fixture", "model");
    message.stop_reason = StopReason::Error;
    message.error_status = Some(503);
    message.error_message = Some("503 unavailable".into());
    Message::Assistant(message)
}

#[test]
fn exact_failed_entry_is_rewritten_and_survives_restart_without_losing_receipt() {
    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    journal.append_message(&Message::User(UserMessage::text("original request"))).unwrap();
    let original = failed();
    let id = journal.append_message(&original).unwrap();
    let leaf = journal.leaf_id().unwrap().to_owned();
    let mut successful = AssistantMessage::empty("openai-completions", "fixture", "model");
    successful.content.push(ara_ai::AssistantBlock::text("recovered"));
    journal.append_message(&Message::Assistant(successful)).unwrap();
    let final_leaf = journal.leaf_id().unwrap().to_owned();
    journal.update_retry_recovery(&[(id.clone(), recovery("recovered"))]).unwrap();
    assert_eq!(journal.leaf_id(), Some(final_leaf.as_str()));
    let reopened = SessionJournal::open(journal.path()).unwrap();
    let mut updated = reopened.entries().iter().find(|entry| entry.id == id).unwrap().message().unwrap();
    let Message::Assistant(message) = &mut updated else { panic!("assistant receipt") };
    assert_eq!(message.retry_recovery.take(), Some(recovery("recovered")));
    assert_eq!(updated, original);
    assert_eq!(leaf, id);
}

#[test]
fn invalid_or_duplicate_target_does_not_partially_mutate_valid_receipt() {
    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    let user = journal.append_message(&Message::User(UserMessage::text("request"))).unwrap();
    let id = journal.append_message(&failed()).unwrap();
    let before = std::fs::read(journal.path()).unwrap();
    for updates in [
        vec![(id.clone(), recovery("recovered")), (user, recovery("superseded"))],
        vec![(id.clone(), recovery("recovered")), (id.clone(), recovery("superseded"))],
        vec![("missing".into(), recovery("recovered"))],
    ] {
        assert!(journal.update_retry_recovery(&updates).is_err());
        assert_eq!(std::fs::read(journal.path()).unwrap(), before);
        assert!(
            journal
                .entries()
                .iter()
                .find(|entry| entry.id == id)
                .unwrap()
                .message()
                .unwrap()
                .as_assistant()
                .unwrap()
                .retry_recovery
                .is_none()
        );
    }
}

#[test]
fn failed_atomic_rewrite_restores_memory_and_preserves_original_raw_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    let id = journal.append_message(&failed()).unwrap();
    let original = std::fs::read(journal.path()).unwrap();
    std::fs::remove_file(journal.path()).unwrap();
    std::fs::create_dir(journal.path()).unwrap();
    assert!(journal.update_retry_recovery(&[(id, recovery("recovered"))]).is_err());
    assert!(journal.entries()[0].message().unwrap().as_assistant().unwrap().retry_recovery.is_none());
    std::fs::remove_dir(journal.path()).unwrap();
    std::fs::write(journal.path(), &original).unwrap();
    let reopened = SessionJournal::open(journal.path()).unwrap();
    assert!(reopened.entries()[0].message().unwrap().as_assistant().unwrap().retry_recovery.is_none());
}

#[test]
fn failed_assistant_recovery_transaction_preserves_native_ownership_durability_and_effect_receipts() {
    fn assert_reopened_context(journal: &SessionJournal, reopened: &SessionJournal) {
        assert_eq!(reopened.entries(), journal.entries());
        let projection = journal.compacted_context_projection().unwrap();
        assert_eq!(reopened.compacted_context_projection().unwrap(), projection);
        assert!(matches!(projection.items.first(), Some(ara_session::CompactedContextItem::Summary(_))));
        let expected = journal.model_context();
        let mut actual = reopened.model_context();
        // Only the first synthetic summary projection gets a fresh timestamp.
        // Source IDs, raw receipts and all real message timestamps stay exact.
        let (Some(Message::User(projected)), Some(Message::User(original))) = (actual.first_mut(), expected.first())
        else {
            panic!("the native summary must project to the first model message")
        };
        projected.timestamp = original.timestamp;
        assert_eq!(actual, expected);
    }

    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    let first_user = journal.append_message(&Message::User(UserMessage::text("completed request"))).unwrap();
    let mut completed = AssistantMessage::empty("openai-completions", "fixture", "model");
    completed.content.push(AssistantBlock::text("completed answer"));
    let first_answer = journal.append_message(&Message::Assistant(completed)).unwrap();
    let kept = journal.append_message(&Message::User(UserMessage::text("recover this request"))).unwrap();
    let mut partial = failed().as_assistant().unwrap().clone();
    partial.content.push(AssistantBlock::text("visible failed output"));
    let original = journal.append_message(&Message::Assistant(partial.clone())).unwrap();
    let raw_original = journal.entries().last().unwrap().clone();
    let bytes = std::fs::read(journal.path()).unwrap();
    assert!(journal.begin_failed_assistant_recovery(&kept, &original, &partial).is_err());
    let mut different = partial.clone();
    different.error_message = Some("different failure".into());
    assert!(journal.begin_failed_assistant_recovery(&original, &original, &different).is_err());
    assert_eq!(std::fs::read(journal.path()).unwrap(), bytes);

    // A temporary parent selection does not alter the native receipt or reload.
    let token = journal.begin_failed_assistant_recovery(&original, &original, &partial).unwrap();
    assert_eq!(journal.leaf_id(), Some(kept.as_str()));
    assert_eq!(journal.entries().iter().find(|entry| entry.id == original), Some(&raw_original));
    assert_eq!(std::fs::read(journal.path()).unwrap(), bytes);
    assert_eq!(SessionJournal::open(journal.path()).unwrap().leaf_id(), Some(original.as_str()));
    let FailedAssistantRecoveryOutcome::Restored { message, appended_entry_id: Some(restored) } =
        journal.finish_failed_assistant_recovery(token, false).unwrap()
    else {
        panic!("contentful rollback")
    };
    assert_eq!(*message, partial);
    assert_ne!(restored, original);
    assert_eq!(journal.entries().iter().find(|entry| entry.id == original), Some(&raw_original));
    assert_eq!(SessionJournal::open(journal.path()).unwrap().leaf_id(), Some(restored.as_str()));

    // Only a fresh valid native compaction commits the selected recovery branch.
    let token = journal.begin_failed_assistant_recovery(&restored, &restored, &partial).unwrap();
    let snapshot = journal.projected_compaction_snapshot().unwrap();
    let summary = journal
        .commit_projected_compaction(&snapshot, "completed first request", &kept, &[first_user, first_answer], 100)
        .unwrap();
    assert_eq!(
        journal.finish_failed_assistant_recovery(token, false).unwrap(),
        FailedAssistantRecoveryOutcome::Committed
    );
    assert_eq!(journal.leaf_id(), Some(summary.as_str()));
    let reopened = SessionJournal::open(journal.path()).unwrap();
    assert_reopened_context(&journal, &reopened);
    assert!(!reopened.branch().iter().any(|entry| entry.id == restored || entry.id == original));
    assert_eq!(reopened.entries().iter().find(|entry| entry.id == original), Some(&raw_original));

    // Empty error rollback only restores Agent context; signed thinking is output.
    for signature in [None, Some("signed-reasoning")] {
        let mut message = failed().as_assistant().unwrap().clone();
        message.content = vec![AssistantBlock::Thinking(ThinkingContent {
            thinking: "\u{FEFF}".into(),
            thinking_signature: signature.map(str::to_owned),
        })];
        let failed_id = journal.append_message(&Message::Assistant(message.clone())).unwrap();
        let token = journal.begin_failed_assistant_recovery(&failed_id, &failed_id, &message).unwrap();
        let FailedAssistantRecoveryOutcome::Restored { message: restored_message, appended_entry_id } =
            journal.finish_failed_assistant_recovery(token, false).unwrap()
        else {
            panic!("no fresh recovery")
        };
        assert_eq!(*restored_message, message);
        assert_eq!(appended_entry_id.is_some(), signature.is_some());
        let reopened = SessionJournal::open(journal.path()).unwrap();
        assert_eq!(reopened.build_context().last(), Some(&Message::Assistant(message.clone())));
        assert_eq!(reopened.model_context().last() == Some(&Message::Assistant(message)), signature.is_some());
    }

    // Imported sibling metadata children are chained in physical order.
    let discarded = journal.append_message(&Message::Assistant(partial.clone())).unwrap();
    let tier_one = journal.append_service_tier_change(&json!({"openai":"priority"})).unwrap();
    let tier_two = journal.append_service_tier_change(&Value::Null).unwrap();
    let path = journal.path().to_path_buf();
    let rewritten = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|line| {
            let mut raw: Value = serde_json::from_str(line).unwrap();
            if raw["id"] == tier_two {
                raw["parentId"] = json!(discarded);
            }
            serde_json::to_string(&raw).unwrap()
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(&path, rewritten).unwrap();
    journal = SessionJournal::open(&path).unwrap();
    let target_parent = journal.entries().iter().find(|entry| entry.id == discarded).unwrap().parent_id.clone();
    let marker = journal.discard_entry_durably(Some(&tier_two), &discarded, &partial).unwrap().unwrap();
    assert!(!journal.entries().iter().any(|entry| entry.id == discarded));
    assert_eq!(journal.entries().iter().find(|entry| entry.id == tier_one).unwrap().parent_id, target_parent);
    assert_eq!(
        journal.entries().iter().find(|entry| entry.id == tier_two).unwrap().parent_id.as_deref(),
        Some(tier_one.as_str())
    );
    assert_eq!(
        journal.entries().last().unwrap().raw["details"],
        json!({"kind":DISCARDED_ENTRY_BRANCH_MARKER,"discardedEntryId":discarded})
    );
    assert_eq!(journal.entries().last().unwrap().raw["summary"], "");
    assert!(journal.projected_compaction_snapshot().is_ok());
    let reopened = SessionJournal::open(&path).unwrap();
    assert_eq!(reopened.leaf_id(), Some(marker.as_str()));
    assert_reopened_context(&journal, &reopened);
    assert!(!reopened.model_context().iter().any(|message| message.as_assistant() == Some(&partial)));

    // A late Bash child retains its original ownership and raw off-branch subtree.
    let discarded = journal.append_message(&Message::Assistant(partial.clone())).unwrap();
    let token = journal.begin_failed_assistant_recovery(&discarded, &discarded, &partial).unwrap();
    let owner = journal.leaf_id().unwrap().to_owned();
    let bash = BashExecutionMessage {
        command: "echo original-owner".into(),
        output: "settled".into(),
        exit_code: Some(0),
        cancelled: false,
        truncated: false,
        timestamp: 9,
        meta: None,
        exclude_from_context: None,
    };
    let late_child = journal.append_bash_execution_to_branch(&bash, Some(&discarded)).unwrap();
    let raw_failed = journal.entries().iter().find(|entry| entry.id == discarded).unwrap().clone();
    let raw_child = journal.entries().iter().find(|entry| entry.id == late_child).unwrap().clone();
    let marker = journal.discard_entry_durably(Some(&owner), &discarded, &partial).unwrap().unwrap();
    assert_eq!(journal.entries().iter().find(|entry| entry.id == discarded), Some(&raw_failed));
    assert_eq!(journal.entries().iter().find(|entry| entry.id == late_child), Some(&raw_child));
    assert_eq!(
        journal.finish_failed_assistant_recovery(token, true).unwrap(),
        FailedAssistantRecoveryOutcome::Committed
    );
    let reopened = SessionJournal::open(&path).unwrap();
    assert_eq!(reopened.leaf_id(), Some(marker.as_str()));
    assert!(!reopened.branch().iter().any(|entry| entry.id == discarded || entry.id == late_child));

    // A failed atomic rewrite restores entries/IDs/leaf, including metadata.
    let discarded = journal.append_message(&Message::Assistant(partial.clone())).unwrap();
    let tier = journal.append_service_tier_change(&Value::Null).unwrap();
    let before_entries = journal.entries().to_vec();
    let before_bytes = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(journal.discard_entry_durably(Some(&tier), &discarded, &partial).is_err());
    assert_eq!(journal.entries(), before_entries);
    assert_eq!(journal.leaf_id(), Some(tier.as_str()));
    std::fs::remove_dir(&path).unwrap();
    std::fs::write(&path, before_bytes).unwrap();
    journal.discard_entry_durably(Some(&tier), &discarded, &partial).unwrap();
    assert!(SessionJournal::open(&path).unwrap().projected_compaction_snapshot().is_ok());

    // Unknown/panicked tool receipts are neither removed nor replay-authorized.
    let tool_directory = tempfile::tempdir().unwrap();
    let mut protected = SessionJournal::create(tool_directory.path(), tool_directory.path()).unwrap();
    let mut tool_turn = partial;
    tool_turn.content.push(AssistantBlock::ToolCall(ToolCall {
        id: "write-call".into(),
        name: "write".into(),
        arguments: JsonObject::new(),
        thought_signature: None,
    }));
    let tool_id = protected.append_message(&Message::Assistant(tool_turn.clone())).unwrap();
    let receipt = protected
        .append_message(&Message::ToolResult(ToolResultMessage {
            tool_call_id: "write-call".into(),
            tool_name: "write".into(),
            content: vec![],
            details: Some(json!({"executed":"unknown","panicked":true})),
            is_error: true,
            timestamp: 10,
        }))
        .unwrap();
    let protected_entries = protected.entries().to_vec();
    let protected_bytes = std::fs::read(protected.path()).unwrap();
    assert!(protected.begin_failed_assistant_recovery(&receipt, &tool_id, &tool_turn).is_err());
    assert!(protected.discard_entry_durably(Some(&receipt), &tool_id, &tool_turn).is_err());
    assert_eq!(protected.entries(), protected_entries);
    assert_eq!(std::fs::read(protected.path()).unwrap(), protected_bytes);
}
