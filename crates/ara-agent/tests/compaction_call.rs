use ara_agent::compaction::{
    SummaryCallErrorKind, SummaryInputError, SummarySource, summarize_sources, summarize_sources_with_instructions,
    summary_output_budget_tokens,
};
use ara_ai::providers::openai_completions::{RetryPolicy, StreamOptions};
use ara_ai::{
    AssistantBlock, AssistantMessage, AssistantMessageEvent, AssistantStream, CallOptions, Context, DeveloperMessage,
    EventSink, ImageContent, Message, Model, ModelProvider, OpenAICompletionsProvider, StopReason, ToolCall,
    ToolChoice, Usage, UserContent, UserMessage,
};
use ara_testkit::FakeUpstream;
use ara_testkit::chunks::{done as sse_done, finish, text as sse_text};
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

enum Script {
    Events(Vec<AssistantMessageEvent>),
    WaitForCancel,
}

type FoldingResponse = dyn Fn(usize, &Context, &CallOptions) -> AssistantMessage + Send + Sync;

struct FoldingProvider {
    seen: Mutex<Vec<(Context, CallOptions)>>,
    response: Box<FoldingResponse>,
}

impl ModelProvider for FoldingProvider {
    fn stream(&self, _model: &Model, context: &Context, options: CallOptions) -> AssistantStream {
        let index = {
            let mut seen = self.seen.lock().unwrap();
            let index = seen.len();
            seen.push((context.clone(), options.clone()));
            index
        };
        let response = (self.response)(index, context, &options);
        let (sink, stream) = EventSink::channel();
        tokio::spawn(async move {
            sink.push(done(response)).await;
        });
        stream
    }
}

// Fixed compaction-summary-cap and compaction-oversized-input native scenarios,
// grouped through the same complete-span → window fold → terminal receipt path.
#[tokio::test]
async fn native_summary_output_budget_and_input_folding_module_scenarios() {
    for case in [
        "fitting",
        "large-cap",
        "small-reserve",
        "fold",
        "carry",
        "provider-overflow",
        "single-huge",
        "non-overflow",
        "length-then-error",
        "cancel-window-two",
        "floor-deadend",
        "tool-window",
        "unknown-tool-veto",
    ] {
        let mut selected = model();
        selected.context_window = Some(match case {
            "provider-overflow" => 400_000.0,
            "fold" | "carry" | "single-huge" | "non-overflow" | "length-then-error" | "cancel-window-two" => 40_000.0,
            "tool-window" | "unknown-tool-veto" => 16_384.0,
            _ => 200_000.0,
        });
        let reserve = match case {
            "large-cap" => Some(150_000.0),
            "small-reserve" => Some(10_000.0),
            _ => None,
        };
        let max_tokens = summary_output_budget_tokens(reserve).unwrap();
        assert_eq!(
            max_tokens,
            match case {
                "large-cap" => 16_384,
                "small-reserve" => 8_000,
                _ => 13_107,
            }
        );
        let turns = match case {
            "provider-overflow" => 60,
            "fold" | "carry" | "non-overflow" | "length-then-error" | "cancel-window-two" => 12,
            _ => 1,
        };
        let repeats = if turns > 1 { if case == "provider-overflow" { 2000 } else { 2500 } } else { 40 };
        let mut messages = (0..turns)
            .flat_map(|index| {
                [
                    Message::User(UserMessage::text(format!("turn {index} {}", "work ".repeat(repeats)))),
                    Message::Assistant(AssistantMessage::empty("openai-completions", "test", "summary-model")),
                ]
            })
            .collect::<Vec<_>>();
        if case == "single-huge" {
            messages[0] =
                Message::User(UserMessage::text(format!("huge source {} tail withheld", "🧭".repeat(80_000))));
        }
        if matches!(case, "tool-window" | "unknown-tool-veto") {
            messages[0] = Message::User(UserMessage::text("read original source ".repeat(1000)));
            let mut calling = AssistantMessage::empty("openai-completions", "test", "summary-model");
            calling.stop_reason = StopReason::ToolUse;
            calling.content.push(AssistantBlock::ToolCall(ToolCall {
                id: "sourced-call".into(),
                name: "read".into(),
                arguments: json!({"source":"original","padding":"work ".repeat(2000)}).as_object().unwrap().clone(),
                thought_signature: None,
            }));
            messages[1] = Message::Assistant(calling);
            messages.push(Message::ToolResult(ara_ai::ToolResultMessage {
                tool_call_id: "sourced-call".into(),
                tool_name: "read".into(),
                content: vec![ara_ai::UserBlock::text("original tool receipt ".repeat(100))],
                details: (case == "unknown-tool-veto").then(|| json!({"executed":"unknown","panicked":true})),
                is_error: case == "unknown-tool-veto",
                timestamp: 77,
            }));
            messages.push(Message::Assistant(AssistantMessage::empty("openai-completions", "test", "summary-model")));
        }
        let raw_before = serde_json::to_value(&messages).unwrap();
        let ids = (0..messages.len()).map(|index| format!("e-{index}")).collect::<Vec<_>>();
        let inputs = ids
            .iter()
            .zip(&messages)
            .map(|(entry_id, message)| SummarySource { entry_id, message })
            .collect::<Vec<_>>();
        let cancel = CancellationToken::new();
        let provider_cancel = cancel.clone();
        let provider = FoldingProvider {
            seen: Mutex::new(Vec::new()),
            response: Box::new(move |index, context, _options| {
                let text = match &context.messages[0] {
                    Message::User(user) => user.content.plain_text(),
                    _ => panic!("summary prompt"),
                };
                let mut response = AssistantMessage::empty("openai-completions", "test", "summary-model");
                response.response_id = Some(format!("summary-request-{index}"));
                response.usage = Usage { output: Some(index as u64 + 1), ..Usage::unknown() };
                if (case == "provider-overflow" && text.len() > 160_000) || case == "floor-deadend" {
                    response.stop_reason = StopReason::Error;
                    response.error_status = Some(400);
                    // Classification must use the original terminal message,
                    // not the independently bounded 512-character diagnostic.
                    response.error_message = Some(format!(
                        "{} prompt is too long: {} tokens > 160000 maximum",
                        "diagnostic ".repeat(70),
                        text.len()
                    ));
                } else if case == "non-overflow" || (case == "length-then-error" && index == 1) {
                    response.stop_reason = StopReason::Error;
                    response.error_status = Some(500);
                    response.error_message = Some("provider exploded".into());
                } else {
                    response.content.push(AssistantBlock::text(format!("fold summary {index}")));
                    if matches!(case, "length-then-error" | "cancel-window-two") && index == 0 {
                        response.stop_reason = StopReason::Length;
                    }
                }
                if case == "cancel-window-two" && index == 1 {
                    provider_cancel.cancel();
                }
                response
            }),
        };
        let previous = (case == "carry").then_some("earlier caller summary");
        let result = summarize_sources_with_instructions(
            &inputs,
            previous,
            Some("retain original observations"),
            &selected,
            &provider,
            max_tokens,
            Instant::now() + Duration::from_secs(5),
            &cancel,
        )
        .await;
        let seen = provider.seen.lock().unwrap();
        assert_eq!(
            serde_json::to_value(&messages).unwrap(),
            raw_before,
            "{case}: no raw observation or receipt rewrite"
        );
        if case == "unknown-tool-veto" {
            assert_eq!(
                result.unwrap_err().kind,
                SummaryCallErrorKind::InvalidInput(SummaryInputError::UnknownToolEffect)
            );
            assert!(seen.is_empty());
            continue;
        }
        for (context, options) in seen.iter() {
            assert_eq!(context.tools, Some(Vec::new()));
            assert_eq!(options.tool_choice, Some(ToolChoice::None));
            assert_eq!(options.max_tokens, Some(max_tokens));
            let Message::User(user) = &context.messages[0] else { panic!("summary user") };
            assert!(user.content.plain_text().ends_with("Additional focus: retain original observations"));
        }
        if matches!(case, "non-overflow" | "length-then-error" | "cancel-window-two" | "floor-deadend") {
            let error = result.unwrap_err();
            assert_eq!(error.invocations.len(), seen.len(), "{case}: every started provider call has a receipt");
            if matches!(case, "length-then-error" | "cancel-window-two") {
                assert_eq!(
                    error.kind,
                    if case == "cancel-window-two" {
                        SummaryCallErrorKind::Cancelled
                    } else {
                        SummaryCallErrorKind::IncompleteResponse
                    }
                );
                assert_eq!(seen.len(), 2);
                assert!(error.invocations[0].error_kind.is_none());
                assert_eq!(error.invocations[0].stop_reason, Some(StopReason::Length));
                assert_eq!(error.invocations[0].window_source_entry_ids.len(), 2);
                let Message::User(second) = &seen[1].0.messages[0] else { panic!("next fold window") };
                assert!(
                    second.content.plain_text().contains("<previous-summary>\nfold summary 0\n</previous-summary>")
                );
            } else {
                assert_eq!(seen.len(), 1);
                assert!(!error.invocations[0].overflow_replanned);
                assert_eq!(error.invocations[0].classification.unwrap().overflow, case == "floor-deadend");
            }
            continue;
        }
        let summary = result.unwrap();
        assert_eq!(
            summary.invocations.len(),
            seen.len(),
            "{case}: final-call usage cannot stand for the complete fold"
        );
        assert_eq!(summary.window_source_entry_ids, ids);
        assert_eq!(summary.text, format!("fold summary {}", seen.len() - 1));
        assert_eq!(summary.usage.output, Some(seen.len() as u64));
        assert_eq!(summary.usage.input, None);
        let accepted_ids = summary
            .invocations
            .iter()
            .filter(|receipt| receipt.error_kind.is_none())
            .flat_map(|receipt| receipt.window_source_entry_ids.clone())
            .collect::<Vec<_>>();
        assert_eq!(accepted_ids, ids, "{case}: accepted windows cover each original source once in order");
        let mut carried = previous.map(str::to_owned);
        for (index, ((context, _), receipt)) in seen.iter().zip(&summary.invocations).enumerate() {
            let Message::User(user) = &context.messages[0] else { panic!("summary user") };
            let text = user.content.plain_text();
            if let Some(previous) = &carried {
                assert!(text.contains(&format!("<previous-summary>\n{previous}\n</previous-summary>")), "{case}");
            } else {
                assert!(!text.contains("<previous-summary>"), "{case}");
            }
            assert!(receipt.usage.as_ref().unwrap().input.is_none());
            if receipt.error_kind.is_none() {
                carried = Some(format!("fold summary {index}"));
            }
            if matches!(case, "fold" | "carry") {
                assert!(text.len() <= 26_000, "small-model floor must fit native planned windows");
            }
        }
        if matches!(case, "fitting" | "large-cap" | "small-reserve") {
            assert_eq!(seen.len(), 1);
        } else {
            assert!(seen.len() > 1, "{case}: exercise actual folding");
        }
        if case == "provider-overflow" {
            let rejected = summary.invocations.iter().filter(|receipt| receipt.overflow_replanned).count();
            assert!(
                (1..4).contains(&rejected),
                "halve observed conversation, not the imaginary catalog ladder: {rejected}"
            );
        }
        if case == "single-huge" {
            let Message::User(first) = &seen[0].0.messages[0] else { panic!("clamped user") };
            let text = first.content.plain_text();
            assert!(text.contains("more characters truncated]"));
            assert!(text.contains("huge source"));
            assert!(!text.contains("tail withheld"));
        }
    }
}

struct ScriptedProvider {
    script: Mutex<Option<Script>>,
    seen: Mutex<Option<(Context, CallOptions)>>,
}

impl ScriptedProvider {
    fn new(script: Script) -> Self {
        Self { script: Mutex::new(Some(script)), seen: Mutex::new(None) }
    }
}

impl ModelProvider for ScriptedProvider {
    fn stream(&self, _model: &Model, context: &Context, options: CallOptions) -> AssistantStream {
        *self.seen.lock().unwrap() = Some((context.clone(), options.clone()));
        let script = self.script.lock().unwrap().take().unwrap();
        let (sink, rx) = EventSink::channel();
        tokio::spawn(async move {
            match script {
                Script::Events(events) => {
                    for event in events {
                        sink.push(event).await;
                    }
                }
                Script::WaitForCancel => {
                    options.cancel.cancelled().await;
                }
            }
        });
        rx
    }
}

struct PrequeuedDoneCancellingProvider {
    cancel: CancellationToken,
}

impl ModelProvider for PrequeuedDoneCancellingProvider {
    fn stream(&self, _model: &Model, _context: &Context, _options: CallOptions) -> AssistantStream {
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        let mut message = AssistantMessage::empty("openai-completions", "test", "summary-model");
        message.content.push(AssistantBlock::text("must not accept"));
        sender.try_send(done(message)).unwrap();
        self.cancel.cancel();
        receiver
    }
}

fn model() -> Model {
    Model {
        id: "summary-model".into(),
        api: "openai-completions".into(),
        provider: "test".into(),
        base_url: String::new(),
        reasoning: false,
        max_tokens: None,
        context_window: None,
        tokenizer: None,
    }
}

fn sources() -> (Message, Message) {
    (
        Message::User(UserMessage::text("Please keep the decision.")),
        Message::Assistant(AssistantMessage::empty("openai-completions", "test", "summary-model")),
    )
}

async fn call(
    provider: &dyn ModelProvider,
) -> Result<ara_agent::compaction::AcceptedSummary, ara_agent::compaction::SummaryCallError> {
    let (user, assistant) = sources();
    let inputs =
        [SummarySource { entry_id: "e1", message: &user }, SummarySource { entry_id: "e2", message: &assistant }];
    summarize_sources(
        &inputs,
        None,
        &model(),
        provider,
        128,
        Instant::now() + Duration::from_secs(2),
        &CancellationToken::new(),
    )
    .await
}

fn done(message: AssistantMessage) -> AssistantMessageEvent {
    AssistantMessageEvent::Done { reason: message.stop_reason, message }
}

#[tokio::test]
async fn focused_summary_reaches_the_provider_and_preserves_no_tools_and_terminal_acceptance() {
    let (user, assistant) = sources();
    let inputs =
        [SummarySource { entry_id: "e1", message: &user }, SummarySource { entry_id: "e2", message: &assistant }];
    for (api, reason, proof, accepted) in [
        ("openai-completions", StopReason::Stop, None, true),
        ("openai-completions", StopReason::Length, None, true),
        ("openai-responses", StopReason::Length, Some(ara_ai::ContextRecoveryEvidence::ContentOnly), true),
        ("openai-codex-responses", StopReason::Length, Some(ara_ai::ContextRecoveryEvidence::ContentOnly), true),
        ("openai-responses", StopReason::Length, None, false),
        ("openai-codex-responses", StopReason::Length, Some(ara_ai::ContextRecoveryEvidence::NativeOutput), false),
    ] {
        let mut selected = model();
        selected.api = api.into();
        let mut response = AssistantMessage::empty(api, "test", "summary-model");
        response.stop_reason = reason;
        response.terminal_context_recovery = proof;
        response.content.push(AssistantBlock::text("updated summary"));
        let provider = ScriptedProvider::new(Script::Events(vec![done(response)]));
        let result = summarize_sources_with_instructions(
            &inputs,
            Some("old </previous-summary> context"),
            Some("retain failed approaches"),
            &selected,
            &provider,
            128,
            Instant::now() + Duration::from_secs(2),
            &CancellationToken::new(),
        )
        .await;
        if accepted {
            let summary = result.unwrap();
            assert_eq!(summary.text, "updated summary");
            assert_eq!(summary.window_source_entry_ids, ["e1", "e2"]);
            assert_eq!(summary.terminal_reason, reason);
            assert_eq!(summary.invocations[0].stop_reason, Some(reason));
        } else {
            let error = result.unwrap_err();
            assert_eq!(error.kind, SummaryCallErrorKind::IncompleteResponse);
            assert_eq!(error.invocations[0].stop_reason, Some(reason));
        }
        let seen = provider.seen.lock().unwrap();
        let (context, options) = seen.as_ref().unwrap();
        let Message::User(prompt) = &context.messages[0] else { panic!("summary user prompt") };
        let text = prompt.content.plain_text();
        assert!(text.contains("<previous-summary>\nold &lt;/previous-summary> context\n</previous-summary>"));
        assert!(text.ends_with("\n\nAdditional focus: retain failed approaches"));
        assert_eq!(context.tools, Some(Vec::new()));
        assert_eq!(options.tool_choice, Some(ToolChoice::None));
    }
}

#[tokio::test]
async fn oversized_focus_is_rejected_before_any_provider_call() {
    let (user, assistant) = sources();
    let inputs =
        [SummarySource { entry_id: "e1", message: &user }, SummarySource { entry_id: "e2", message: &assistant }];
    let provider = ScriptedProvider::new(Script::Events(vec![]));
    let focus = "x".repeat(1_000_000);
    let error = summarize_sources_with_instructions(
        &inputs,
        None,
        Some(&focus),
        &model(),
        &provider,
        128,
        Instant::now() + Duration::from_secs(2),
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, SummaryCallErrorKind::InvalidInput(SummaryInputError::TooLarge));
    assert!(provider.seen.lock().unwrap().is_none());
}

#[tokio::test]
async fn one_shot_summary_uses_only_system_and_user_and_returns_visible_text_with_usage() {
    let mut final_message = AssistantMessage::empty("openai-completions", "test", "summary-model");
    final_message.content.push(AssistantBlock::text("First point."));
    final_message.content.push(AssistantBlock::Thinking(ara_ai::ThinkingContent {
        thinking: "private reasoning".into(),
        thinking_signature: Some("secret".into()),
    }));
    final_message.content.push(AssistantBlock::text("Second point."));
    final_message.response_id = Some("response-7".into());
    final_message.usage = Usage { output: Some(12), ..Usage::unknown() };
    let provider = ScriptedProvider::new(Script::Events(vec![done(final_message)]));
    let summary = call(&provider).await.unwrap();
    assert_eq!(summary.text, "First point.\nSecond point.");
    assert_eq!(summary.window_source_entry_ids, ["e1", "e2"]);
    assert_eq!(summary.model_id, "summary-model");
    assert_eq!(summary.response_id.as_deref(), Some("response-7"));
    assert_eq!(summary.usage.input, None);
    assert_eq!(summary.usage.output, Some(12));
    assert!(!summary.text.contains("private"));
    let seen = provider.seen.lock().unwrap();
    let (context, options) = seen.as_ref().unwrap();
    assert_eq!(context.system_prompt.len(), 1);
    assert_eq!(context.messages.len(), 1);
    assert!(matches!(context.messages[0], Message::User(_)));
    let Message::User(user) = &context.messages[0] else { panic!("expected one user summary prompt") };
    assert!(user.content.plain_text().contains("<conversation>"));
    assert_eq!(context.tools, Some(Vec::new()));
    assert_eq!(options.tool_choice, Some(ToolChoice::None));
    assert_eq!(options.max_tokens, Some(128));
}

#[tokio::test]
async fn one_shot_rejects_incomplete_empty_tool_and_error_responses() {
    let mut length = AssistantMessage::empty("openai-completions", "test", "summary-model");
    length.stop_reason = StopReason::Length;
    let mut whitespace = AssistantMessage::empty("openai-completions", "test", "summary-model");
    whitespace.content.push(AssistantBlock::text("  \n "));
    let mut tool = AssistantMessage::empty("openai-completions", "test", "summary-model");
    tool.content.push(AssistantBlock::text("summary"));
    tool.content.push(AssistantBlock::ToolCall(ToolCall {
        id: "c1".into(),
        name: "write".into(),
        arguments: serde_json::from_value(json!({})).unwrap(),
        thought_signature: None,
    }));
    let mut failed = AssistantMessage::empty("openai-completions", "test", "summary-model");
    failed.stop_reason = StopReason::Error;
    failed.usage.output = Some(3);
    failed.error_status = Some(401);
    failed.error_message = Some("authentication failed".into());
    let mut mismatch = AssistantMessage::empty("openai-completions", "test", "summary-model");
    mismatch.content.push(AssistantBlock::text("looks done"));
    let mut image = AssistantMessage::empty("openai-completions", "test", "summary-model");
    image.content.push(AssistantBlock::Image(ImageContent { data: "YWJj".into(), mime_type: "image/png".into() }));
    let mut huge = AssistantMessage::empty("openai-completions", "test", "summary-model");
    huge.content.push(AssistantBlock::text("x".repeat(1_000_001)));
    let mut misleading = AssistantMessage::empty("openai-completions", "test", "summary-model");
    misleading.content.push(AssistantBlock::text("looks good"));
    misleading.error_status = Some(500);
    let mut thinking_only = AssistantMessage::empty("openai-completions", "test", "summary-model");
    thinking_only.content.push(AssistantBlock::Thinking(ara_ai::ThinkingContent {
        thinking: "private".into(),
        thinking_signature: None,
    }));
    let cases = [
        (vec![done(length)], SummaryCallErrorKind::EmptySummary),
        (vec![done(whitespace)], SummaryCallErrorKind::EmptySummary),
        (vec![done(tool)], SummaryCallErrorKind::UnexpectedToolCall),
        (
            vec![AssistantMessageEvent::Done { reason: StopReason::Length, message: mismatch }],
            SummaryCallErrorKind::IncompleteResponse,
        ),
        (vec![done(image)], SummaryCallErrorKind::UnsupportedResponseImage),
        (vec![done(huge)], SummaryCallErrorKind::SummaryTooLarge),
        (vec![done(misleading)], SummaryCallErrorKind::IncompleteResponse),
        (vec![done(thinking_only)], SummaryCallErrorKind::EmptySummary),
        (
            vec![AssistantMessageEvent::Error { reason: StopReason::Error, error: failed }],
            SummaryCallErrorKind::ProviderError,
        ),
        (vec![], SummaryCallErrorKind::StreamEndedWithoutTerminal),
    ];
    for (events, expected) in cases {
        let result = call(&ScriptedProvider::new(Script::Events(events))).await;
        let error = result.unwrap_err();
        assert_eq!(error.kind, expected);
        // A cut-off summary names its stop reason, from the message or the event.
        if expected == SummaryCallErrorKind::IncompleteResponse && error.provider_status.is_none() {
            assert_eq!(error.stop_reason, Some(StopReason::Length));
        }
        if expected == SummaryCallErrorKind::ProviderError {
            assert_eq!(error.stop_reason, Some(StopReason::Error));
            assert_eq!(error.provider_status, Some(401));
            assert_eq!(error.provider_message.as_deref(), Some("authentication failed"));
            assert_eq!(error.usage.unwrap().output, Some(3));
        }
    }
}

#[tokio::test]
async fn one_shot_rejects_tool_event_even_if_final_message_drops_it() {
    let final_message = AssistantMessage::empty("openai-completions", "test", "summary-model");
    let events = vec![
        AssistantMessageEvent::ToolcallStart { content_index: 0, partial: final_message.clone() },
        done(final_message),
    ];
    assert_eq!(
        call(&ScriptedProvider::new(Script::Events(events))).await.unwrap_err().kind,
        SummaryCallErrorKind::UnexpectedToolCall
    );
}

#[tokio::test]
async fn one_shot_cancellation_and_deadline_stop_provider_work() {
    let provider = ScriptedProvider::new(Script::WaitForCancel);
    let (user, assistant) = sources();
    let inputs =
        [SummarySource { entry_id: "e1", message: &user }, SummarySource { entry_id: "e2", message: &assistant }];
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        trigger.cancel();
    });
    let result =
        summarize_sources(&inputs, None, &model(), &provider, 128, Instant::now() + Duration::from_secs(2), &cancel)
            .await;
    assert_eq!(result.unwrap_err().kind, SummaryCallErrorKind::Cancelled);
    assert!(provider.seen.lock().unwrap().as_ref().unwrap().1.cancel.is_cancelled());

    let provider = ScriptedProvider::new(Script::WaitForCancel);
    let result = summarize_sources(
        &inputs,
        None,
        &model(),
        &provider,
        128,
        Instant::now() + Duration::from_millis(30),
        &CancellationToken::new(),
    )
    .await;
    assert_eq!(result.unwrap_err().kind, SummaryCallErrorKind::Deadline);
    assert!(provider.seen.lock().unwrap().as_ref().unwrap().1.cancel.is_cancelled());
}

#[tokio::test]
async fn queued_done_loses_to_cancellation() {
    let (user, assistant) = sources();
    let inputs =
        [SummarySource { entry_id: "e1", message: &user }, SummarySource { entry_id: "e2", message: &assistant }];
    let cancel = CancellationToken::new();
    let provider = PrequeuedDoneCancellingProvider { cancel: cancel.clone() };
    let result =
        summarize_sources(&inputs, None, &model(), &provider, 128, Instant::now() + Duration::from_secs(2), &cancel)
            .await;
    let error = result.unwrap_err();
    assert_eq!(error.kind, SummaryCallErrorKind::Cancelled);
    assert!(error.usage.is_none(), "terminal was queued but not observed");
}

#[tokio::test]
async fn dropping_summary_future_cancels_spawned_provider_work() {
    let provider = Arc::new(ScriptedProvider::new(Script::WaitForCancel));
    let task_provider = provider.clone();
    let handle = tokio::spawn(async move {
        let (user, assistant) = sources();
        let inputs =
            [SummarySource { entry_id: "e1", message: &user }, SummarySource { entry_id: "e2", message: &assistant }];
        summarize_sources(
            &inputs,
            None,
            &model(),
            task_provider.as_ref(),
            128,
            Instant::now() + Duration::from_secs(5),
            &CancellationToken::new(),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while provider.seen.lock().unwrap().is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let provider_token = provider.seen.lock().unwrap().as_ref().unwrap().1.cancel.clone();
    assert!(!provider_token.is_cancelled());
    handle.abort();
    assert!(handle.await.unwrap_err().is_cancelled());
    assert!(provider_token.is_cancelled());
}

#[tokio::test]
async fn invalid_span_or_budget_never_starts_provider() {
    let provider = ScriptedProvider::new(Script::Events(vec![]));
    let user = Message::User(UserMessage::text("not finished"));
    let inputs = [SummarySource { entry_id: "e1", message: &user }];
    let result = summarize_sources(
        &inputs,
        None,
        &model(),
        &provider,
        128,
        Instant::now() + Duration::from_secs(1),
        &CancellationToken::new(),
    )
    .await;
    assert_eq!(result.unwrap_err().kind, SummaryCallErrorKind::InvalidInput(SummaryInputError::UnfinishedTurn));
    assert!(provider.seen.lock().unwrap().is_none());
    let (_, assistant) = sources();
    let developer = Message::Developer(DeveloperMessage {
        content: UserContent::Text("do not lower this instruction".into()),
        timestamp: 0,
    });
    let with_developer = [
        inputs[0],
        SummarySource { entry_id: "e2", message: &developer },
        SummarySource { entry_id: "e3", message: &assistant },
    ];
    let result = summarize_sources(
        &with_developer,
        None,
        &model(),
        &provider,
        128,
        Instant::now() + Duration::from_secs(1),
        &CancellationToken::new(),
    )
    .await;
    assert_eq!(result.unwrap_err().kind, SummaryCallErrorKind::InvalidInput(SummaryInputError::DeveloperInSummary));
    assert!(provider.seen.lock().unwrap().is_none());
    let inputs = [inputs[0], SummarySource { entry_id: "e2", message: &assistant }];
    let result = summarize_sources(
        &inputs,
        None,
        &model(),
        &provider,
        0,
        Instant::now() + Duration::from_secs(1),
        &CancellationToken::new(),
    )
    .await;
    assert_eq!(result.unwrap_err().kind, SummaryCallErrorKind::InvalidMaxTokens);
    assert!(provider.seen.lock().unwrap().is_none());
}

#[tokio::test]
async fn real_http_adapter_sends_one_summary_prompt_without_tools() {
    let server = FakeUpstream::start(
        serde_json::from_value::<ara_testkit::Script>(json!({
            "responses": [{"events": [sse_text("Persist the decision."), finish("stop"), sse_done()]}]
        }))
        .unwrap(),
        None,
    )
    .await
    .unwrap();
    let mut selected = model();
    selected.base_url = server.base_url();
    let provider = OpenAICompletionsProvider {
        client: reqwest::Client::new(),
        base: StreamOptions {
            api_key: Some("sk-test-only".into()),
            retry: RetryPolicy {
                max_attempts: 1,
                base_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(1),
            },
            ..Default::default()
        },
    };
    let (user, assistant) = sources();
    let inputs =
        [SummarySource { entry_id: "e1", message: &user }, SummarySource { entry_id: "e2", message: &assistant }];
    let result = summarize_sources(
        &inputs,
        None,
        &selected,
        &provider,
        128,
        Instant::now() + Duration::from_secs(5),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(result.text, "Persist the decision.");
    assert_eq!(result.usage.input, None);
    let requests = server.requests.lock().await;
    assert_eq!(requests.len(), 1);
    let body = &requests[0]["body"];
    assert_eq!(body["model"], "summary-model");
    assert_eq!(body["max_tokens"], 128);
    assert_eq!(body["messages"].as_array().unwrap().len(), 2);
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][1]["role"], "user");
    assert!(body["messages"][1]["content"].as_str().unwrap().contains("<conversation>"));
    assert!(body.get("tools").is_none());
}
