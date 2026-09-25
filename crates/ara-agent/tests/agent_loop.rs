//! Agent loop behavior tests (OMP `packages/agent/test/agent-loop.test.ts` themes)
//! with a scripted in-process provider, plus one real HTTP chain case.

use ara_agent::*;
use ara_ai::event::EventSink;
use ara_ai::*;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------- fixtures

#[derive(Clone)]
enum Turn {
    /// Emit a complete assistant turn.
    Reply { text: String, calls: Vec<(String, String, Value)>, stop: StopReason },
    /// Emit text and one tool call (with toolcall_end) plus one unfinished call, then wait for cancel.
    HangAfterPartial,
    /// Emit a provider error after one completed tool call.
    ErrorAfterToolCall(String),
}

struct ScriptedProvider {
    turns: Mutex<Vec<Turn>>,
    contexts: Mutex<Vec<Context>>,
}

impl ScriptedProvider {
    fn new(turns: Vec<Turn>) -> Arc<Self> {
        Arc::new(ScriptedProvider { turns: Mutex::new(turns), contexts: Mutex::new(Vec::new()) })
    }
}

fn reply(text: &str, calls: &[(&str, &str, Value)], stop: StopReason) -> Turn {
    Turn::Reply {
        text: text.into(),
        calls: calls.iter().map(|(i, n, a)| (i.to_string(), n.to_string(), a.clone())).collect(),
        stop,
    }
}

fn call_block(id: &str, name: &str, args: &Value) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        arguments: args.as_object().cloned().unwrap_or_default(),
        thought_signature: None,
    }
}

impl ModelProvider for ScriptedProvider {
    fn stream(&self, model: &Model, context: &Context, options: CallOptions) -> AssistantStream {
        self.contexts.lock().unwrap().push(context.clone());
        let turn = {
            let mut t = self.turns.lock().unwrap();
            if t.is_empty() { reply("(script exhausted)", &[], StopReason::Stop) } else { t.remove(0) }
        };
        let (sink, rx) = EventSink::channel();
        let mut msg = AssistantMessage::empty(&model.api, &model.provider, &model.id);
        tokio::spawn(async move {
            sink.push(AssistantMessageEvent::Start { partial: msg.clone() }).await;
            match turn {
                Turn::Reply { text, calls, stop } => {
                    if !text.is_empty() {
                        msg.content.push(AssistantBlock::text(""));
                        let i = msg.content.len() - 1;
                        sink.push(AssistantMessageEvent::TextStart { content_index: i, partial: msg.clone() }).await;
                        msg.content[i] = AssistantBlock::text(text.clone());
                        sink.push(AssistantMessageEvent::TextDelta {
                            content_index: i,
                            delta: text.clone(),
                            partial: msg.clone(),
                        })
                        .await;
                        sink.push(AssistantMessageEvent::TextEnd {
                            content_index: i,
                            content: text,
                            partial: msg.clone(),
                        })
                        .await;
                    }
                    for (id, name, args) in calls {
                        let tc = call_block(&id, &name, &args);
                        msg.content.push(AssistantBlock::ToolCall(tc.clone()));
                        let i = msg.content.len() - 1;
                        sink.push(AssistantMessageEvent::ToolcallStart { content_index: i, partial: msg.clone() })
                            .await;
                        sink.push(AssistantMessageEvent::ToolcallEnd {
                            content_index: i,
                            tool_call: tc,
                            partial: msg.clone(),
                        })
                        .await;
                    }
                    msg.stop_reason = stop;
                    sink.push(AssistantMessageEvent::Done { reason: stop, message: msg }).await;
                }
                Turn::HangAfterPartial => {
                    msg.content.push(AssistantBlock::text("thinking out loud"));
                    sink.push(AssistantMessageEvent::TextDelta {
                        content_index: 0,
                        delta: "thinking out loud".into(),
                        partial: msg.clone(),
                    })
                    .await;
                    let done = call_block("done-1", "echo", &json!({"text": "a"}));
                    msg.content.push(AssistantBlock::ToolCall(done.clone()));
                    sink.push(AssistantMessageEvent::ToolcallEnd {
                        content_index: 1,
                        tool_call: done,
                        partial: msg.clone(),
                    })
                    .await;
                    msg.content.push(AssistantBlock::ToolCall(call_block("partial-2", "echo", &json!({"te": null}))));
                    sink.push(AssistantMessageEvent::ToolcallStart { content_index: 2, partial: msg.clone() }).await;
                    options.cancel.cancelled().await;
                    msg.stop_reason = StopReason::Aborted;
                    msg.error_message = Some("Request was aborted".into());
                    sink.push(AssistantMessageEvent::Error { reason: StopReason::Aborted, error: msg }).await;
                }
                Turn::ErrorAfterToolCall(err) => {
                    let done = call_block("c-err", "echo", &json!({"text": "x"}));
                    msg.content.push(AssistantBlock::ToolCall(done.clone()));
                    sink.push(AssistantMessageEvent::ToolcallEnd {
                        content_index: 0,
                        tool_call: done,
                        partial: msg.clone(),
                    })
                    .await;
                    msg.stop_reason = StopReason::Error;
                    msg.error_message = Some(err);
                    sink.push(AssistantMessageEvent::Error { reason: StopReason::Error, error: msg }).await;
                }
            }
        });
        rx
    }
}

struct EchoTool {
    delay: Duration,
    concurrency: Concurrency,
    log: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl AgentTool for EchoTool {
    fn definition(&self) -> Tool {
        Tool {
            name: "echo".into(),
            description: "Echo text".into(),
            parameters: json!({"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"], "additionalProperties": false}),
        }
    }
    fn concurrency(&self, _args: &JsonObject) -> Concurrency {
        self.concurrency
    }
    async fn execute(
        &self,
        id: &str,
        args: JsonObject,
        cancel: CancellationToken,
        update: UpdateFn,
    ) -> Result<ToolOutput, ToolError> {
        let text = args["text"].as_str().unwrap_or_default().to_string();
        self.log.lock().unwrap().push(format!("start:{id}"));
        update(ToolOutput::text(format!("partial {text}")));
        tokio::select! {
            _ = tokio::time::sleep(self.delay) => {}
            _ = cancel.cancelled() => {
                self.log.lock().unwrap().push(format!("cancelled:{id}"));
                return Err(ToolError("echo cancelled".into()));
            }
        }
        self.log.lock().unwrap().push(format!("end:{id}"));
        if text == "fail" {
            return Ok(ToolOutput { content: vec![], details: None, is_error: true });
        }
        Ok(ToolOutput::text(format!("echo: {text}")).with_details(json!({"len": text.len()})))
    }
}

#[derive(Default)]
struct Hooks {
    block: bool,
    wait_for_cancel: bool,
    slow_dequeue_ms: u64,
    steering: Mutex<Vec<Message>>,
    follow_up: Mutex<Vec<Message>>,
}

#[async_trait]
impl LoopHooks for Hooks {
    async fn before_tool_call(&self, _call: &ToolCall, _args: &JsonObject, cancel: &CancellationToken) -> ToolDecision {
        if self.wait_for_cancel {
            cancel.cancelled().await;
            return ToolDecision::Allow;
        }
        if self.block { ToolDecision::Block(Some("denied by host policy".into())) } else { ToolDecision::Allow }
    }
    async fn steering_messages(&self) -> Vec<Message> {
        let taken = std::mem::take(&mut *self.steering.lock().unwrap());
        if !taken.is_empty() && self.slow_dequeue_ms > 0 {
            tokio::time::sleep(Duration::from_millis(self.slow_dequeue_ms)).await;
        }
        taken
    }
    async fn follow_up_messages(&self) -> Vec<Message> {
        std::mem::take(&mut *self.follow_up.lock().unwrap())
    }
}

fn model() -> Model {
    Model {
        id: "scripted".into(),
        api: "openai-completions".into(),
        provider: "test".into(),
        base_url: String::new(),
        reasoning: false,
        max_tokens: None,
    }
}

fn config(provider: Arc<dyn ModelProvider>, tools: Vec<Arc<dyn AgentTool>>, hooks: Arc<dyn LoopHooks>) -> AgentConfig {
    AgentConfig {
        model: model(),
        provider,
        system_prompt: vec!["sys".into()],
        tools,
        tool_choice: None,
        max_tokens: None,
        temperature: None,
        deadline: None,
        hooks,
    }
}

fn echo(delay_ms: u64, concurrency: Concurrency) -> (Arc<dyn AgentTool>, Arc<Mutex<Vec<String>>>) {
    let log = Arc::new(Mutex::new(Vec::new()));
    (Arc::new(EchoTool { delay: Duration::from_millis(delay_ms), concurrency, log: log.clone() }), log)
}

async fn types(sink: &RecordingSink) -> Vec<String> {
    sink.events
        .lock()
        .await
        .iter()
        .map(|e| {
            let role = match e {
                AgentEvent::MessageStart { message } | AgentEvent::MessageEnd { message } => {
                    format!(":{}", message.role())
                }
                _ => String::new(),
            };
            format!("{}{role}", e.type_name())
        })
        .filter(|t| t != "message_update")
        .collect()
}

fn result_text(m: &Message) -> String {
    match m {
        Message::ToolResult(r) => r
            .content
            .iter()
            .map(|b| match b {
                UserBlock::Text(t) => t.text.clone(),
                _ => String::new(),
            })
            .collect(),
        _ => panic!("not a tool result: {m:?}"),
    }
}

fn user(text: &str) -> Message {
    Message::User(UserMessage::text(text))
}

// ------------------------------------------------------------------- tests

#[tokio::test]
async fn simple_prompt_event_sequence() {
    let provider = ScriptedProvider::new(vec![reply("Hello!", &[], StopReason::Stop)]);
    let sink = RecordingSink::default();
    let mut ctx = Vec::new();
    let new = agent_loop(
        vec![user("hi")],
        &mut ctx,
        &config(provider.clone(), vec![], Arc::new(NoHooks)),
        &CancellationToken::new(),
        &sink,
    )
    .await;
    assert_eq!(
        types(&sink).await,
        vec![
            "agent_start",
            "turn_start",
            "message_start:user",
            "message_end:user",
            "message_start:assistant",
            "message_end:assistant",
            "turn_end",
            "agent_end",
        ]
    );
    assert_eq!(new.len(), 2);
    assert_eq!(ctx.len(), 2);
    assert_eq!(provider.contexts.lock().unwrap()[0].system_prompt, vec!["sys".to_string()]);
    let updates = sink.events.lock().await.iter().filter(|e| matches!(e, AgentEvent::MessageUpdate { .. })).count();
    assert_eq!(updates, 3, "text_start/delta/end streamed as message_update");
}

#[tokio::test]
async fn tool_call_turn_executes_and_continues() {
    let provider = ScriptedProvider::new(vec![
        reply("", &[("c1", "echo", json!({"text": "one"}))], StopReason::ToolUse),
        reply("done", &[], StopReason::Stop),
    ]);
    let (tool, log) = echo(1, Concurrency::Shared);
    let sink = RecordingSink::default();
    let mut ctx = Vec::new();
    let new = agent_loop(
        vec![user("go")],
        &mut ctx,
        &config(provider.clone(), vec![tool], Arc::new(NoHooks)),
        &CancellationToken::new(),
        &sink,
    )
    .await;
    assert_eq!(
        types(&sink).await,
        vec![
            "agent_start",
            "turn_start",
            "message_start:user",
            "message_end:user",
            "message_start:assistant",
            "message_end:assistant",
            "tool_execution_start",
            "tool_execution_update",
            "tool_execution_end",
            "message_start:toolResult",
            "message_end:toolResult",
            "turn_end",
            "turn_start",
            "message_start:assistant",
            "message_end:assistant",
            "turn_end",
            "agent_end",
        ]
    );
    assert_eq!(new.iter().map(Message::role).collect::<Vec<_>>(), vec!["user", "assistant", "toolResult", "assistant"]);
    assert_eq!(result_text(&new[2]), "echo: one");
    assert_eq!(*log.lock().unwrap(), vec!["start:c1", "end:c1"]);
    // Second model call sees the tool result.
    let second = &provider.contexts.lock().unwrap()[1];
    assert_eq!(second.messages.last().unwrap().role(), "toolResult");
}

#[tokio::test]
async fn validation_unknown_tool_blocked_and_empty_error() {
    let provider = ScriptedProvider::new(vec![
        reply(
            "",
            &[
                ("bad", "echo", json!({"txt": "x"})),
                ("missing", "nope", json!({})),
                ("fail", "echo", json!({"text": "fail"})),
            ],
            StopReason::ToolUse,
        ),
        reply("ok", &[], StopReason::Stop),
    ]);
    let (tool, log) = echo(1, Concurrency::Shared);
    let mut ctx = Vec::new();
    let new = agent_loop(
        vec![user("go")],
        &mut ctx,
        &config(provider, vec![tool], Arc::new(NoHooks)),
        &CancellationToken::new(),
        &NullSink,
    )
    .await;
    let results: Vec<&Message> = new.iter().filter(|m| m.role() == "toolResult").collect();
    let text_of = |id: &str| {
        results
            .iter()
            .find(|m| matches!(m, Message::ToolResult(r) if r.tool_call_id == id))
            .map(|m| result_text(m))
            .unwrap()
    };
    assert!(text_of("bad").starts_with("Validation failed for tool \"echo\":\n  - text: is required"));
    assert_eq!(text_of("missing"), "Tool nope not found");
    assert_eq!(text_of("fail"), "Tool failed with no output.");
    assert_eq!(*log.lock().unwrap(), vec!["start:fail", "end:fail"], "invalid calls never execute");

    let provider = ScriptedProvider::new(vec![
        reply("", &[("c", "echo", json!({"text": "x"}))], StopReason::ToolUse),
        reply("ok", &[], StopReason::Stop),
    ]);
    let (tool, log) = echo(1, Concurrency::Shared);
    let hooks = Arc::new(Hooks { block: true, ..Default::default() });
    let new = agent_loop(
        vec![user("go")],
        &mut Vec::new(),
        &config(provider, vec![tool], hooks),
        &CancellationToken::new(),
        &NullSink,
    )
    .await;
    assert_eq!(result_text(&new[2]), "denied by host policy");
    assert!(log.lock().unwrap().is_empty(), "blocked call has no effect");
}

#[tokio::test]
async fn provider_error_pairs_completed_calls_with_synthetic_results() {
    let provider = ScriptedProvider::new(vec![Turn::ErrorAfterToolCall("503 overloaded".into())]);
    let (tool, log) = echo(1, Concurrency::Shared);
    let sink = RecordingSink::default();
    let new = agent_loop(
        vec![user("go")],
        &mut Vec::new(),
        &config(provider, vec![tool], Arc::new(NoHooks)),
        &CancellationToken::new(),
        &sink,
    )
    .await;
    assert_eq!(new.len(), 3);
    assert_eq!(new[1].as_assistant().unwrap().stop_reason, StopReason::Error);
    assert_eq!(
        result_text(&new[2]),
        "Tool call was not executed because the provider stream ended with an error before the tool could run: 503 overloaded"
    );
    match &new[2] {
        Message::ToolResult(r) => assert_eq!(r.details.as_ref().unwrap()["source"], json!("assistant_stop_error")),
        _ => unreachable!(),
    }
    assert!(log.lock().unwrap().is_empty());
    assert_eq!(types(&sink).await.last().unwrap(), "agent_end");
}

#[tokio::test]
async fn abort_during_stream_keeps_only_completed_calls() {
    let provider = ScriptedProvider::new(vec![Turn::HangAfterPartial]);
    let (tool, log) = echo(1, Concurrency::Shared);
    let cancel = CancellationToken::new();
    let sink = Arc::new(RecordingSink::default());
    let (c2, s2) = (cancel.clone(), sink.clone());
    tokio::spawn(async move {
        // Cancel once the partial is observed live.
        loop {
            if s2.events.lock().await.iter().any(|e| {
                matches!(e, AgentEvent::MessageUpdate { event: AssistantMessageEvent::ToolcallStart { .. }, .. })
            }) {
                c2.cancel();
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });
    let mut ctx = Vec::new();
    let new = agent_loop(
        vec![user("go")],
        &mut ctx,
        &config(provider, vec![tool], Arc::new(NoHooks)),
        &cancel,
        sink.as_ref(),
    )
    .await;
    let assistant = new[1].as_assistant().unwrap();
    assert_eq!(assistant.stop_reason, StopReason::Aborted);
    assert_eq!(assistant.error_message.as_deref(), Some("Request was aborted"));
    let ids: Vec<&str> = assistant.tool_calls().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, vec!["done-1"], "unfinished tool call dropped");
    assert_eq!(assistant.stop_details.as_ref().unwrap()["type"], json!("stream_interrupted_after_content"));
    assert_eq!(result_text(&new[2]), "Tool execution was aborted: Request was aborted");
    assert!(log.lock().unwrap().is_empty(), "aborted turn never executes tools");
    assert_eq!(ctx.len(), 3);
}

#[tokio::test]
async fn abort_during_tool_execution_stops_before_next_model_call() {
    let provider = ScriptedProvider::new(vec![
        reply("", &[("slow", "echo", json!({"text": "x"}))], StopReason::ToolUse),
        reply("never", &[], StopReason::Stop),
    ]);
    let (tool, log) = echo(5_000, Concurrency::Shared);
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        c2.cancel();
    });
    let started = Instant::now();
    let new = agent_loop(
        vec![user("go")],
        &mut Vec::new(),
        &config(provider.clone(), vec![tool], Arc::new(NoHooks)),
        &cancel,
        &NullSink,
    )
    .await;
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(*log.lock().unwrap(), vec!["start:slow", "cancelled:slow"]);
    assert_eq!(result_text(&new[2]), "echo cancelled");
    let last = new.last().unwrap().as_assistant().unwrap();
    assert_eq!(last.stop_reason, StopReason::Aborted);
    assert_eq!(provider.contexts.lock().unwrap().len(), 1, "no second provider call after abort");
}

#[tokio::test]
async fn length_stop_pairs_calls_and_resamples() {
    let provider = ScriptedProvider::new(vec![
        reply("", &[("big", "echo", json!({"text": "x"}))], StopReason::Length),
        reply("smaller now", &[], StopReason::Stop),
    ]);
    let (tool, log) = echo(1, Concurrency::Shared);
    let new = agent_loop(
        vec![user("go")],
        &mut Vec::new(),
        &config(provider.clone(), vec![tool], Arc::new(NoHooks)),
        &CancellationToken::new(),
        &NullSink,
    )
    .await;
    assert!(
        result_text(&new[2]).starts_with("Tool call was not executed because the assistant hit its output token limit")
    );
    assert!(log.lock().unwrap().is_empty());
    assert_eq!(new.last().unwrap().as_assistant().unwrap().text(), "smaller now");
    assert_eq!(provider.contexts.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn shared_calls_overlap_and_exclusive_serializes() {
    let provider = ScriptedProvider::new(vec![
        reply("", &[("a", "echo", json!({"text": "a"})), ("b", "echo", json!({"text": "b"}))], StopReason::ToolUse),
        reply("", &[("c", "echo", json!({"text": "c"})), ("d", "echo", json!({"text": "d"}))], StopReason::ToolUse),
        reply("end", &[], StopReason::Stop),
    ]);
    let (shared, _) = echo(300, Concurrency::Shared);
    let started = Instant::now();
    agent_loop(
        vec![user("go")],
        &mut Vec::new(),
        &config(provider, vec![shared], Arc::new(NoHooks)),
        &CancellationToken::new(),
        &NullSink,
    )
    .await;
    let shared_elapsed = started.elapsed();
    assert!(shared_elapsed < Duration::from_millis(1000), "two batches of overlapping 300ms calls: {shared_elapsed:?}");

    let provider = ScriptedProvider::new(vec![
        reply("", &[("a", "echo", json!({"text": "a"})), ("b", "echo", json!({"text": "b"}))], StopReason::ToolUse),
        reply("end", &[], StopReason::Stop),
    ]);
    let (exclusive, log) = echo(200, Concurrency::Exclusive);
    let started = Instant::now();
    agent_loop(
        vec![user("go")],
        &mut Vec::new(),
        &config(provider, vec![exclusive], Arc::new(NoHooks)),
        &CancellationToken::new(),
        &NullSink,
    )
    .await;
    assert!(started.elapsed() >= Duration::from_millis(400));
    assert_eq!(*log.lock().unwrap(), vec!["start:a", "end:a", "start:b", "end:b"]);
}

#[tokio::test]
async fn deadline_cancels_in_flight_stream() {
    let provider = ScriptedProvider::new(vec![Turn::HangAfterPartial]);
    let (tool, log) = echo(1, Concurrency::Shared);
    let mut cfg = config(provider, vec![tool], Arc::new(NoHooks));
    cfg.deadline = Some(Instant::now() + Duration::from_millis(150));
    let started = Instant::now();
    let new = agent_loop(vec![user("go")], &mut Vec::new(), &cfg, &CancellationToken::new(), &NullSink).await;
    assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    let a = new[1].as_assistant().unwrap();
    assert_eq!(a.stop_reason, StopReason::Aborted);
    assert_eq!(a.error_message.as_deref(), Some("Deadline exceeded"));
    assert_eq!(result_text(&new[2]), "Tool execution was aborted: Deadline exceeded");
    assert!(log.lock().unwrap().is_empty());
}

#[tokio::test]
async fn deadline_cancels_running_tool() {
    let provider = ScriptedProvider::new(vec![
        reply("", &[("slow", "echo", json!({"text": "x"}))], StopReason::ToolUse),
        reply("never", &[], StopReason::Stop),
    ]);
    let (tool, log) = echo(10_000, Concurrency::Shared);
    let mut cfg = config(provider.clone(), vec![tool], Arc::new(NoHooks));
    cfg.deadline = Some(Instant::now() + Duration::from_millis(150));
    let started = Instant::now();
    let new = agent_loop(vec![user("go")], &mut Vec::new(), &cfg, &CancellationToken::new(), &NullSink).await;
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(*log.lock().unwrap(), vec!["start:slow", "cancelled:slow"]);
    assert_eq!(result_text(&new[2]), "echo cancelled");
    assert_eq!(provider.contexts.lock().unwrap().len(), 1, "no model call after the deadline");
}

#[tokio::test]
async fn dequeued_steering_survives_deadline() {
    let provider = ScriptedProvider::new(vec![
        reply("", &[("c1", "echo", json!({"text": "x"}))], StopReason::ToolUse),
        reply("never", &[], StopReason::Stop),
    ]);
    let (tool, _) = echo(60, Concurrency::Shared);
    let hooks = Arc::new(Hooks { slow_dequeue_ms: 300, ..Default::default() });
    let mut cfg = config(provider, vec![tool], hooks.clone());
    cfg.deadline = Some(Instant::now() + Duration::from_millis(150));
    // First dequeue (run start) is empty; the steer arrives mid-run and its dequeue outlives the deadline.
    let h2 = hooks.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        h2.steering.lock().unwrap().push(user("late steer"));
    });
    let new = agent_loop(vec![user("go")], &mut Vec::new(), &cfg, &CancellationToken::new(), &NullSink).await;
    assert!(new.iter().any(|m| matches!(m, Message::User(u) if u.content.plain_text() == "late steer")), "{new:?}");
}

#[tokio::test]
async fn panicking_tool_becomes_error_result() {
    struct Boom;
    #[async_trait]
    impl AgentTool for Boom {
        fn definition(&self) -> Tool {
            Tool { name: "boom".into(), description: String::new(), parameters: json!({"type": "object"}) }
        }
        async fn execute(
            &self,
            _: &str,
            _: JsonObject,
            _: CancellationToken,
            _: UpdateFn,
        ) -> Result<ToolOutput, ToolError> {
            panic!("kaboom");
        }
    }
    let provider = ScriptedProvider::new(vec![
        reply("", &[("p", "boom", json!({})), ("e", "echo", json!({"text": "fine"}))], StopReason::ToolUse),
        reply("ok", &[], StopReason::Stop),
    ]);
    let (echo_tool, log) = echo(1, Concurrency::Shared);
    let new = agent_loop(
        vec![user("go")],
        &mut Vec::new(),
        &config(provider, vec![Arc::new(Boom), echo_tool], Arc::new(NoHooks)),
        &CancellationToken::new(),
        &NullSink,
    )
    .await;
    let texts: Vec<String> = new.iter().filter(|m| m.role() == "toolResult").map(result_text).collect();
    assert!(texts.contains(&"Tool boom panicked: kaboom".to_string()), "{texts:?}");
    assert!(texts.contains(&"echo: fine".to_string()));
    assert_eq!(*log.lock().unwrap(), vec!["start:e", "end:e"]);
    assert_eq!(new.last().unwrap().as_assistant().unwrap().text(), "ok");
}

#[tokio::test]
async fn dropping_the_run_cancels_running_tools() {
    let provider =
        ScriptedProvider::new(vec![reply("", &[("slow", "echo", json!({"text": "x"}))], StopReason::ToolUse)]);
    let (tool, log) = echo(10_000, Concurrency::Shared);
    let cfg = config(provider, vec![tool], Arc::new(NoHooks));
    let mut ctx = Vec::new();
    let cancel = CancellationToken::new();
    let r = tokio::time::timeout(
        Duration::from_millis(150),
        agent_loop(vec![user("go")], &mut ctx, &cfg, &cancel, &NullSink),
    )
    .await;
    assert!(r.is_err(), "run was dropped by the host timeout");
    assert!(!cancel.is_cancelled(), "caller token untouched");
    // The tool's child token fires via the drop guard; the tool future itself was dropped with the run.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(*log.lock().unwrap(), vec!["start:slow"]);
}

#[tokio::test]
async fn approval_hook_observes_abort() {
    let provider = ScriptedProvider::new(vec![reply(
        "",
        &[("a", "echo", json!({"text": "x"})), ("b", "echo", json!({"text": "y"}))],
        StopReason::ToolUse,
    )]);
    let (tool, log) = echo(1, Concurrency::Shared);
    let hooks = Arc::new(Hooks { wait_for_cancel: true, ..Default::default() });
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        c2.cancel();
    });
    let started = Instant::now();
    let new =
        agent_loop(vec![user("go")], &mut Vec::new(), &config(provider, vec![tool], hooks), &cancel, &NullSink).await;
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(log.lock().unwrap().is_empty());
    let texts: Vec<String> = new.iter().filter(|m| m.role() == "toolResult").map(result_text).collect();
    assert_eq!(texts, vec!["Tool was not executed because the run was aborted: Request was aborted."; 2]);
}

#[tokio::test]
async fn parse_error_args_are_trimmed_in_events() {
    let bad = ToolCall {
        id: "w".into(),
        name: "echo".into(),
        arguments: ara_ai::json::parse_final_arguments(&format!("{{\"text\": \"{}", "x".repeat(10_000))),
        thought_signature: None,
    };
    let mut msg = AssistantMessage::empty("a", "p", "m");
    msg.stop_reason = StopReason::ToolUse;
    msg.content.push(AssistantBlock::ToolCall(bad));
    let (tool, _) = echo(1, Concurrency::Shared);
    let cfg = config(ScriptedProvider::new(vec![]), vec![tool], Arc::new(NoHooks));
    let sink = RecordingSink::default();
    let results = execute_tool_calls(&msg, &cfg, &CancellationToken::new(), &sink).await;
    assert!(results[0].is_error);
    let events = sink.events.lock().await;
    let AgentEvent::ToolExecutionStart { args, .. } = &events[0] else { panic!() };
    assert_eq!(args.keys().collect::<Vec<_>>(), vec!["__parseError"]);
}

#[test]
fn retain_completed_falls_back_to_error_message_for_null_explanation() {
    let mut m = AssistantMessage::empty("a", "p", "m");
    m.stop_reason = StopReason::Error;
    m.error_message = Some("socket closed".into());
    m.stop_details = Some(json!({"type": "x", "explanation": null}));
    m.content.push(AssistantBlock::ToolCall(call_block("dropped", "echo", &json!({}))));
    let out = ara_agent::agent_loop::retain_completed_tool_calls(m, &Default::default());
    assert_eq!(
        out.stop_details.unwrap(),
        json!({"type": "stream_interrupted_after_content", "category": "x", "explanation": "socket closed"})
    );
    assert!(out.content.is_empty());
}

#[tokio::test]
async fn steering_and_follow_up_messages() {
    let provider = ScriptedProvider::new(vec![
        reply("", &[("c1", "echo", json!({"text": "x"}))], StopReason::ToolUse),
        reply("after steer", &[], StopReason::Stop),
        reply("after follow-up", &[], StopReason::Stop),
    ]);
    let (tool, _) = echo(1, Concurrency::Shared);
    let hooks = Arc::new(Hooks { follow_up: Mutex::new(vec![user("follow up")]), ..Default::default() });
    let cfg = config(provider.clone(), vec![tool], hooks.clone());
    // Steering queued before the run starts is injected in the first turn.
    hooks.steering.lock().unwrap().push(user("steer"));
    let new = agent_loop(vec![user("go")], &mut Vec::new(), &cfg, &CancellationToken::new(), &NullSink).await;
    let roles: Vec<String> = new
        .iter()
        .map(|m| match m {
            Message::User(u) => format!("user:{}", u.content.plain_text()),
            o => o.role().into(),
        })
        .collect();
    assert_eq!(
        roles,
        vec!["user:go", "user:steer", "assistant", "toolResult", "assistant", "user:follow up", "assistant"]
    );
    assert_eq!(provider.contexts.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn continue_rules_and_unpaired_tail_resume() {
    let provider = ScriptedProvider::new(vec![reply("resumed", &[], StopReason::Stop)]);
    let (tool, log) = echo(1, Concurrency::Shared);
    let cfg = config(provider, vec![tool], Arc::new(NoHooks));
    let err = agent_loop_continue(&mut Vec::new(), &cfg, &CancellationToken::new(), &NullSink, UnpairedTail::Execute)
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "Cannot continue: no messages in context");
    let mut done = AssistantMessage::empty("a", "p", "m");
    done.content.push(AssistantBlock::text("fin"));
    let err = agent_loop_continue(
        &mut vec![user("q"), Message::Assistant(done)],
        &cfg,
        &CancellationToken::new(),
        &NullSink,
        UnpairedTail::Execute,
    )
    .await
    .unwrap_err();
    assert_eq!(err.to_string(), "Cannot continue from message role: assistant");

    let mut tail = AssistantMessage::empty("a", "p", "m");
    tail.stop_reason = StopReason::ToolUse;
    tail.content.push(AssistantBlock::ToolCall(call_block("again", "echo", &json!({"text": "r"}))));
    let mut ctx = vec![user("q"), Message::Assistant(tail)];
    let err = agent_loop_continue(&mut ctx.clone(), &cfg, &CancellationToken::new(), &NullSink, UnpairedTail::Refuse)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("effects are unknown"), "{err}");
    assert!(log.lock().unwrap().is_empty(), "refused tail never executes");
    let new =
        agent_loop_continue(&mut ctx, &cfg, &CancellationToken::new(), &NullSink, UnpairedTail::Execute).await.unwrap();
    assert_eq!(
        *log.lock().unwrap(),
        vec!["start:again", "end:again"],
        "explicit continue re-executes the unpaired tail"
    );
    assert_eq!(new.iter().map(Message::role).collect::<Vec<_>>(), vec!["toolResult", "assistant"]);
}

#[tokio::test]
async fn real_http_chain_with_fake_upstream() {
    use ara_testkit::chunks::*;
    use ara_testkit::{FakeUpstream, Script};
    let script: Script = serde_json::from_value(json!({"responses": [
        {"events": [tool_call(0, "call_1", "echo", "{\"text\":\"wire\"}"), finish("tool_calls"), usage(20, 5), done()]},
        {"events": [text("all done"), finish("stop"), usage(30, 2), done()]}
    ]}))
    .unwrap();
    let server = FakeUpstream::start(script, None).await.unwrap();
    let provider = Arc::new(OpenAICompletionsProvider { client: reqwest::Client::new(), base: Default::default() });
    let (tool, log) = echo(1, Concurrency::Shared);
    let mut cfg = config(provider, vec![tool], Arc::new(NoHooks));
    cfg.model.base_url = server.base_url();
    let new = agent_loop(vec![user("use echo")], &mut Vec::new(), &cfg, &CancellationToken::new(), &NullSink).await;
    assert_eq!(*log.lock().unwrap(), vec!["start:call_1", "end:call_1"]);
    assert_eq!(new.last().unwrap().as_assistant().unwrap().text(), "all done");
    let reqs = server.requests.lock().await;
    let msgs = reqs[1]["body"]["messages"].as_array().unwrap();
    assert_eq!(msgs[2]["tool_calls"][0]["id"], json!("call_1"));
    assert_eq!(msgs[3], json!({"role": "tool", "content": "echo: wire", "tool_call_id": "call_1"}));
}
