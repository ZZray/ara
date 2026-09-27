use ara_agent::compaction::{
    SummaryInputError, SummarySource, WholeTurnCutCandidate, WholeTurnCutSelection, select_whole_turn_cut,
    serialize_sources_for_summary, whole_turn_cut_candidates,
};
use ara_ai::{
    AssistantBlock, AssistantMessage, DeveloperMessage, ImageContent, JsonObject, Message, StopReason, ToolCall,
    ToolResultMessage, UserBlock, UserContent, UserMessage,
};
use serde_json::json;

fn user(text: &str) -> Message {
    Message::User(UserMessage::text(text))
}

fn developer(text: &str) -> Message {
    Message::Developer(DeveloperMessage { content: UserContent::Text(text.into()), timestamp: 0 })
}

fn assistant(reason: StopReason, call: Option<&str>) -> Message {
    let mut message = AssistantMessage::empty("openai-completions", "fake", "m");
    message.stop_reason = reason;
    if let Some(id) = call {
        message.content.push(AssistantBlock::ToolCall(ToolCall {
            id: id.into(),
            name: "write".into(),
            arguments: JsonObject::new(),
            thought_signature: None,
        }));
    } else {
        message.content.push(AssistantBlock::text("done"));
    }
    Message::Assistant(message)
}

fn tool_result(id: &str, unknown_effect: bool) -> Message {
    Message::ToolResult(ToolResultMessage {
        tool_call_id: id.into(),
        tool_name: "write".into(),
        content: vec![UserBlock::text("receipt")],
        details: unknown_effect.then(|| json!({"timedOut":true})),
        is_error: unknown_effect,
        timestamp: 0,
    })
}

fn cuts(ids: &[&str], messages: &[Message]) -> Vec<WholeTurnCutCandidate> {
    assert_eq!(ids.len(), messages.len());
    let sources =
        ids.iter().zip(messages).map(|(entry_id, message)| SummarySource { entry_id, message }).collect::<Vec<_>>();
    whole_turn_cut_candidates(&sources)
}

fn at(index: usize, id: &str) -> WholeTurnCutCandidate {
    WholeTurnCutCandidate { first_kept_index: index, first_kept_entry_id: id.into() }
}

fn selected(
    ids: &[&str],
    messages: &[Message],
    target: usize,
) -> Result<Option<WholeTurnCutSelection>, SummaryInputError> {
    let sources =
        ids.iter().zip(messages).map(|(entry_id, message)| SummarySource { entry_id, message }).collect::<Vec<_>>();
    select_whole_turn_cut(&sources, target, None)
}

#[test]
fn offers_only_complete_turn_boundaries_and_keeps_in_progress_tail() {
    let messages = [
        user("one"),
        assistant(StopReason::Stop, None),
        user("two"),
        assistant(StopReason::Stop, None),
        user("unfinished"),
    ];
    assert_eq!(cuts(&["e1", "e2", "e3", "e4", "e5"], &messages), vec![at(2, "e3"), at(4, "e5")]);
}

#[test]
fn never_cuts_between_tool_call_receipt_and_final_assistant() {
    let messages = [
        user("run"),
        assistant(StopReason::ToolUse, Some("c1")),
        tool_result("c1", false),
        assistant(StopReason::Stop, None),
        user("next"),
    ];
    assert_eq!(cuts(&["e1", "e2", "e3", "e4", "e5"], &messages), vec![at(4, "e5")]);

    let unfinished = [user("run"), assistant(StopReason::ToolUse, Some("c1")), tool_result("c1", false), user("next")];
    assert!(cuts(&["e1", "e2", "e3", "e4"], &unfinished).is_empty());
}

#[test]
fn rejects_unknown_effects_and_failed_turns_from_summary_prefix() {
    let unknown = [
        user("run"),
        assistant(StopReason::ToolUse, Some("c1")),
        tool_result("c1", true),
        assistant(StopReason::Stop, None),
        user("next"),
    ];
    assert!(cuts(&["e1", "e2", "e3", "e4", "e5"], &unknown).is_empty());

    let failed = [user("run"), assistant(StopReason::Error, None), user("next")];
    assert!(cuts(&["e1", "e2", "e3"], &failed).is_empty());
}

#[test]
fn never_downgrades_developer_content_to_summary_text() {
    let messages = [
        user("one"),
        assistant(StopReason::Stop, None),
        user("two"),
        developer("high priority"),
        assistant(StopReason::Stop, None),
        user("three"),
    ];
    assert_eq!(cuts(&["e1", "e2", "e3", "e4", "e5", "e6"], &messages), vec![at(2, "e3")]);

    let starts_high_priority =
        [developer("high priority"), user("one"), assistant(StopReason::Stop, None), user("two")];
    assert!(cuts(&["e1", "e2", "e3", "e4"], &starts_high_priority).is_empty());
}

#[test]
fn rejects_invalid_first_kept_id_and_noncomplete_history() {
    let messages = [user("one"), assistant(StopReason::Stop, None), user("two")];
    assert!(cuts(&["e1", "e2", "e1"], &messages).is_empty());
    assert!(cuts(&["e1", "e2", "bad/id"], &messages).is_empty());
    assert!(cuts(&["e1", "e2"], &messages[..2]).is_empty());
    assert!(cuts(&[], &[]).is_empty());
}

#[test]
fn structural_boundary_does_not_claim_prompt_payload_support() {
    let image = Message::User(UserMessage {
        content: UserContent::Blocks(vec![UserBlock::Image(ImageContent {
            data: "YWJj".into(),
            mime_type: "image/png".into(),
        })]),
        synthetic: None,
        timestamp: 0,
    });
    let messages = [image, assistant(StopReason::Stop, None), user("next")];
    let ids = ["e1", "e2", "e3"];
    assert_eq!(cuts(&ids, &messages), vec![at(2, "e3")]);
    let sources =
        ids.iter().zip(&messages).map(|(entry_id, message)| SummarySource { entry_id, message }).collect::<Vec<_>>();
    assert_eq!(serialize_sources_for_summary(&sources[..2]), Err(SummaryInputError::UnsupportedImage));
}

#[test]
fn candidates_stop_at_the_summary_source_limit() {
    let mut messages = Vec::new();
    for _ in 0..129 {
        messages.push(user("turn"));
        messages.push(assistant(StopReason::Stop, None));
    }
    messages.push(user("next"));
    let ids = (0..messages.len()).map(|index| format!("e{index}")).collect::<Vec<_>>();
    let id_refs = ids.iter().map(String::as_str).collect::<Vec<_>>();
    let candidates = cuts(&id_refs, &messages);
    assert_eq!(candidates.last(), Some(&at(256, "e256")));
    assert!(candidates.iter().all(|candidate| candidate.first_kept_index <= 256));
    let selection = selected(&id_refs, &messages, 0).unwrap().unwrap();
    assert_eq!(selection.candidate, at(256, "e256"));
    assert!(selection.estimated_retained_exceeds_target);
}

#[test]
fn selects_the_most_recent_complete_turn_boundary_within_the_estimated_target() {
    let messages = [
        user("one"),
        assistant(StopReason::Stop, None),
        user("abcdefgh"),
        assistant(StopReason::Stop, None),
        user("tail"),
        assistant(StopReason::Stop, None),
    ];
    let ids = ["e1", "e2", "e3", "e4", "e5", "e6"];
    assert_eq!(selected(&ids, &messages, 7).unwrap(), None, "already within target");
    assert_eq!(
        selected(&ids, &messages, 5).unwrap(),
        Some(WholeTurnCutSelection {
            candidate: at(2, "e3"),
            estimated_retained_raw_tokens: 5,
            estimated_retained_exceeds_target: false,
        })
    );
    assert_eq!(
        selected(&ids, &messages, 4).unwrap(),
        Some(WholeTurnCutSelection {
            candidate: at(4, "e5"),
            estimated_retained_raw_tokens: 2,
            estimated_retained_exceeds_target: false,
        }),
        "a target inside the middle turn preserves the whole newest turn"
    );
    for target in [0, 1] {
        assert_eq!(
            selected(&ids, &messages, target).unwrap(),
            Some(WholeTurnCutSelection {
                candidate: at(4, "e5"),
                estimated_retained_raw_tokens: 2,
                estimated_retained_exceeds_target: true,
            })
        );
    }
}

#[test]
fn keeps_an_unfinished_tool_tail_raw_and_reports_oversized_recent_turn() {
    let messages = [
        user("one"),
        assistant(StopReason::Stop, None),
        user("run"),
        assistant(StopReason::ToolUse, Some("c1")),
        tool_result("c1", false),
    ];
    let selection = selected(&["e1", "e2", "e3", "e4", "e5"], &messages, 1).unwrap().unwrap();
    assert_eq!(selection.candidate, at(2, "e3"));
    assert!(selection.estimated_retained_exceeds_target);
    assert!(selection.estimated_retained_raw_tokens > 1);
}

#[test]
fn refuses_a_cut_whose_summary_prompt_cannot_preserve_its_source() {
    let image = Message::User(UserMessage {
        content: UserContent::Blocks(vec![UserBlock::Image(ImageContent {
            data: "YWJj".into(),
            mime_type: "image/png".into(),
        })]),
        synthetic: None,
        timestamp: 0,
    });
    let messages = [image, assistant(StopReason::Stop, None), user("next")];
    assert_eq!(selected(&["e1", "e2", "e3"], &messages, 0), Err(SummaryInputError::UnsupportedImage));

    let messages = [user(&"x".repeat(1_000_000)), assistant(StopReason::Stop, None), user("next")];
    assert_eq!(selected(&["e1", "e2", "e3"], &messages, 0), Err(SummaryInputError::TooLarge));
}

#[test]
fn falls_back_to_an_earlier_boundary_to_keep_unsupported_content_raw() {
    let image = Message::User(UserMessage {
        content: UserContent::Blocks(vec![UserBlock::Image(ImageContent {
            data: "YWJj".into(),
            mime_type: "image/png".into(),
        })]),
        synthetic: None,
        timestamp: 0,
    });
    let ids = ["e1", "e2", "e3", "e4", "e5"];
    let messages =
        [user("text"), assistant(StopReason::Stop, None), image, assistant(StopReason::Stop, None), user("next")];
    let selection = selected(&ids, &messages, 0).unwrap().unwrap();
    assert_eq!(selection.candidate, at(2, "e3"));
    assert!(selection.estimated_retained_exceeds_target);

    let messages = [
        user("text"),
        assistant(StopReason::Stop, None),
        user(&"x".repeat(1_000_000)),
        assistant(StopReason::Stop, None),
        user("next"),
    ];
    let selection = selected(&ids, &messages, 0).unwrap().unwrap();
    assert_eq!(selection.candidate, at(2, "e3"));
    assert!(selection.estimated_retained_exceeds_target);
}
