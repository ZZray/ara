use ara_ai::{AssistantMessage, AssistantRetryRecovery, Message, StopReason, UserMessage};
use ara_session::SessionJournal;

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
