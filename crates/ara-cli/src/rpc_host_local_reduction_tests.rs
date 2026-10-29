//! Concentrated actual Host accounting/recovery flows. Generated canonical
//! journals exercise persistence and idle runtime refresh without model/network
//! calls. Counts are local estimates, not provider wire-fit evidence.
use super::*;
use ara_ai::{AssistantBlock, StopReason, ToolCall, ToolResultMessage, Usage};
use std::path::Path;

fn assistant(host: &Host, text: &str, input: Option<u64>) -> Message {
    let mut message =
        AssistantMessage::empty(&host.config.model.api, &host.config.model.provider, &host.config.model.id);
    message.content.push(AssistantBlock::text(text));
    message.usage.input = input;
    Message::Assistant(message)
}

fn read_call(host: &Host, id: &str) -> Message {
    let mut message =
        AssistantMessage::empty(&host.config.model.api, &host.config.model.provider, &host.config.model.id);
    message.content.push(AssistantBlock::ToolCall(ToolCall {
        id: id.into(),
        name: "read".into(),
        arguments: json!({"path":"file.ts"}).as_object().unwrap().clone(),
        thought_signature: None,
        ..Default::default()
    }));
    message.stop_reason = StopReason::ToolUse;
    Message::Assistant(message)
}

fn read_result(id: &str, text: &str) -> Message {
    Message::ToolResult(ToolResultMessage {
        tool_call_id: id.into(),
        tool_name: "read".into(),
        content: vec![UserBlock::text(text)],
        details: None,
        is_error: false,
        timestamp: ara_ai::now_ms(),
        provider_metadata: None,
        useless: false,
    })
}

fn install_raw_history(host: &mut Host, directory: &Path, messages: Vec<Value>) {
    let journal = SessionJournal::create(directory, directory).unwrap();
    let path = journal.path().to_path_buf();
    let mut lines = vec![serde_json::to_string(journal.header()).unwrap()];
    for (index, message) in messages.into_iter().enumerate() {
        lines.push(
            serde_json::to_string(&json!({"type":"message","id":format!("m{index}"),
            "parentId":index.checked_sub(1).map(|index|format!("m{index}")),
            "timestamp":"2026-10-02T00:00:00.000Z","message":message}))
            .unwrap(),
        );
    }
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    let journal = SessionJournal::open(&path).unwrap();
    let header = journal.header().clone();
    host.session = Session::new(Some(journal), header, &[], None, None).unwrap();
    let messages = {
        let journal = host.session.journal.try_lock().unwrap();
        host.route_raw_context(&journal).unwrap()
    };
    host.artifact_router.bind(host.session.artifacts.clone());
    host.agent.replace_idle_raw_messages(messages).unwrap();
}

async fn append_settled(host: &mut Host, messages: Vec<Message>) {
    let messages = {
        let mut journal = host.session.journal.lock().await;
        for message in messages {
            journal.append_message(&message).unwrap();
        }
        host.route_raw_context(&journal).unwrap()
    };
    host.agent.replace_idle_raw_messages(messages).unwrap();
}

#[tokio::test]
async fn native_host_provider_pending_prune_saved_and_noop_recovery_family() {
    let directory = tempfile::tempdir().unwrap();
    let (mut host, _) = super::tests::fixture();
    assert!(
        !host.rescue_local_history(None, Some((11_000, 0)), false, &[], None).await.unwrap(),
        "an already-fitting no-op cannot manufacture recovery progress"
    );

    let mut useless = serde_json::to_value(read_result("read", &"u".repeat(4_000))).unwrap();
    useless["useless"] = json!(true);
    let fence = format!("```text\n{}\n```", "f".repeat(4_000));
    let initial = vec![
        serde_json::to_value(Message::User(UserMessage::text("task"))).unwrap(),
        serde_json::to_value(read_call(&host, "read")).unwrap(),
        useless,
        serde_json::to_value(assistant(&host, &fence, None)).unwrap(),
        serde_json::to_value(assistant(&host, "accepted anchor", Some(10_000))).unwrap(),
    ];
    install_raw_history(&mut host, directory.path(), initial);
    let old_provider = host.config.provider.clone();
    let prune_saved = host.prune_local_history().await.unwrap();
    assert!(prune_saved > 900 && prune_saved < 1_000);
    assert!(
        !host.recovery_fits(None, Some((11_000, prune_saved)), &[]).await.unwrap(),
        "pruning alone remains above the 80 percent recovery band"
    );
    assert!(host.shake_local_history(true, None).await.unwrap());
    let current_provider = host.config.provider.clone();
    {
        let published = host.snapshot.read().unwrap();
        let binding = published.model.as_ref().unwrap();
        assert_eq!(binding.model.id, host.config.model.id);
        assert!(Arc::ptr_eq(&binding.provider, &current_provider));
        assert_eq!(published.system_prompt, host.config.system_prompt);
        assert_eq!(published.tools.len(), host.config.tools.len());
        assert!(published.tools.iter().zip(&host.config.tools).all(|(left, right)| Arc::ptr_eq(left, right)));
    }
    assert!(!Arc::ptr_eq(&old_provider, &current_provider), "rewrite closes the retained provider binding");

    assert!(
        host.recovery_fits(None, Some((11_000, prune_saved)), &[]).await.unwrap(),
        "this turn's prune savings survive the subsequent shake accounting"
    );
    assert!(
        !host.recovery_fits(None, Some((11_000, 0)), &[]).await.unwrap(),
        "the old usage anchor still needs the explicit prune correction"
    );
    let pending = [AgentMessage::from(Message::User(UserMessage::text("p".repeat(4_000))))];
    assert!(
        !host.recovery_fits(None, Some((11_000, prune_saved)), &pending).await.unwrap(),
        "pending User content belongs in both provider estimate and the 80 percent floor"
    );
    assert!(
        !host.rescue_local_history(None, Some((11_000, prune_saved)), false, &[], None).await.unwrap(),
        "after all regions are elided, a fitting no-op rescue still returns false"
    );

    // A retained, JSON-valid journal may still fail typed public projection.
    // Exercise the post-publication stop boundary in this existing Host family.
    for primary_error in [None, Some("original published history failure")] {
        let fault_directory = tempfile::tempdir().unwrap();
        let (mut retained, _) = super::tests::fixture();
        install_raw_history(
            &mut retained,
            fault_directory.path(),
            vec![serde_json::to_value(Message::User(UserMessage::text("retained public input"))).unwrap()],
        );
        let session = retained.session.clone();
        let public_before = session.messages.lock().unwrap().clone();
        let (path, header) = {
            let journal = session.journal.lock().await;
            (journal.path().to_path_buf(), serde_json::to_string(journal.header()).unwrap())
        };
        let malformed = json!({"type":"message","id":"malformed","parentId":null,
            "timestamp":"2026-10-02T00:00:00.000Z",
            "message":{"role":"assistant","content":"invalid typed assistant content"}});
        let published_bytes = format!("{header}\n{}\n", serde_json::to_string(&malformed).unwrap()).into_bytes();
        std::fs::write(&path, &published_bytes).unwrap();
        *session.journal.lock().await = SessionJournal::open(&path).unwrap();
        *session.persistence_error.lock().unwrap() = primary_error.map(str::to_owned);
        retained.agent.steer(Message::User(UserMessage::text("queued input")));
        retained.drain_queues = true;
        let error = {
            let journal = session.journal.lock().await;
            public_messages_after_publication(&journal, &session, &retained.connection, &mut retained.drain_queues)
                .unwrap_err()
        };
        assert!(error.to_string().contains("Session public projection failed after publication"));
        let latched = session.persistence_error.lock().unwrap().clone().unwrap();
        match primary_error {
            Some(primary) => {
                assert!(latched.starts_with(primary), "the original publication error remains primary");
                assert!(
                    latched.contains("; secondary observation: Session public projection failed after publication")
                );
            }
            None => assert!(latched.starts_with("Session public projection failed after publication")),
        }
        assert!(retained.connection.is_cancelled());
        assert!(!retained.drain_queues);
        assert!(retained.agent.has_queued_messages(), "projection failure does not replay queued input");
        assert_eq!(*session.messages.lock().unwrap(), public_before);
        assert_eq!(std::fs::read(&path).unwrap(), published_bytes, "observation does not rewrite durable history");
    }
}

#[tokio::test]
async fn native_host_images_rebase_stale_prune_and_fresh_total_usage_family() {
    let directory = tempfile::tempdir().unwrap();
    let (mut host, _) = super::tests::fixture();
    let image_user = Message::User(UserMessage {
        content: UserContent::Blocks(vec![UserBlock::Image(ImageContent {
            data: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB".into(),
            mime_type: "image/png".into(),
            detail: None,
            compaction_frame: false,
        })]),
        synthetic: None,
        timestamp: ara_ai::now_ms(),
        provider_payload: None,
        provider_payload_origin: None,
    });
    let initial = vec![
        serde_json::to_value(image_user).unwrap(),
        serde_json::to_value(assistant(&host, "done", Some(190_000))).unwrap(),
    ];
    install_raw_history(&mut host, directory.path(), initial);
    let before = host.context_tokens(&host.agent.raw_messages().await, &[]).await.unwrap();
    assert_eq!(before, 190_000);
    assert!(!host.recovery_fits(None, Some((1_000, 0)), &[]).await.unwrap());
    assert!(host.rescue_local_history(None, Some((1_000, 0)), true, &[], None).await.unwrap());
    assert!(host.local_context_rebase.is_some());
    let rebased = host.context_tokens(&host.agent.raw_messages().await, &[]).await.unwrap();
    assert!(rebased < 100, "actual image rewrite rebases the stale 190k estimate");
    assert_eq!(
        host.session.journal.lock().await.entries()[1].raw["message"]["usage"]["input"],
        190_000,
        "rebase does not fabricate a replacement provider usage receipt"
    );

    let settled = vec![
        read_call(&host, "old"),
        read_result("old", &"old ".repeat(200)),
        read_call(&host, "new"),
        read_result("new", &"new ".repeat(200)),
        assistant(&host, "read complete", None),
    ];
    append_settled(&mut host, settled).await;
    assert!(host.prune_local_history().await.unwrap() > 0);
    assert!(host.local_context_rebase.is_some());
    let after_prune = host.context_tokens(&host.agent.raw_messages().await, &[]).await.unwrap();
    assert!(after_prune < 1_000, "body changes during stale pruning cannot resurrect the invalidated 190k anchor");

    let mut fresh = match assistant(&host, "fresh usage", None) {
        Message::Assistant(message) => message,
        _ => unreachable!(),
    };
    fresh.usage = Usage { total_tokens: Some(1_234), output: Some(10), ..Usage::unknown() };
    append_settled(&mut host, vec![Message::Assistant(fresh)]).await;
    assert_eq!(
        host.context_tokens(&host.agent.raw_messages().await, &[]).await.unwrap(),
        1_234,
        "a new valid total-only receipt supersedes the local rebase"
    );
    assert!(!host.recovery_fits(None, Some((1_000, 0)), &[]).await.unwrap());
}
