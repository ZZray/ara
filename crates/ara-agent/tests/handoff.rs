//! Concentrated fixed OMP handoff/file-operation input families. Scripted
//! ModelProvider receipts prove Core contracts, not a real provider/cache hit.
use ara_agent::compaction::SummaryCallErrorKind;
use ara_agent::handoff::{
    HandoffFileDetails, generate_handoff_from_context, prepare_handoff_summary, render_handoff_prompt,
};
use ara_ai::{
    AssistantBlock, AssistantMessage, AssistantMessageEvent, AssistantStream, CallOptions, Context, EventSink,
    ImageContent, Message, Model, ModelProvider, StopReason, ThinkingContent, Tool, ToolCall, ToolChoice, Usage,
    UserBlock, UserContent, UserMessage,
};
use serde_json::json;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

enum Script {
    Events(Vec<AssistantMessageEvent>),
    WaitForCancel,
    QueuedThenCancel(Box<AssistantMessage>, CancellationToken),
}

struct ScriptedProvider {
    scripts: Mutex<VecDeque<Script>>,
    seen: Mutex<Vec<(Context, CallOptions)>>,
}

impl ScriptedProvider {
    fn new(scripts: Vec<Script>) -> Self {
        Self { scripts: Mutex::new(scripts.into()), seen: Mutex::new(Vec::new()) }
    }
}

impl ModelProvider for ScriptedProvider {
    fn stream(&self, _: &Model, context: &Context, options: CallOptions) -> AssistantStream {
        self.seen.lock().unwrap().push((context.clone(), options.clone()));
        let script = self.scripts.lock().unwrap().pop_front().expect("only planned native calls may run");
        if let Script::QueuedThenCancel(message, cancel) = script {
            let (sender, receiver) = tokio::sync::mpsc::channel(1);
            sender.try_send(terminal(*message)).unwrap();
            cancel.cancel();
            return receiver;
        }
        let (sink, stream) = EventSink::channel();
        tokio::spawn(async move {
            match script {
                Script::Events(events) => {
                    for event in events {
                        sink.push(event).await;
                    }
                }
                Script::WaitForCancel => options.cancel.cancelled().await,
                Script::QueuedThenCancel(..) => unreachable!(),
            }
        });
        stream
    }
}

fn model() -> Model {
    Model {
        id: "handoff-model".into(),
        api: "openai-completions".into(),
        provider: "fixture".into(),
        base_url: "https://fixture.invalid/v1".into(),
        reasoning: true,
        max_tokens: None,
        context_window: Some(200_000.0),
        tokenizer: None,
    }
}

fn response(text: &str) -> AssistantMessage {
    let mut message = AssistantMessage::empty("openai-completions", "fixture", "handoff-model");
    if !text.is_empty() {
        message.content.push(AssistantBlock::text(text));
    }
    message.response_id = Some("actual-handoff-receipt".into());
    message.duration = Some(17);
    message.ttft = Some(3);
    message
}

fn tool(name: &str, path: &str) -> AssistantBlock {
    AssistantBlock::ToolCall(ToolCall {
        id: format!("{name}:{path}"),
        name: name.into(),
        arguments: json!({"path":path}).as_object().unwrap().clone(),
        thought_signature: None,
    })
}

fn terminal(message: AssistantMessage) -> AssistantMessageEvent {
    match message.stop_reason {
        StopReason::Error | StopReason::Aborted => {
            AssistantMessageEvent::Error { reason: message.stop_reason, error: message }
        }
        reason => AssistantMessageEvent::Done { reason, message },
    }
}

fn live_context() -> Context {
    Context {
        system_prompt: vec!["base system prompt".into(), "stable shared cache prefix".into()],
        messages: vec![
            Message::User(UserMessage {
                content: UserContent::Blocks(vec![
                    UserBlock::Image(ImageContent {
                        detail: None,
                        compaction_frame: false,
                        data: "original-image".into(),
                        mime_type: "image/png".into(),
                    }),
                    UserBlock::text("original user task"),
                ]),
                synthetic: None,
                timestamp: 7,
            }),
            Message::Assistant(response("settled work")),
            Message::User(UserMessage::text(render_handoff_prompt(Some("Preserve exact commands.")))),
        ],
        tools: Some(vec![Tool {
            name: "read".into(),
            description: "live tool schema".into(),
            parameters: json!({"type":"object","properties":{"path":{"type":"string"}}}),
        }]),
    }
}

#[tokio::test]
async fn native_live_context_and_terminal_text_extraction_family() {
    for case in ["stop", "length", "tool-use", "non-text", "empty"] {
        let context = live_context();
        let before = context.clone();
        let mut message = response("");
        if case != "empty" {
            message.content.push(AssistantBlock::Thinking(ThinkingContent {
                thinking: "private reasoning must not enter the document".into(),
                thinking_signature: None,
            }));
            message.content.push(tool("read", "never-executed.rs"));
            message.content.push(AssistantBlock::Image(ImageContent {
                detail: None,
                compaction_frame: false,
                data: "never-returned".into(),
                mime_type: "image/png".into(),
            }));
        }
        if !matches!(case, "empty" | "non-text") {
            message.content.insert(0, AssistantBlock::text("first"));
            message.content.push(AssistantBlock::text("last"));
        }
        message.stop_reason = match case {
            "length" => StopReason::Length,
            "tool-use" => StopReason::ToolUse,
            _ => StopReason::Stop,
        };
        let expected_usage = message.usage.clone();
        let events = if case == "tool-use" {
            vec![AssistantMessageEvent::ToolcallStart { content_index: 1, partial: message.clone() }, terminal(message)]
        } else {
            vec![terminal(message)]
        };
        let provider = ScriptedProvider::new(vec![Script::Events(events)]);
        let cancel = CancellationToken::new();
        let result = generate_handoff_from_context(
            &context,
            &model(),
            &provider,
            Some(321),
            Instant::now() + Duration::from_secs(2),
            &cancel,
        )
        .await
        .unwrap();
        assert_eq!(context, before, "{case}: live Context is unchanged");
        assert_eq!(result.text, if matches!(case, "empty" | "non-text") { "" } else { "first\nlast" }, "{case}");
        assert_eq!(
            result.terminal_reason,
            match case {
                "length" => StopReason::Length,
                "tool-use" => StopReason::ToolUse,
                _ => StopReason::Stop,
            }
        );
        assert_eq!(result.usage, expected_usage);
        assert!(result.usage.input.is_none(), "unknown is not fabricated zero");
        assert!(result.window_source_entry_ids.is_empty(), "Host assigns provenance IDs");
        assert_eq!(result.response_id.as_deref(), Some("actual-handoff-receipt"));
        assert_eq!((result.duration_ms, result.ttft_ms), (Some(17), Some(3)));
        assert_eq!(result.invocations.len(), 1);
        let seen = provider.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, before);
        assert_eq!(seen[0].1.tool_choice, Some(ToolChoice::None));
        assert_eq!(seen[0].1.max_tokens, Some(321));
        assert!(seen[0].1.temperature.is_none());
        assert!(seen[0].1.loop_guard.is_none());
        assert!(seen[0].1.cancel.is_cancelled(), "settled invocation releases its provider binding");
        assert!(!cancel.is_cancelled());
    }
}

#[tokio::test]
async fn native_auto_only_retry_and_failure_receipts_family() {
    let long_diagnostic = format!("{} tool_choice only auto supported", "detail ".repeat(100));
    for (status, diagnostic, calls) in [
        (Some(400), "tool_choice 'none' is not supported; use auto", 2),
        (Some(400), "TOOL_CHOICE AUTO SUPPORTED", 2),
        (Some(400), "tool_choice unsupported; auto", 1),
        (Some(400), "tool_choice supported; automatic", 1),
        (Some(400), "my_tool_choice auto supported", 1),
        (Some(500), "tool_choice auto supported", 1),
        (None, "tool_choice auto supported", 1),
        (Some(400), long_diagnostic.as_str(), 2),
    ] {
        let mut error = response("error text must not become a document");
        error.stop_reason = StopReason::Error;
        error.error_status = status;
        error.error_message = Some(diagnostic.into());
        let mut success = response("continued document");
        success.usage = Usage { output: Some(9), ..Usage::unknown() };
        let provider =
            ScriptedProvider::new(vec![Script::Events(vec![terminal(error)]), Script::Events(vec![terminal(success)])]);
        let context = live_context();
        let result = generate_handoff_from_context(
            &context,
            &model(),
            &provider,
            None,
            Instant::now() + Duration::from_secs(2),
            &CancellationToken::new(),
        )
        .await;
        if calls == 2 {
            let accepted = result.unwrap();
            assert_eq!(accepted.text, "continued document");
            assert_eq!(accepted.usage.output, Some(9));
            assert_eq!(accepted.invocations.len(), 2);
            assert_eq!(accepted.invocations[0].provider_status, status);
            assert_eq!(accepted.invocations[0].error_kind, Some(SummaryCallErrorKind::ProviderError));
            assert!(accepted.invocations[0].oneshot_retry_eligible);
            assert!(accepted.invocations[0].usage.as_ref().unwrap().input.is_none());
            assert!(accepted.invocations[1].error_kind.is_none());
        } else {
            let failure = result.unwrap_err();
            assert_eq!(failure.kind, SummaryCallErrorKind::ProviderError);
            assert_eq!(failure.provider_status, status);
            assert_eq!(failure.invocations.len(), 1);
            assert!(failure.provider_message.as_deref().unwrap().starts_with("Handoff generation failed:"));
        }
        let seen = provider.seen.lock().unwrap();
        assert_eq!(seen.len(), calls);
        assert!(seen.iter().all(|(seen_context, options)| *seen_context == context && options.max_tokens.is_none()));
        assert_eq!(seen[0].1.tool_choice, Some(ToolChoice::None));
        if calls == 2 {
            assert_eq!(seen[1].1.tool_choice, Some(ToolChoice::Auto));
        }
    }
    let mut error = response("");
    error.stop_reason = StopReason::Error;
    error.error_status = Some(400);
    error.error_message = Some("tool_choice auto supported".into());
    let provider = ScriptedProvider::new(vec![
        Script::Events(vec![terminal(error.clone())]),
        Script::Events(vec![terminal(error)]),
    ]);
    let failure = generate_handoff_from_context(
        &live_context(),
        &model(),
        &provider,
        None,
        Instant::now() + Duration::from_secs(2),
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(failure.invocations.len(), 2, "an auto-only rejection on Auto cannot loop");
    assert_eq!(provider.seen.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn native_cancel_deadline_and_future_drop_family() {
    for case in ["pre-cancel", "pre-deadline", "cancel", "deadline", "queued-cancel", "provider-abort"] {
        let cancel = CancellationToken::new();
        let mut deadline = Instant::now() + Duration::from_secs(2);
        let mut aborted = response("must not accept cancellation text");
        aborted.stop_reason = StopReason::Aborted;
        let script = match case {
            "queued-cancel" => Script::QueuedThenCancel(Box::new(response("queued")), cancel.clone()),
            "provider-abort" => Script::Events(vec![terminal(aborted)]),
            _ => Script::WaitForCancel,
        };
        let provider = ScriptedProvider::new(vec![script]);
        if case == "pre-cancel" {
            cancel.cancel();
        }
        if case == "pre-deadline" {
            deadline = Instant::now();
        }
        if case == "deadline" {
            deadline = Instant::now() + Duration::from_millis(20);
        }
        if case == "cancel" {
            let trigger = cancel.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(20)).await;
                trigger.cancel();
            });
        }
        let failure = generate_handoff_from_context(&live_context(), &model(), &provider, None, deadline, &cancel)
            .await
            .unwrap_err();
        assert_eq!(
            failure.kind,
            if matches!(case, "pre-deadline" | "deadline") {
                SummaryCallErrorKind::Deadline
            } else {
                SummaryCallErrorKind::Cancelled
            },
            "{case}"
        );
        let seen = provider.seen.lock().unwrap();
        assert_eq!(seen.len(), usize::from(!case.starts_with("pre-")), "{case}");
        assert_eq!(failure.invocations.len(), seen.len(), "actual requests only");
        if let Some((_, options)) = seen.first() {
            assert!(options.cancel.is_cancelled());
        }
        if case == "queued-cancel" {
            assert!(failure.usage.is_none(), "queued is not observed terminal usage");
        }
    }
    let provider = Arc::new(ScriptedProvider::new(vec![Script::WaitForCancel]));
    let task_provider = provider.clone();
    let handle = tokio::spawn(async move {
        generate_handoff_from_context(
            &live_context(),
            &model(),
            task_provider.as_ref(),
            None,
            Instant::now() + Duration::from_secs(2),
            &CancellationToken::new(),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while provider.seen.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    handle.abort();
    assert!(handle.await.unwrap_err().is_cancelled());
    assert!(
        provider.seen.lock().unwrap()[0].1.cancel.is_cancelled(),
        "dropping the future stops spawned provider work"
    );
}

#[test]
fn native_prompt_and_cumulative_file_operation_family() {
    let base_prompt = render_handoff_prompt(None);
    assert_eq!(base_prompt, render_handoff_prompt(Some("")));
    assert!(base_prompt.starts_with("<critical>\nWrite a handoff document for another instance of yourself."));
    assert!(base_prompt.ends_with("</output>"));
    assert!(!base_prompt.contains("Additional focus:"));
    let focused = render_handoff_prompt(Some("Keep {{literal}} and a -> b."));
    assert!(focused.ends_with("<instruction>\nAdditional focus: Keep {{literal}} and a -> b.\n</instruction>"));

    let mut history = response("");
    for path in [
        "docs/compaction.md:100-170:raw",
        "docs/compaction.md:8-16,128-139,384-388",
        "docs/compaction.md:raw",
        "docs/compaction.md:conflicts",
    ] {
        history.content.push(tool("read", path));
    }
    for (name, path) in [
        ("read", "src/a.ts:30-80"),
        ("write", "src/a.ts"),
        ("read", "src/b.ts"),
        ("edit", "src/c.ts"),
        ("write", "src/d.ts"),
        ("read", "archive.zip:dir/file.ts:50-60"),
        ("read", "artifact://7"),
        ("read", "local://ctx.md"),
        ("read", "https://example.com/page"),
        ("write", "conflict://1"),
        ("write", "src/login.ts:conflict://3"),
        ("bash", "not-a-native-file-op"),
    ] {
        history.content.push(tool(name, path));
    }
    let previous = HandoffFileDetails {
        read_files: vec!["docs/old.md:2-5".into(), "artifact://old".into()],
        modified_files: vec!["src/previous.rs".into(), "conflict://old".into()],
    };
    let context = Context { messages: vec![Message::Assistant(history)], ..Context::default() };
    let document = "## Goal\nContinue.\n\n<files>stale</files>\n<read-files>old</read-files>\n<modified-files>older</modified-files>\u{feff}";
    let prepared = prepare_handoff_summary(document, &context, Some(&previous));
    assert_eq!(prepared.read_files, ["archive.zip:dir/file.ts", "docs/compaction.md", "docs/old.md", "src/b.ts"]);
    assert_eq!(prepared.modified_files, ["src/a.ts", "src/c.ts", "src/d.ts", "src/previous.rs"]);
    assert_eq!(
        prepared.summary,
        [
            "## Goal",
            "Continue.",
            "",
            "<files>",
            "# archive.zip:dir/",
            "file.ts (Read)",
            "# docs/",
            "compaction.md (Read)",
            "old.md (Read)",
            "# src/",
            "a.ts (RW)",
            "b.ts (Read)",
            "c.ts (Write)",
            "d.ts (Write)",
            "previous.rs (Write)",
            "</files>"
        ]
        .join("\n")
    );

    let mut many = response("");
    for index in 0..22 {
        many.content.push(tool("read", &format!("files/f{index:02}.rs")));
    }
    let prepared =
        prepare_handoff_summary("", &Context { messages: vec![Message::Assistant(many)], ..Context::default() }, None);
    assert_eq!(prepared.read_files.len(), 22, "details keep the complete cumulative set");
    assert!(prepared.summary.ends_with("f19.rs (Read)\n[…2 files elided…]\n</files>"));
    assert!(!prepared.summary.contains("f20.rs"));

    let mut paths = response("");
    for path in [
        "src/a.rs:50",
        "src/a.rs:50-",
        "src/a.rs:50+150",
        "src/a.rs:2724..2727",
        "src/a.rs:raw:2-4",
        "db.sqlite:users",
        "😀.rs",
        "\u{e000}.rs",
    ] {
        paths.content.push(tool("read", path));
    }
    let prepared =
        prepare_handoff_summary("", &Context { messages: vec![Message::Assistant(paths)], ..Context::default() }, None);
    assert_eq!(
        prepared.read_files,
        ["db.sqlite:users", "src/a.rs", "😀.rs", "\u{e000}.rs"],
        "native UTF-16 sorting and selector grammar"
    );
    let empty = prepare_handoff_summary("untouched\u{feff}", &Context::default(), None);
    assert_eq!(empty.summary, "untouched");
}
