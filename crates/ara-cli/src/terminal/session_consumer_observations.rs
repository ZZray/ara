//! Actual selected Rust consumer contracts; no terminal/visual acceptance claim.
use super::*;
use ara_session::context_view::{TranscriptView, TranscriptViewMessage};
use serde_json::json;

fn body(value: serde_json::Value) -> SessionWire {
    SessionWire::parse(&value.to_string()).unwrap()
}
fn observation(revision: u64, rows: Vec<(&str, SessionWire)>) -> SessionTranscriptObservation {
    let len = rows.len();
    SessionTranscriptObservation {
        owner: None,
        session_id: "consumer-fixture".into(),
        revision,
        collapsed: false,
        view: TranscriptView {
            messages: rows.into_iter().map(|(id, body)| TranscriptViewMessage { entry_id: id.into(), body }).collect(),
            cache_miss_explained_at: vec![false; len],
        },
        originals: HashMap::new(),
    }
}
fn user() -> SessionWire {
    body(json!({"role":"user","content":"large settled user turn","timestamp":1}))
}
fn assistant() -> SessionWire {
    let mut a = ara_ai::AssistantMessage::empty("anthropic-messages", "anthropic", "test");
    a.timestamp = 1;
    a.usage = serde_json::from_value(json!({"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":2,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}})).unwrap();
    a.content
        .push(ara_ai::AssistantBlock::text("## Large settled assistant turn\n\n```ts\nconst retained = true;\n```"));
    body(serde_json::to_value(ara_ai::Message::Assistant(a)).unwrap())
}
fn summary(warning: &str) -> SessionWire {
    body(
        json!({"role":"compactionSummary","summary":"A **real** summary", "tokensBefore":256000,"tokensAfter":20000,"method":"remote","warning":warning,"timestamp":1}),
    )
}

#[test]
fn fixed_tui_summary_consumer_settled_reuse_and_source_cache() {
    // B-0795aea21e: retain the SAME settled user+assistant component instances,
    // using its original text fixtures rather than arbitrary short content.
    let mut controller = EventController::new(Arc::new(Theme::dark()), ara_edit::EditMode::Hashline);
    controller.observe_session(observation(1, vec![("u", user()), ("a", assistant())]), None).unwrap();
    let user_key = BlockKey::Session("u".into(), 0);
    let assistant_key = BlockKey::Session("a".into(), 1);
    let user_rows = match controller.session_transcript.rows.get_mut(&user_key).unwrap() {
        SessionRow::Markdown(c) => c.render(80),
        _ => panic!(),
    };
    let assistant_rows = match controller.session_transcript.rows.get_mut(&assistant_key).unwrap() {
        SessionRow::Markdown(c) => c.render(80),
        _ => panic!(),
    };
    controller.observe_session(observation(2, vec![("u", user()), ("a", assistant())]), None).unwrap();
    let user_again = match controller.session_transcript.rows.get_mut(&user_key).unwrap() {
        SessionRow::Markdown(c) => c.render(80),
        _ => panic!(),
    };
    let assistant_again = match controller.session_transcript.rows.get_mut(&assistant_key).unwrap() {
        SessionRow::Markdown(c) => c.render(80),
        _ => panic!(),
    };
    assert!(Arc::ptr_eq(&user_rows, &user_again));
    assert!(Arc::ptr_eq(&assistant_rows, &assistant_again));
    controller
        .observe_session(observation(3, vec![("u", user()), ("a", assistant()), ("c", summary("first warning"))]), None)
        .unwrap();
    controller.toggle_summary_expanded();
    let key = BlockKey::Session("c".into(), 0);
    let first = controller.session_rows(&key, 80).unwrap();
    assert!(first.join("\n").contains("first warning"));
    controller
        .observe_session(
            observation(4, vec![("u", user()), ("a", assistant()), ("c", summary("changed warning"))]),
            None,
        )
        .unwrap();
    let changed = controller.session_rows(&key, 80).unwrap();
    assert!(changed.join("\n").contains("changed warning"));
    assert!(!changed.join("\n").contains("first warning"));
    controller.observe_session(observation(3, vec![("c", summary("stale warning"))]), None).unwrap();
    assert!(controller.session_rows(&key, 80).unwrap().join("\n").contains("changed warning"));
    let old_theme = controller.session_transcript.theme.unwrap();
    controller.theme = Arc::new(Theme::light());
    controller
        .observe_session(
            observation(5, vec![("u", user()), ("a", assistant()), ("c", summary("changed warning"))]),
            None,
        )
        .unwrap();
    assert_ne!(controller.session_transcript.theme.unwrap(), old_theme);
}

#[test]
fn fixed_tui_summary_consumer_entry_receipt_and_native_owner() {
    let mut controller = EventController::new(Arc::new(Theme::dark()), ara_edit::EditMode::Hashline);
    controller.start_entry_observation();
    controller.observe_entry_receipt("u".into(), Arc::new(user())).unwrap();
    assert_eq!(controller.transcript.block_keys(), [BlockKey::Session("u".into(), 0)]);
    controller.observe_session(observation(1, vec![("u", user())]), None).unwrap();
    controller.observe_session(observation(2, vec![("u", user())]), None).unwrap();
    assert_eq!(
        controller.transcript.block_keys(),
        [BlockKey::Session("u".into(), 0)],
        "original persisted user receipt must not stack replay rows"
    );
    let first = crate::native_factory::PreparedNativeSession::prepare().unwrap();
    let other = crate::native_factory::PreparedNativeSession::prepare().unwrap();
    let mut view = observation(1, vec![("u", user())]);
    view.owner = Some(first.clone());
    assert!(view.same_owner(Some(&first)));
    assert!(!view.same_owner(Some(&other)));
    assert!(!view.same_owner(None));
    let mut wrong = observation(3, vec![("u", user())]);
    wrong.session_id = "different-logical-session".into();
    assert!(controller.observe_session(wrong, None).is_err());
}

#[test]
fn fixed_tui_summary_consumer_history_offer_before_collapse_writer_ack() {
    use super::super::transcript::HistoryKind;
    let mut transcript = Transcript::default();
    transcript.add_text(1, "old physical history".into(), true);
    transcript.add_tool(BlockKey::Session("c".into(), 0), true);
    let old = transcript.prepare_frame(80, 30, |_, _, _| Some(vec!["".into(), "summary divider".into(), "".into()]));
    transcript.commit_frame(old);
    transcript.begin_history_flush();
    let offered =
        transcript.prepare_frame(80, 30, |_, _, _| Some(vec!["".into(), "summary divider".into(), "".into()]));
    let old_receipt = offered.plan.history.clone().unwrap();
    assert_eq!(old_receipt.kind, HistoryKind::Append);
    transcript.commit_frame(offered);
    transcript.select_session(vec![BlockKey::Session("c".into(), 0)], true);
    assert!(!transcript.acknowledge_history(old_receipt.id + 1));
    let unchanged =
        transcript.prepare_frame(80, 30, |_, _, _| Some(vec!["".into(), "summary divider".into(), "".into()]));
    assert_eq!(unchanged.plan.history.as_ref().unwrap().id, old_receipt.id);
    assert_eq!(unchanged.plan.history.as_ref().unwrap().rows, old_receipt.rows);
    assert_eq!(transcript.block_keys().len(), 2, "logical commit is not physical ack");
    assert!(transcript.acknowledge_history(old_receipt.id));
    let replacement =
        transcript.prepare_frame(80, 30, |_, _, _| Some(vec!["".into(), "summary divider".into(), "".into()]));
    let new_receipt = replacement.plan.history.clone().unwrap();
    assert_eq!(new_receipt.kind, HistoryKind::ReplaceCollapsed);
    assert_ne!(new_receipt.id, old_receipt.id);
    let bytes = super::super::driver::compose_frame(&[], &replacement.plan);
    assert!(bytes.contains("\x1b[3J\x1b[2J\x1b[H"));
    assert!(!bytes.contains("old physical history"));
    assert_eq!(
        new_receipt.rows.first().map(String::as_str),
        Some(""),
        "summary structural leading blank survives tool trim"
    );
    transcript.commit_frame(replacement);
    assert!(!transcript.acknowledge_history(old_receipt.id));
    let retry = transcript.prepare_frame(80, 30, |_, _, _| Some(vec!["".into(), "summary divider".into(), "".into()]));
    assert_eq!(
        retry.plan.history.as_ref().unwrap().rows,
        new_receipt.rows,
        "failed writer retains exact replacement offer"
    );
    assert!(transcript.acknowledge_history(new_receipt.id));
    let done = transcript.prepare_frame(80, 30, |_, _, _| Some(vec![]));
    assert!(done.plan.history.is_none());
}

#[test]
fn fixed_tui_summary_consumer_full_retains_physical_scrollback() {
    let mut transcript = Transcript::default();
    transcript.add_text(1, "settled".into(), true);
    let initial = transcript.render_frame(80, 30);
    assert!(initial.history.is_none());
    transcript.begin_history_flush();
    let offered = transcript.render_frame(80, 30);
    transcript.acknowledge_history(offered.history.unwrap().id);
    transcript.add_tool(BlockKey::Session("c".into(), 0), true);
    transcript.select_session(vec![BlockKey::Text(1), BlockKey::Session("c".into(), 0)], false);
    let frame = transcript.prepare_frame(80, 30, |_, _, _| Some(vec!["".into(), "divider".into(), "".into()]));
    assert!(!super::super::driver::compose_frame(&[], &frame.plan).contains("\x1b[3J"));
    assert!(
        !frame.plan.history.as_ref().is_some_and(|history| history.rows.iter().any(|r| r == "settled")),
        "unchanged committed prefix cannot be appended twice"
    );
}

#[test]
fn fixed_tui_summary_consumer_generic_result_before_message_start() {
    // Local regression for the existing generic-text fallback; this does NOT
    // claim OMP Bash ToolExecutionComponent pending/identity equivalence.
    let mut controller = EventController::new(Arc::new(Theme::dark()), ara_edit::EditMode::Hashline);
    let result = ara_agent::ToolOutput::text("original generic result");
    controller
        .consume_in(
            &AgentEvent::ToolExecutionEnd {
                tool_call_id: "generic-call".into(),
                tool_name: "bash".into(),
                result,
                is_error: false,
            },
            None,
            None,
        )
        .unwrap();
    let original_keys = controller.transcript.block_keys();
    assert_eq!(original_keys.len(), 1);
    controller.start_entry_observation();
    let result_body = body(
        json!({"role":"toolResult","toolCallId":"generic-call","toolName":"bash","content":[{"type":"text","text":"original generic result"}],"isError":false,"timestamp":1}),
    );
    controller.observe_entry_receipt("r".into(), Arc::new(ara_session::native_clone_wire(&result_body))).unwrap();
    controller
        .observe_session(observation(1, vec![("r", ara_session::native_clone_wire(&result_body))]), None)
        .unwrap();
    assert_eq!(
        controller.transcript.block_keys(),
        original_keys,
        "receipt preserves the already-rendered result before MessageStart"
    );
    controller.theme = Arc::new(Theme::light());
    controller.observe_session(observation(2, vec![("r", result_body)]), None).unwrap();
    assert_eq!(
        controller.transcript.block_keys(),
        original_keys,
        "theme rebuild must not stack a second generic result"
    );
}
