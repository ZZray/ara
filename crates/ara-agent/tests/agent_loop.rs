//! Agent loop behavior tests (OMP `packages/agent/test/agent-loop.test.ts` themes)
//! with a scripted in-process provider, plus one real HTTP chain case.

use ara_agent::compaction::{SummarySource, build_summary_prompt};
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
        ..Default::default()
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

struct LegacySchemaTool {
    effects: Arc<Mutex<Vec<Value>>>,
}

struct WireSchemaTool {
    effects: Arc<Mutex<Vec<Value>>>,
}

struct StrictEditTool {
    effects: Arc<Mutex<Vec<Value>>>,
}

struct RawCustomTool {
    logical_name: &'static str,
    alias: Option<&'static str>,
    effects: Arc<Mutex<Vec<Value>>>,
}

#[async_trait]
impl AgentTool for RawCustomTool {
    fn definition(&self) -> Tool {
        Tool {
            name: self.logical_name.into(),
            description: "Record a raw custom invocation".into(),
            parameters: json!({"type":"object","properties":{"input":{"type":"string","minLength":1}},
                "required":["input"],"additionalProperties":false}),
            ..Default::default()
        }
    }
    fn custom_format(&self) -> Option<CustomToolFormat> {
        self.alias.map(|_| CustomToolFormat { syntax: CustomToolSyntax::Lark, definition: "start: /.+/".into() })
    }
    fn custom_wire_name(&self) -> Option<String> {
        self.alias.map(str::to_owned)
    }
    async fn execute(
        &self,
        id: &str,
        args: JsonObject,
        _cancel: CancellationToken,
        _update: UpdateFn,
    ) -> Result<ToolOutput, ToolError> {
        self.effects.lock().unwrap().push(json!({"logical":self.logical_name,"id":id,"args":args}));
        Ok(ToolOutput::text(format!("effect:{}", self.logical_name)))
    }
}

#[derive(Default)]
struct CustomApproval {
    block: bool,
    seen: Mutex<Vec<Value>>,
}

#[async_trait]
impl LoopHooks for CustomApproval {
    async fn before_tool_call(&self, call: &ToolCall, args: &JsonObject, _cancel: &CancellationToken) -> ToolDecision {
        self.seen.lock().unwrap().push(json!({"name":call.name,"customWireName":call.custom_wire_name,"args":args}));
        if self.block { ToolDecision::Block(Some("denied by host policy".into())) } else { ToolDecision::Allow }
    }
}

#[tokio::test]
async fn conditional_freeform_dispatch_schema_approval_and_internal_name_group() {
    use ara_testkit::{FakeUpstream, Script};
    for (invalid, blocked, collision) in
        [(false, false, false), (true, false, false), (false, true, false), (false, false, true)]
    {
        let raw = if invalid { "" } else { "*** Begin Patch\n+raw { not JSON } 中文\n*** End Patch" };
        let script:Script=serde_json::from_value(json!({"responses":[
            {"events":[
                {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"custom_tool_call","id":"ctc_dispatch","call_id":"call_dispatch","name":"apply_patch","input":raw}}},
                {"data":{"type":"response.completed","response":{"status":"completed"}}}
            ]},
            {"events":[
                {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"reviewed"}]}}},
                {"data":{"type":"response.completed","response":{"status":"completed"}}}
            ]}
        ]})).unwrap();
        let endpoint = FakeUpstream::start(script, None).await.unwrap();
        let mut opts = ara_ai::providers::openai_responses::StreamOptions::default();
        opts.request.apply_patch_tool_type = Some("freeform".into());
        opts.retry.max_attempts = 1;
        let provider = Arc::new(OpenAIResponsesProvider { client: reqwest::Client::new(), base: opts });
        let effects = Arc::new(Mutex::new(Vec::new()));
        let mut tools: Vec<Arc<dyn AgentTool>> = vec![Arc::new(RawCustomTool {
            logical_name: "edit",
            alias: Some("apply_patch"),
            effects: effects.clone(),
        })];
        if collision {
            tools.push(Arc::new(RawCustomTool { logical_name: "apply_patch", alias: None, effects: effects.clone() }));
        }
        let hooks = Arc::new(CustomApproval { block: blocked, ..Default::default() });
        let mut cfg = config(provider, tools, hooks.clone());
        cfg.model.api = "openai-responses".into();
        cfg.model.base_url = endpoint.base_url();
        let sink = Arc::new(RecordingSink::default());
        let messages = Agent::new(cfg, Vec::new())
            .prompt(vec![user("apply the raw input")], CancellationToken::new(), sink.clone())
            .await
            .unwrap()
            .model_messages()
            .unwrap();
        let results = messages
            .iter()
            .filter_map(|message| match message {
                Message::ToolResult(result) => Some(result),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tool_call_id, "call_dispatch|ctc_dispatch");
        assert_eq!(results[0].tool_name, "apply_patch", "received wire name stays on receipts");
        assert_eq!(results[0].is_error, invalid || blocked);
        assert_eq!(messages.last().unwrap().as_assistant().unwrap().text(), "reviewed");
        {
            let approved = hooks.seen.lock().unwrap();
            assert_eq!(approved.len(), usize::from(!invalid), "schema validation precedes approval");
            if !invalid {
                assert_eq!(
                    approved[0],
                    json!({"name":"apply_patch","customWireName":"apply_patch","args":{"input":raw}})
                );
            }
            let effects = effects.lock().unwrap();
            if invalid || blocked {
                assert!(effects.is_empty());
            } else {
                assert_eq!(
                    *effects,
                    vec![
                        json!({"logical":if collision {"apply_patch"} else {"edit"},"id":"call_dispatch|ctc_dispatch","args":{"input":raw}})
                    ]
                );
            }
        }
        let events = sink.events.lock().await;
        assert!(events.iter().any(|event| matches!(event,AgentEvent::ToolExecutionEnd {tool_name,is_error,..} if tool_name=="apply_patch" && *is_error==(invalid||blocked))));
        let recorded = endpoint.requests.lock().await;
        assert_eq!(recorded.len(), 2);
        assert_eq!(
            recorded[0]["body"]["tools"][0]["type"], "custom",
            "tool-owned optional metadata reaches protocol projection"
        );
        let input = recorded[1]["body"]["input"].as_array().unwrap();
        assert!(input.iter().any(|item| item["type"] == "custom_tool_call"
            && item["call_id"] == "call_dispatch"
            && item["input"] == raw));
        assert!(
            input.iter().any(|item| item["type"] == "custom_tool_call_output" && item["call_id"] == "call_dispatch")
        );
    }
}

#[async_trait]
impl AgentTool for StrictEditTool {
    fn definition(&self) -> Tool {
        Tool {
            name: "edit".into(),
            description: "Record a test edit".into(),
            parameters: json!({"type":"object","properties":{"path":{"type":"string"}},
                "required":["path"],"additionalProperties":false}),
            ..Default::default()
        }
    }

    async fn execute(
        &self,
        _id: &str,
        args: JsonObject,
        _cancel: CancellationToken,
        _update: UpdateFn,
    ) -> Result<ToolOutput, ToolError> {
        self.effects.lock().unwrap().push(Value::Object(args));
        Ok(ToolOutput::text("edited"))
    }
}

#[async_trait]
impl AgentTool for WireSchemaTool {
    fn definition(&self) -> Tool {
        Tool {
            name: "schema_wire".into(),
            description: "Checks Responses schema wire against execution validation".into(),
            parameters: json!({"type":"object","properties":{
                "skip":{"anyOf":[{"type":"integer","minimum":0},{"type":"null"}]},
                "mode":{"anyOf":[{"const":"a","description":"mode"},{"const":"b","description":"mode"}]},
                "flag":{"enum":[true,false]}
            },"required":["skip","mode","flag"]}),
            ..Default::default()
        }
    }

    fn concurrency(&self, _args: &JsonObject) -> Concurrency {
        Concurrency::Shared
    }

    async fn execute(
        &self,
        _id: &str,
        args: JsonObject,
        _cancel: CancellationToken,
        _update: UpdateFn,
    ) -> Result<ToolOutput, ToolError> {
        self.effects.lock().unwrap().push(Value::Object(args));
        Ok(ToolOutput::text("wire tool executed"))
    }
}

#[async_trait]
impl AgentTool for LegacySchemaTool {
    fn definition(&self) -> Tool {
        Tool {
            name: "legacy_schema".into(),
            description: "Checks legacy JSON Schema arguments".into(),
            parameters: json!({
                "type":"object",
                "properties":{
                    "item":{"$ref":"#/definitions/Item"},
                    "pair":{"type":"array","items":[{"type":"string"},{"type":"integer"}],"additionalItems":false}
                },
                "required":["item","pair"],
                "definitions":{"Item":{"type":"string"}}
            }),
            ..Default::default()
        }
    }

    fn concurrency(&self, _args: &JsonObject) -> Concurrency {
        Concurrency::Shared
    }

    async fn execute(
        &self,
        _id: &str,
        args: JsonObject,
        _cancel: CancellationToken,
        _update: UpdateFn,
    ) -> Result<ToolOutput, ToolError> {
        self.effects.lock().unwrap().push(Value::Object(args));
        Ok(ToolOutput::text("legacy tool executed"))
    }
}

#[async_trait]
impl AgentTool for EchoTool {
    fn definition(&self) -> Tool {
        Tool {
            name: "echo".into(),
            description: "Echo text".into(),
            parameters: json!({"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"], "additionalProperties": false}),
            ..Default::default()
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
            return Ok(ToolOutput {
                content: vec![],
                details: None,
                is_error: true,
                provider_metadata: None,
                useless: false,
            });
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
        context_window: None,
        tokenizer: None,
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
        max_model_calls: None,
        hooks,
        interrupt_mode: ara_agent::InterruptMode::Immediate,
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
                    format!(":{}", message.json().unwrap()["role"].as_str().unwrap())
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

fn user_texts(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|m| match m {
            Message::User(u) => Some(u.content.plain_text()),
            _ => None,
        })
        .collect()
}

// ------------------------------------------------------------------- tests

fn owned_input(model: &Message, origin: &str) -> AgentInput {
    AgentInput { model: model.clone(), provenance: Some(Arc::new(json!({"private-origin": origin}))) }
}

fn assert_input_identity(actual: &[AgentInput], expected: &[AgentInput]) {
    assert_eq!(actual, expected);
    for (actual, expected) in actual.iter().zip(expected) {
        match (&actual.provenance, &expected.provenance) {
            (Some(actual), Some(expected)) => assert!(Arc::ptr_eq(actual, expected)),
            (None, None) => {}
            _ => panic!("input provenance changed"),
        }
    }
}

#[derive(Default)]
struct InputSink {
    inputs: Mutex<Vec<AgentInput>>,
    events: RecordingSink,
}

#[async_trait]
impl AgentEventSink for InputSink {
    async fn emit(&self, event: AgentEvent) {
        if let AgentEvent::MessageStart { message } = &event
            && let Some(model @ (Message::User(_) | Message::Developer(_))) = message.checked_model_snapshot().unwrap()
        {
            self.inputs.lock().unwrap().push(AgentInput { model, provenance: message.source() });
        }
        self.events.emit(event).await;
    }

    async fn emit_input(&self, input: AgentInput) {
        self.inputs.lock().unwrap().push(input.clone());
        self.emit(AgentEvent::MessageStart { message: input.model.clone().into() }).await;
        self.emit(AgentEvent::MessageEnd { message: input.model.into() }).await;
    }
}

#[derive(Default)]
struct InputHooks {
    steering: Mutex<Vec<AgentInput>>,
    follow_up: Mutex<Vec<AgentInput>>,
}

#[async_trait]
impl LoopHooks for InputHooks {
    async fn steering_inputs(&self) -> Vec<AgentInput> {
        std::mem::take(&mut *self.steering.lock().unwrap())
    }

    async fn follow_up_inputs(&self) -> Vec<AgentInput> {
        std::mem::take(&mut *self.follow_up.lock().unwrap())
    }
}

enum AdmissionAction {
    Allow,
    Block,
    Cancel,
    CancelOnReturn,
    Wait(Arc<tokio::sync::Notify>),
    WaitThenAllow(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>),
}

async fn admission_action(action: AdmissionAction, cancel: &CancellationToken) -> Result<(), AdmissionError> {
    match action {
        AdmissionAction::Allow => Ok(()),
        AdmissionAction::Block => Err(AdmissionError::Blocked("503 admission unavailable".into())),
        AdmissionAction::Cancel => Err(AdmissionError::Cancelled),
        AdmissionAction::CancelOnReturn => {
            cancel.cancel();
            Ok(())
        }
        AdmissionAction::Wait(entered) => {
            entered.notify_one();
            std::future::pending().await
        }
        AdmissionAction::WaitThenAllow(entered, release) => {
            entered.notify_one();
            release.notified().await;
            Ok(())
        }
    }
}

#[derive(Default)]
struct AdmissionHooks {
    queues: InputHooks,
    queued_admission: Mutex<std::collections::VecDeque<AdmissionAction>>,
    model_admission: Mutex<std::collections::VecDeque<AdmissionAction>>,
    dequeues: Mutex<[usize; 2]>,
    model_admissions: Mutex<usize>,
}

#[async_trait]
impl LoopHooks for AdmissionHooks {
    async fn before_queued_input_dequeue(&self, cancel: &CancellationToken) -> Result<(), AdmissionError> {
        let action = self.queued_admission.lock().unwrap().pop_front().unwrap_or(AdmissionAction::Allow);
        admission_action(action, cancel).await
    }

    async fn before_model_call(
        &self,
        snapshot: ExecutionSnapshot,
        cancel: &CancellationToken,
    ) -> Result<ExecutionSnapshot, AdmissionError> {
        *self.model_admissions.lock().unwrap() += 1;
        let action = self.model_admission.lock().unwrap().pop_front().unwrap_or(AdmissionAction::Allow);
        admission_action(action, cancel).await?;
        Ok(snapshot)
    }

    async fn steering_inputs(&self) -> Vec<AgentInput> {
        self.dequeues.lock().unwrap()[0] += 1;
        self.queues.steering_inputs().await
    }

    async fn follow_up_inputs(&self) -> Vec<AgentInput> {
        self.dequeues.lock().unwrap()[1] += 1;
        self.queues.follow_up_inputs().await
    }
}

struct InputDequeueGate {
    follow_up: bool,
    inputs: Mutex<Vec<AgentInput>>,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

impl InputDequeueGate {
    async fn take(&self) -> Vec<AgentInput> {
        let inputs = std::mem::take(&mut *self.inputs.lock().unwrap());
        if !inputs.is_empty() {
            self.entered.notify_one();
            self.release.notified().await;
        }
        inputs
    }
}

#[async_trait]
impl LoopHooks for InputDequeueGate {
    async fn steering_inputs(&self) -> Vec<AgentInput> {
        if self.follow_up { Vec::new() } else { self.take().await }
    }

    async fn follow_up_inputs(&self) -> Vec<AgentInput> {
        if self.follow_up { self.take().await } else { Vec::new() }
    }
}

#[tokio::test]
async fn owned_inputs_keep_distinct_provenance_for_equal_model_messages_in_all_queue_modes() {
    let model = Message::User(UserMessage {
        content: UserContent::Text("identical input".into()),
        synthetic: None,
        timestamp: 42,
        provider_payload: None,
        provider_payload_origin: None,
    });
    for (steering_mode, follow_up_mode, users_per_call) in [
        (QueueMode::OneAtATime, QueueMode::OneAtATime, vec![2, 3, 4, 5, 6, 7]),
        (QueueMode::All, QueueMode::All, vec![4, 7]),
        (QueueMode::All, QueueMode::OneAtATime, vec![4, 5, 6, 7]),
        (QueueMode::OneAtATime, QueueMode::All, vec![2, 3, 4, 7]),
    ] {
        let provider = ScriptedProvider::new(vec![]);
        let agent = Agent::new(config(provider.clone(), vec![], Arc::new(NoHooks)), Vec::new());
        agent.set_steering_mode(steering_mode);
        agent.set_follow_up_mode(follow_up_mode);
        let initial = owned_input(&model, "initial");
        let steer_a = owned_input(&model, "steer-a");
        let steer_b = owned_input(&model, "steer-b");
        let follow_a = owned_input(&model, "follow-a");
        let follow_b = owned_input(&model, "follow-b");
        agent.steer_input(steer_a.clone());
        agent.steer_input(steer_b.clone());
        agent.steer(model.clone());
        agent.follow_up_input(follow_a.clone());
        agent.follow_up_input(follow_b.clone());
        agent.follow_up(model.clone());
        assert_input_identity(&agent.peek_steering_inputs(), &[steer_a.clone(), steer_b.clone(), model.clone().into()]);
        assert_input_identity(
            &agent.peek_follow_up_inputs(),
            &[follow_a.clone(), follow_b.clone(), model.clone().into()],
        );
        assert_eq!(agent.peek_steering_queue().unwrap(), vec![model.clone(); 3]);
        assert_eq!(agent.peek_follow_up_queue().unwrap(), vec![model.clone(); 3]);
        let sink = Arc::new(InputSink::default());
        let report = agent.prompt_inputs(vec![initial.clone()], CancellationToken::new(), sink.clone()).await.unwrap();
        let expected = [initial, steer_a, steer_b, model.clone().into(), follow_a, follow_b, model.clone().into()];
        assert_input_identity(&sink.inputs.lock().unwrap(), &expected);
        assert_eq!(report.end, RunEnd::Completed);
        let report_users: Vec<_> = report
            .model_messages()
            .unwrap()
            .into_iter()
            .filter(|message| matches!(message, Message::User(_)))
            .collect();
        assert_eq!(report_users, vec![model.clone(); 7]);
        assert_eq!(agent.messages().await.unwrap(), report.model_messages().unwrap());
        assert!(!serde_json::to_string(&report.model_messages().unwrap()).unwrap().contains("private-origin"));
        {
            let contexts = provider.contexts.lock().unwrap();
            assert_eq!(
                contexts.iter().map(|context| user_texts(&context.messages).len()).collect::<Vec<_>>(),
                users_per_call
            );
            for context in contexts.iter() {
                assert!(!serde_json::to_string(&context.messages).unwrap().contains("private-origin"));
            }
        }
        assert!(!agent.has_queued_messages());
        let events = sink.events.events.lock().await;
        let inputs: Vec<_> = events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    AgentEvent::MessageStart { message } | AgentEvent::MessageEnd { message } if message.json().unwrap()["role"] == "user"
                )
            })
            .collect();
        assert_eq!(inputs.len(), 14);
        for (pair, input) in inputs.as_chunks::<2>().0.iter().zip(&expected) {
            let raw: AgentMessage = input.clone().into();
            assert!(matches!((pair[0], pair[1]),
                (AgentEvent::MessageStart { message: start }, AgentEvent::MessageEnd { message: end })
                    if ara_agent::checked_record_equal(start, &raw).unwrap()
                        && ara_agent::checked_record_equal(end, &raw).unwrap()));
            let (AgentEvent::MessageStart { message: start }, AgentEvent::MessageEnd { message: end }) =
                (pair[0], pair[1])
            else {
                panic!("input start/end order changed");
            };
            assert!(start.same_record(end));
            if let Some(source) = &input.provenance {
                assert!(Arc::ptr_eq(&start.source().unwrap(), source));
            }
        }
    }
}

#[tokio::test]
async fn owned_native_queues_precede_owned_host_hooks_and_legacy_hooks_adapt() {
    let model = Message::User(UserMessage {
        content: UserContent::Text("same".into()),
        synthetic: None,
        timestamp: 42,
        provider_payload: None,
        provider_payload_origin: None,
    });
    let initial = owned_input(&model, "initial");
    let native_steer = owned_input(&model, "native-steer");
    let host_steer = owned_input(&model, "host-steer");
    let native_follow = owned_input(&model, "native-follow");
    let host_follow = owned_input(&model, "host-follow");
    let hooks = Arc::new(InputHooks::default());
    hooks.steering.lock().unwrap().push(host_steer.clone());
    hooks.follow_up.lock().unwrap().push(host_follow.clone());
    let provider = ScriptedProvider::new(vec![]);
    let agent = Agent::new(config(provider.clone(), vec![], hooks), Vec::new());
    agent.steer_input(native_steer.clone());
    agent.follow_up_input(native_follow.clone());
    let sink = Arc::new(InputSink::default());
    agent.prompt_inputs(vec![initial.clone()], CancellationToken::new(), sink.clone()).await.unwrap();
    assert_input_identity(
        &sink.inputs.lock().unwrap(),
        &[initial, native_steer, host_steer, native_follow, host_follow],
    );
    assert_eq!(
        provider.contexts.lock().unwrap().iter().map(|ctx| user_texts(&ctx.messages).len()).collect::<Vec<_>>(),
        [3, 5]
    );

    let legacy = Arc::new(Hooks::default());
    legacy.steering.lock().unwrap().push(model.clone());
    legacy.follow_up.lock().unwrap().push(model.clone());
    let legacy_agent = Agent::new(config(ScriptedProvider::new(vec![]), vec![], legacy), Vec::new());
    let sink = Arc::new(InputSink::default());
    legacy_agent.prompt(vec![model.clone()], CancellationToken::new(), sink.clone()).await.unwrap();
    assert_input_identity(&sink.inputs.lock().unwrap(), &vec![AgentInput::from(model); 3]);
}

#[tokio::test]
async fn owned_idle_continue_skips_the_second_steer_until_the_next_model_turn() {
    let model = Message::User(UserMessage {
        content: UserContent::Text("same".into()),
        synthetic: None,
        timestamp: 42,
        provider_payload: None,
        provider_payload_origin: None,
    });
    for empty in [true, false] {
        let provider = ScriptedProvider::new(vec![]);
        let previous =
            if empty { vec![] } else { vec![Message::Assistant(AssistantMessage::empty("test", "test", "test"))] };
        let agent = Agent::new(config(provider.clone(), vec![], Arc::new(NoHooks)), previous);
        let first = owned_input(&model, "first");
        let second = owned_input(&model, "second");
        agent.steer_input(first.clone());
        agent.steer_input(second.clone());
        let sink = Arc::new(InputSink::default());
        let report = agent.continue_run(CancellationToken::new(), sink.clone()).await.unwrap();
        assert_eq!(report.end, RunEnd::Completed);
        assert_input_identity(&sink.inputs.lock().unwrap(), &[first, second]);
        assert_eq!(
            provider.contexts.lock().unwrap().iter().map(|ctx| user_texts(&ctx.messages).len()).collect::<Vec<_>>(),
            [1, 2]
        );
    }
}

#[test]
fn owned_queue_peek_pop_and_old_projection_share_one_queue() {
    let model = Message::User(UserMessage {
        content: UserContent::Text("same".into()),
        synthetic: None,
        timestamp: 42,
        provider_payload: None,
        provider_payload_origin: None,
    });
    let agent = Agent::new(config(ScriptedProvider::new(vec![]), vec![], Arc::new(NoHooks)), Vec::new());
    let first = owned_input(&model, "first");
    let second = owned_input(&model, "second");
    agent.steer_input(first.clone());
    agent.steer_input(second.clone());
    agent.follow_up_input(first.clone());
    agent.follow_up_input(second.clone());
    assert_input_identity(&[agent.pop_last_steer_input().unwrap()], std::slice::from_ref(&second));
    assert_input_identity(&[agent.pop_last_follow_up_input().unwrap()], &[second]);
    assert_input_identity(&agent.peek_steering_inputs(), std::slice::from_ref(&first));
    assert_input_identity(&agent.peek_follow_up_inputs(), &[first]);
    assert_eq!(agent.pop_last_steer(), Some(model.clone()));
    assert_eq!(agent.pop_last_follow_up(), Some(model));
    assert_eq!(agent.queued_counts(), (0, 0));
}

#[tokio::test]
async fn owned_cancelled_idle_dequeue_rolls_back_both_queues_with_exact_provenance() {
    let model = Message::User(UserMessage {
        content: UserContent::Text("same".into()),
        synthetic: None,
        timestamp: 42,
        provider_payload: None,
        provider_payload_origin: None,
    });
    for follow_up in [false, true] {
        for mode in [QueueMode::All, QueueMode::OneAtATime] {
            let first = owned_input(&model, "native-first");
            let second = owned_input(&model, "native-second");
            let host = owned_input(&model, "host");
            let gate = Arc::new(InputDequeueGate {
                follow_up,
                inputs: Mutex::new(vec![host.clone()]),
                entered: tokio::sync::Notify::new(),
                release: tokio::sync::Notify::new(),
            });
            let provider = ScriptedProvider::new(vec![]);
            let agent = Agent::new(config(provider.clone(), vec![], Arc::new(NoHooks)), Vec::new());
            agent.set_steering_mode(mode);
            agent.set_follow_up_mode(mode);
            if follow_up {
                agent.follow_up_input(first.clone());
                agent.follow_up_input(second.clone());
            } else {
                agent.steer_input(first.clone());
                agent.steer_input(second.clone());
            }
            let sink = Arc::new(InputSink::default());
            let cancel = CancellationToken::new();
            let waiting = {
                let (agent, cancel, sink, run) =
                    (agent.clone(), cancel.clone(), sink.clone(), config(provider.clone(), vec![], gate.clone()));
                tokio::spawn(async move { agent.continue_run_with_config(run, cancel, sink).await })
            };
            tokio::time::timeout(Duration::from_secs(3), gate.entered.notified()).await.unwrap();
            cancel.cancel();
            gate.release.notify_one();
            assert!(matches!(waiting.await.unwrap(), Err(AgentError::CannotContinue(_))));
            let expected = if mode == QueueMode::All { vec![first, second, host] } else { vec![first, host, second] };
            let remaining = if follow_up { agent.peek_follow_up_inputs() } else { agent.peek_steering_inputs() };
            assert_input_identity(&remaining, &expected);
            assert!(agent.messages().await.unwrap().is_empty());
            assert!(sink.inputs.lock().unwrap().is_empty());
            assert!(sink.events.events.lock().await.is_empty());
            assert!(provider.contexts.lock().unwrap().is_empty());
            let already_cancelled = CancellationToken::new();
            already_cancelled.cancel();
            assert!(matches!(
                agent.continue_run(already_cancelled, sink.clone()).await,
                Err(AgentError::CannotContinue(_))
            ));
            let report = agent.continue_run(CancellationToken::new(), sink.clone()).await.unwrap();
            assert_eq!(report.end, RunEnd::Completed);
            assert_input_identity(&sink.inputs.lock().unwrap(), &expected);
            assert!(!agent.has_queued_messages());
        }
    }
}

#[tokio::test]
async fn owned_already_dequeued_inputs_commit_once_at_model_budget_for_both_hooks() {
    let model = Message::User(UserMessage {
        content: UserContent::Text("same".into()),
        synthetic: None,
        timestamp: 42,
        provider_payload: None,
        provider_payload_origin: None,
    });
    for follow_up in [false, true] {
        let initial = owned_input(&model, "initial");
        let first = owned_input(&model, "pending-first");
        let second = owned_input(&model, "pending-second");
        let hooks = Arc::new(InputHooks::default());
        if follow_up {
            hooks.follow_up.lock().unwrap().extend([first.clone(), second.clone()]);
        } else {
            hooks.steering.lock().unwrap().extend([first.clone(), second.clone()]);
        }
        let provider = ScriptedProvider::new(vec![]);
        let mut cfg = config(provider.clone(), vec![], hooks);
        cfg.max_model_calls = Some(usize::from(follow_up));
        let sink = InputSink::default();
        let mut context = Vec::new();
        let report =
            agent_loop_inputs(vec![initial.clone()], &mut context, &cfg, &CancellationToken::new(), &sink).await;
        assert_eq!(report.end, RunEnd::ModelCallBudget);
        assert_input_identity(&sink.inputs.lock().unwrap(), &[initial, first, second]);
        assert_eq!(user_texts(&report.model_messages().unwrap()), ["same", "same", "same"]);
        assert!({
            let models = report.model_messages().unwrap();
            context.len() == models.len()
                && context
                    .iter()
                    .zip(&models)
                    .all(|(record, model)| ara_agent::checked_model_equal(record, model).unwrap())
        });
        assert_eq!(provider.contexts.lock().unwrap().len(), usize::from(follow_up));
    }
}

#[tokio::test]
async fn owned_already_dequeued_inputs_commit_once_after_deadline_for_both_hooks() {
    let model = Message::User(UserMessage {
        content: UserContent::Text("same".into()),
        synthetic: None,
        timestamp: 42,
        provider_payload: None,
        provider_payload_origin: None,
    });
    for follow_up in [false, true] {
        let initial = owned_input(&model, "initial");
        let first = owned_input(&model, "pending-first");
        let second = owned_input(&model, "pending-second");
        let gate = Arc::new(InputDequeueGate {
            follow_up,
            inputs: Mutex::new(vec![first.clone(), second.clone()]),
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        let provider = ScriptedProvider::new(vec![]);
        let mut cfg = config(provider.clone(), vec![], gate.clone());
        let deadline = Instant::now() + Duration::from_millis(500);
        cfg.deadline = Some(deadline);
        let sink = Arc::new(InputSink::default());
        let waiting = {
            let (sink, initial) = (sink.clone(), initial.clone());
            tokio::spawn(async move {
                let mut context = Vec::new();
                let report =
                    agent_loop_inputs(vec![initial], &mut context, &cfg, &CancellationToken::new(), sink.as_ref())
                        .await;
                (report, context)
            })
        };
        tokio::time::timeout(Duration::from_secs(3), gate.entered.notified()).await.unwrap();
        tokio::time::sleep_until(deadline.into()).await;
        gate.release.notify_one();
        let (report, context) = waiting.await.unwrap();
        assert_eq!(report.end, RunEnd::Deadline);
        assert_input_identity(&sink.inputs.lock().unwrap(), &[initial, first, second]);
        assert_eq!(user_texts(&report.model_messages().unwrap()), ["same", "same", "same"]);
        assert!({
            let models = report.model_messages().unwrap();
            context.len() == models.len()
                && context
                    .iter()
                    .zip(&models)
                    .all(|(record, model)| ara_agent::checked_model_equal(record, model).unwrap())
        });
        assert_eq!(provider.contexts.lock().unwrap().len(), usize::from(follow_up));
    }
}

#[tokio::test]
async fn run_config_selects_provider_tools_hooks_and_request_options() {
    struct ObservedProvider {
        scripted: Arc<ScriptedProvider>,
        requests: Mutex<Vec<(Model, CallOptions)>>,
    }
    impl ModelProvider for ObservedProvider {
        fn stream(&self, model: &Model, context: &Context, options: CallOptions) -> AssistantStream {
            self.requests.lock().unwrap().push((model.clone(), options.clone()));
            self.scripted.stream(model, context, options)
        }
    }
    struct RunHooks(Hooks);
    #[async_trait]
    impl LoopHooks for RunHooks {
        async fn steering_messages(&self) -> Vec<Message> {
            self.0.steering_messages().await
        }
        async fn transform_provider_context(&self, mut context: Context, _model: &Model) -> Context {
            context.system_prompt.push("run transform".into());
            context
        }
    }
    let defaults = ScriptedProvider::new(vec![
        reply("", &[("default-tool", "echo", json!({"text":"denied"}))], StopReason::ToolUse),
        reply("default restored", &[], StopReason::Stop),
    ]);
    let default_hooks = Arc::new(Hooks { block: true, ..Default::default() });
    let (default_tool, default_effects) = echo(0, Concurrency::Shared);
    let agent = Agent::new(config(defaults.clone(), vec![default_tool], default_hooks), vec![]);
    agent.steer(user("owned steer"));
    let hooks = Arc::new(RunHooks(Hooks::default()));
    hooks.0.steering.lock().unwrap().push(user("run hook steer"));
    let scripted =
        ScriptedProvider::new(vec![reply("", &[("run-tool", "echo", json!({"text":"once"}))], StopReason::ToolUse)]);
    let provider = Arc::new(ObservedProvider { scripted: scripted.clone(), requests: Mutex::new(vec![]) });
    let (tool, effects) = echo(0, Concurrency::Shared);
    let mut run = config(provider.clone(), vec![tool], hooks);
    run.model.id = "run-model".into();
    run.system_prompt = vec!["run prompt".into()];
    run.max_tokens = Some(37);
    run.temperature = Some(0.25);
    run.tool_choice = Some(ToolChoice::Tool("echo".into()));
    run.max_model_calls = Some(1);
    let report = agent
        .prompt_with_config(vec![user("initial")], run, CancellationToken::new(), Arc::new(NullSink))
        .await
        .unwrap();
    assert_eq!(report.end, RunEnd::ModelCallBudget);
    assert_eq!(*effects.lock().unwrap(), ["start:run-tool", "end:run-tool"]);
    assert!(defaults.contexts.lock().unwrap().is_empty());
    {
        let requests = provider.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0.id, "run-model");
        assert_eq!(requests[0].1.max_tokens, Some(37));
        assert_eq!(requests[0].1.temperature, Some(0.25));
        assert_eq!(requests[0].1.tool_choice, Some(ToolChoice::Tool("echo".into())));
        let contexts = scripted.contexts.lock().unwrap();
        assert_eq!(contexts[0].system_prompt, ["run prompt", "run transform"]);
        assert_eq!(contexts[0].tools.as_ref().unwrap()[0].name, "echo");
        assert_eq!(user_texts(&contexts[0].messages), ["initial", "owned steer", "run hook steer"]);
    }
    assert_eq!(
        agent.prompt(vec![user("defaults again")], CancellationToken::new(), Arc::new(NullSink)).await.unwrap().end,
        RunEnd::Completed
    );
    assert!(default_effects.lock().unwrap().is_empty());
    let messages = agent.messages().await.unwrap();
    assert!(messages.iter().any(|message| matches!(message, Message::ToolResult(result) if result.tool_call_id == "default-tool" && result.is_error)));
    assert_eq!(messages.last().unwrap().as_assistant().unwrap().text(), "default restored");
    assert_eq!(messages.last().unwrap().as_assistant().unwrap().model, "scripted");
    let contexts = defaults.contexts.lock().unwrap();
    assert_eq!(contexts.len(), 2);
    assert_eq!(contexts[0].system_prompt, ["sys"]);
    assert_eq!(contexts[0].tools.as_ref().unwrap()[0].name, "echo");
}

#[tokio::test]
async fn run_config_refreshes_deadlines_without_replacing_transcript_queues_or_defaults() {
    let defaults = ScriptedProvider::new(vec![]);
    let mut original = config(defaults.clone(), vec![], Arc::new(NoHooks));
    original.deadline = Some(Instant::now() - Duration::from_secs(1));
    let agent = Agent::new(original, vec![]);
    let provider = ScriptedProvider::new(vec![
        reply("one", &[], StopReason::Stop),
        reply("two", &[], StopReason::Stop),
        reply("three", &[], StopReason::Stop),
    ]);
    let fresh = || {
        let mut run = config(provider.clone(), vec![], Arc::new(NoHooks));
        run.deadline = Some(Instant::now() + Duration::from_secs(30));
        run
    };
    for text in ["first", "second"] {
        assert_eq!(
            agent
                .prompt_with_config(vec![user(text)], fresh(), CancellationToken::new(), Arc::new(NullSink))
                .await
                .unwrap()
                .end,
            RunEnd::Completed
        );
    }
    agent.set_steering_mode(QueueMode::All);
    agent.steer(user("third-a"));
    agent.steer(user("third-b"));
    assert!(matches!(
        agent.continue_run(CancellationToken::new(), Arc::new(NullSink)).await,
        Err(AgentError::CannotContinue(_))
    ));
    assert_eq!(agent.queued_counts(), (2, 0));
    assert_eq!(
        agent.continue_run_with_config(fresh(), CancellationToken::new(), Arc::new(NullSink)).await.unwrap().end,
        RunEnd::Completed
    );
    assert_eq!(agent.steering_mode(), QueueMode::All);
    assert_eq!(user_texts(&agent.messages().await.unwrap()), ["first", "second", "third-a", "third-b"]);
    assert!(defaults.contexts.lock().unwrap().is_empty());
    assert_eq!(provider.contexts.lock().unwrap().len(), 3);
    assert_eq!(agent.queued_counts(), (0, 0));
}

#[tokio::test]
async fn run_config_continues_empty_and_native_transcripts_with_supplied_hooks_and_deadline() {
    let provider = ScriptedProvider::new(vec![
        reply("empty", &[], StopReason::Stop),
        reply("native", &[], StopReason::Stop),
        reply("recorded tool", &[], StopReason::Stop),
    ]);
    let defaults = ScriptedProvider::new(vec![]);
    let mut assistant = AssistantMessage::empty("test", "test", "test");
    assistant.stop_reason = StopReason::ToolUse;
    assistant.content.push(AssistantBlock::ToolCall(call_block("recorded", "echo", &json!({"text":"once"}))));
    let recorded = vec![
        user("native pending prompt"),
        Message::Assistant(assistant),
        Message::ToolResult(ToolResultMessage {
            tool_call_id: "recorded".into(),
            tool_name: "echo".into(),
            content: vec![UserBlock::text("recorded receipt")],
            details: None,
            is_error: false,
            timestamp: 0,
            provider_metadata: None,
            useless: false,
        }),
    ];
    let (tool, effects) = echo(0, Concurrency::Shared);
    for messages in [vec![], vec![user("native pending prompt")], recorded] {
        let mut original = config(defaults.clone(), vec![], Arc::new(NoHooks));
        original.deadline = Some(Instant::now() - Duration::from_secs(1));
        let empty = messages.is_empty();
        let agent = Agent::new(original, messages);
        let hooks = Arc::new(Hooks::default());
        if empty {
            hooks.follow_up.lock().unwrap().push(user("supplied hook follow-up"));
        }
        let mut run = config(provider.clone(), vec![tool.clone()], hooks);
        run.deadline = Some(Instant::now() + Duration::from_secs(30));
        assert_eq!(
            agent.continue_run_with_config(run, CancellationToken::new(), Arc::new(NullSink)).await.unwrap().end,
            RunEnd::Completed
        );
        assert_eq!(
            user_texts(&agent.messages().await.unwrap()),
            [if empty { "supplied hook follow-up" } else { "native pending prompt" }]
        );
    }
    assert!(defaults.contexts.lock().unwrap().is_empty());
    assert_eq!(provider.contexts.lock().unwrap().len(), 3);
    assert!(effects.lock().unwrap().is_empty());
}

#[tokio::test]
async fn run_config_preserves_cancelled_dequeue_and_unknown_effect_refusal() {
    struct DequeueGate {
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }
    #[async_trait]
    impl LoopHooks for DequeueGate {
        async fn steering_messages(&self) -> Vec<Message> {
            self.entered.notify_one();
            self.release.notified().await;
            vec![user("supplied hook message")]
        }
    }
    let defaults = ScriptedProvider::new(vec![]);
    let provider = ScriptedProvider::new(vec![]);
    let agent = Agent::new(config(defaults.clone(), vec![], Arc::new(NoHooks)), vec![]);
    let gate = Arc::new(DequeueGate { entered: tokio::sync::Notify::new(), release: tokio::sync::Notify::new() });
    let cancel = CancellationToken::new();
    let pending = {
        let (agent, cancel, run) = (agent.clone(), cancel.clone(), config(provider.clone(), vec![], gate.clone()));
        tokio::spawn(async move { agent.continue_run_with_config(run, cancel, Arc::new(NullSink)).await })
    };
    tokio::time::timeout(Duration::from_secs(3), gate.entered.notified()).await.unwrap();
    agent.steer(user("owned message"));
    cancel.cancel();
    gate.release.notify_one();
    assert!(matches!(pending.await.unwrap(), Err(AgentError::CannotContinue(_))));
    assert_eq!(user_texts(&agent.peek_steering_queue().unwrap()), ["owned message", "supplied hook message"]);
    assert!(agent.messages().await.unwrap().is_empty());
    assert!(provider.contexts.lock().unwrap().is_empty());
    let mut expired = config(provider.clone(), vec![], Arc::new(NoHooks));
    expired.deadline = Some(Instant::now() - Duration::from_secs(1));
    assert!(matches!(
        agent.continue_run_with_config(expired, CancellationToken::new(), Arc::new(NullSink)).await,
        Err(AgentError::CannotContinue(_))
    ));
    assert_eq!(agent.queued_counts(), (2, 0));
    let mut tail = AssistantMessage::empty("test", "test", "test");
    tail.stop_reason = StopReason::ToolUse;
    tail.content.push(AssistantBlock::ToolCall(call_block("unknown-run", "echo", &json!({"text":"unknown"}))));
    let unsafe_agent = Agent::new(config(defaults, vec![], Arc::new(NoHooks)), vec![Message::Assistant(tail)]);
    unsafe_agent.steer(user("must remain"));
    let (tool, effects) = echo(0, Concurrency::Shared);
    let run = config(provider.clone(), vec![tool], Arc::new(NoHooks));
    assert!(matches!(
        unsafe_agent
            .prompt_with_config(vec![user("refused")], run.clone(), CancellationToken::new(), Arc::new(NullSink))
            .await,
        Err(AgentError::CannotContinue(_))
    ));
    assert!(matches!(
        unsafe_agent.continue_run_with_config(run, CancellationToken::new(), Arc::new(NullSink)).await,
        Err(AgentError::CannotContinue(_))
    ));
    assert!(effects.lock().unwrap().is_empty());
    assert!(provider.contexts.lock().unwrap().is_empty());
    assert_eq!(unsafe_agent.queued_counts(), (1, 0));
    assert_eq!(unsafe_agent.messages().await.unwrap().len(), 1);
}

#[tokio::test]
async fn run_config_busy_rejection_and_dropped_waiter_retain_owned_effects_and_receipts() {
    struct GatedSink {
        recorded: RecordingSink,
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
        completed: tokio::sync::Notify,
    }
    #[async_trait]
    impl AgentEventSink for GatedSink {
        async fn emit(&self, event: AgentEvent) {
            let tool_start = matches!(event, AgentEvent::ToolExecutionStart { .. });
            let completed = matches!(event, AgentEvent::AgentEnd { .. });
            self.recorded.emit(event).await;
            if tool_start {
                self.entered.notify_one();
                self.release.notified().await;
            }
            if completed {
                self.completed.notify_one();
            }
        }
    }
    let defaults = ScriptedProvider::new(vec![]);
    let provider = ScriptedProvider::new(vec![
        reply("", &[("owned-run", "echo", json!({"text":"once"}))], StopReason::ToolUse),
        reply("done", &[], StopReason::Stop),
    ]);
    let (tool, effects) = echo(0, Concurrency::Shared);
    let agent = Agent::new(config(defaults.clone(), vec![], Arc::new(NoHooks)), vec![]);
    let sink = Arc::new(GatedSink {
        recorded: RecordingSink::default(),
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
        completed: tokio::sync::Notify::new(),
    });
    let waiting = {
        let (agent, sink, run) = (agent.clone(), sink.clone(), config(provider.clone(), vec![tool], Arc::new(NoHooks)));
        tokio::spawn(async move {
            agent.prompt_with_config(vec![user("initial")], run, CancellationToken::new(), sink).await
        })
    };
    tokio::time::timeout(Duration::from_secs(3), sink.entered.notified()).await.unwrap();
    agent.steer(user("queued"));
    let rejected = config(defaults.clone(), vec![], Arc::new(NoHooks));
    assert!(matches!(
        agent
            .prompt_with_config(vec![user("wrong")], rejected.clone(), CancellationToken::new(), Arc::new(NullSink))
            .await,
        Err(AgentError::Busy)
    ));
    assert!(matches!(
        agent.continue_run_with_config(rejected, CancellationToken::new(), Arc::new(NullSink)).await,
        Err(AgentError::Busy)
    ));
    assert_eq!(user_texts(&agent.peek_steering_queue().unwrap()), ["queued"]);
    waiting.abort();
    assert!(waiting.await.unwrap_err().is_cancelled());
    sink.release.notify_one();
    // messages() is a live snapshot, not a join. Observe the detached Run end.
    tokio::time::timeout(Duration::from_secs(3), sink.completed.notified()).await.unwrap();
    let messages = tokio::time::timeout(Duration::from_secs(3), agent.messages()).await.unwrap().unwrap();
    assert_eq!(*effects.lock().unwrap(), ["start:owned-run", "end:owned-run"]);
    assert_eq!(user_texts(&messages), ["initial", "queued"]);
    assert_eq!(messages.last().unwrap().as_assistant().unwrap().text(), "done");
    assert!(
        sink.recorded
            .events
            .lock()
            .await
            .iter()
            .any(|event| matches!(ended_model_message(event), Some(Message::ToolResult(_))))
    );
    assert!(defaults.contexts.lock().unwrap().is_empty());
}

#[tokio::test]
async fn stateful_agent_drains_steering_then_follow_up_and_rejects_parallel_prompt() {
    let provider = ScriptedProvider::new(vec![
        reply("", &[("one", "echo", json!({"text": "once"}))], StopReason::ToolUse),
        reply("after steer", &[], StopReason::Stop),
        reply("after follow-up", &[], StopReason::Stop),
    ]);
    let (tool, log) = echo(250, Concurrency::Shared);
    let agent = Agent::new(config(provider.clone(), vec![tool], Arc::new(NoHooks)), Vec::new());
    let sink = Arc::new(RecordingSink::default());
    let running = {
        let agent = agent.clone();
        let sink = sink.clone();
        tokio::spawn(async move { agent.prompt(vec![user("initial")], CancellationToken::new(), sink).await })
    };
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if log.lock().unwrap().iter().any(|v| v == "start:one") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(agent.is_busy());
    assert!(matches!(
        agent.prompt(vec![user("wrong")], CancellationToken::new(), Arc::new(NullSink)).await,
        Err(AgentError::Busy)
    ));
    agent.steer(user("steer"));
    agent.follow_up(user("follow-up"));
    let report = running.await.unwrap().unwrap();
    assert_eq!(report.end, RunEnd::Completed);
    assert_eq!(user_texts(&agent.messages().await.unwrap()), ["initial", "steer", "follow-up"]);
    assert_eq!(*log.lock().unwrap(), vec!["start:one", "end:one"]);
    assert_eq!(provider.contexts.lock().unwrap().len(), 3);
    assert_eq!(agent.queued_counts(), (0, 0));
    assert!(!agent.is_busy());
}

#[tokio::test]
async fn stateful_agent_queue_modes_control_model_turns_independently() {
    async fn run(steering: QueueMode, follow_up: QueueMode) -> (Vec<String>, usize) {
        let provider = ScriptedProvider::new(vec![
            reply("one", &[], StopReason::Stop),
            reply("two", &[], StopReason::Stop),
            reply("three", &[], StopReason::Stop),
            reply("four", &[], StopReason::Stop),
        ]);
        let agent = Agent::new(config(provider.clone(), vec![], Arc::new(NoHooks)), Vec::new());
        assert_eq!(agent.steering_mode(), QueueMode::OneAtATime);
        assert_eq!(agent.follow_up_mode(), QueueMode::OneAtATime);
        agent.set_steering_mode(steering);
        agent.set_follow_up_mode(follow_up);
        agent.steer(user("steer-a"));
        agent.steer(user("steer-b"));
        agent.follow_up(user("follow-a"));
        agent.follow_up(user("follow-b"));
        let report = agent.prompt(vec![user("initial")], CancellationToken::new(), Arc::new(NullSink)).await.unwrap();
        assert_eq!(report.end, RunEnd::Completed);
        assert!(!agent.has_queued_messages());
        (user_texts(&agent.messages().await.unwrap()), provider.contexts.lock().unwrap().len())
    }

    let expected = ["initial", "steer-a", "steer-b", "follow-a", "follow-b"];
    assert_eq!(run(QueueMode::OneAtATime, QueueMode::OneAtATime).await, (expected.map(str::to_string).to_vec(), 4));
    assert_eq!(run(QueueMode::All, QueueMode::All).await, (expected.map(str::to_string).to_vec(), 2));
    assert_eq!(run(QueueMode::All, QueueMode::OneAtATime).await, (expected.map(str::to_string).to_vec(), 3));
    assert_eq!(run(QueueMode::OneAtATime, QueueMode::All).await, (expected.map(str::to_string).to_vec(), 3));
}

#[tokio::test]
async fn idle_continue_admission_preserves_host_and_agent_queues_and_releases_busy() {
    for follow_up in [false, true] {
        for failure in 0..4 {
            let entered = Arc::new(tokio::sync::Notify::new());
            let action = match failure {
                0 => AdmissionAction::Block,
                1 => AdmissionAction::Cancel,
                2 => AdmissionAction::CancelOnReturn,
                _ => AdmissionAction::Wait(entered.clone()),
            };
            let hooks = Arc::new(AdmissionHooks::default());
            if follow_up {
                hooks.queued_admission.lock().unwrap().push_back(AdmissionAction::Allow);
            }
            hooks.queued_admission.lock().unwrap().push_back(action);
            let host_input = owned_input(&user("host queued"), "host");
            let agent_input = owned_input(&user("agent queued"), "agent");
            let host_queue = if follow_up { &hooks.queues.follow_up } else { &hooks.queues.steering };
            host_queue.lock().unwrap().push(host_input.clone());
            let provider = ScriptedProvider::new(vec![]);
            let agent = Agent::new(config(provider.clone(), vec![], hooks.clone()), Vec::new());
            if follow_up {
                agent.follow_up_input(agent_input.clone());
            } else {
                agent.steer_input(agent_input.clone());
            }
            let cancel = CancellationToken::new();
            let running = {
                let agent = agent.clone();
                let cancel = cancel.clone();
                tokio::spawn(async move { agent.continue_run(cancel, Arc::new(NullSink)).await })
            };
            if failure == 3 {
                tokio::time::timeout(Duration::from_secs(3), entered.notified()).await.unwrap();
                cancel.cancel();
            }
            let error = tokio::time::timeout(Duration::from_secs(3), running).await.unwrap().unwrap().unwrap_err();
            if failure == 0 {
                assert!(matches!(error, AgentError::Admission(AdmissionError::Blocked(_))));
            } else {
                assert!(matches!(error, AgentError::Admission(AdmissionError::Cancelled)));
            }
            assert!(!agent.is_busy());
            assert!(agent.messages().await.unwrap().is_empty());
            assert!(provider.contexts.lock().unwrap().is_empty());
            assert_eq!(*hooks.dequeues.lock().unwrap(), [usize::from(follow_up), 0]);
            assert_input_identity(&host_queue.lock().unwrap(), &[host_input]);
            let owned = if follow_up { agent.peek_follow_up_inputs() } else { agent.peek_steering_inputs() };
            assert_input_identity(&owned, &[agent_input]);
        }
    }
}

#[tokio::test]
async fn running_queue_admission_preserves_inputs_and_emits_distinct_terminal_events() {
    for follow_up in [false, true] {
        for blocked in [false, true] {
            let hooks = Arc::new(AdmissionHooks::default());
            if follow_up {
                hooks.queued_admission.lock().unwrap().extend([
                    AdmissionAction::Allow,
                    AdmissionAction::Allow,
                    AdmissionAction::Allow,
                ]);
            }
            hooks.queued_admission.lock().unwrap().push_back(if blocked {
                AdmissionAction::Block
            } else {
                AdmissionAction::Cancel
            });
            let host_input = owned_input(&user("host queued"), "host");
            let agent_input = owned_input(&user("agent queued"), "agent");
            let host_queue = if follow_up { &hooks.queues.follow_up } else { &hooks.queues.steering };
            host_queue.lock().unwrap().push(host_input.clone());
            let provider = ScriptedProvider::new(vec![reply("first", &[], StopReason::Stop)]);
            let agent = Agent::new(config(provider.clone(), vec![], hooks.clone()), Vec::new());
            if follow_up {
                agent.follow_up_input(agent_input.clone());
            } else {
                agent.steer_input(agent_input.clone());
            }
            let sink = Arc::new(RecordingSink::default());
            let report = agent.prompt(vec![user("initial")], CancellationToken::new(), sink.clone()).await.unwrap();
            assert_eq!(report.end, if blocked { RunEnd::Error } else { RunEnd::Aborted });
            let message = report.model_messages().unwrap().last().unwrap().as_assistant().cloned().unwrap();
            assert_eq!(message.stop_reason, if blocked { StopReason::Error } else { StopReason::Aborted });
            if blocked {
                assert_eq!(message.error_message.as_deref(), Some("503 admission unavailable"));
                let evidence = message.failure_evidence.as_ref().unwrap();
                assert_eq!(evidence.kind, ara_ai::retry_classification::ProviderErrorKind::Config);
                assert!(evidence.replay_blocked && evidence.same_route_blocked);
                assert!(evidence.context_recovery.is_none());
                assert!(ara_ai::retry_classification::classify_retry(&message, "openai-completions").replay_blocked);
            }
            assert!(!agent.is_busy());
            assert_eq!(provider.contexts.lock().unwrap().len(), usize::from(follow_up));
            assert_eq!(*hooks.dequeues.lock().unwrap(), [if follow_up { 3 } else { 0 }, 0]);
            assert_input_identity(&host_queue.lock().unwrap(), &[host_input]);
            let owned = if follow_up { agent.peek_follow_up_inputs() } else { agent.peek_steering_inputs() };
            assert_input_identity(&owned, &[agent_input]);
            let event_types = types(&sink).await;
            assert_eq!(event_types.first().unwrap(), "agent_start");
            assert_eq!(event_types.last().unwrap(), "agent_end");
            assert_eq!(event_types.iter().filter(|t| t.as_str() == "turn_end").count(), 1 + usize::from(follow_up));
        }
    }
}

#[tokio::test]
async fn admission_deadline_stops_uncooperative_queue_and_model_hooks_without_dispatch() {
    for model_gate in [false, true] {
        let entered = Arc::new(tokio::sync::Notify::new());
        let hooks = Arc::new(AdmissionHooks::default());
        let actions = if model_gate { &hooks.model_admission } else { &hooks.queued_admission };
        actions.lock().unwrap().push_back(AdmissionAction::Wait(entered.clone()));
        let provider = ScriptedProvider::new(vec![]);
        let mut cfg = config(provider.clone(), vec![], hooks.clone());
        cfg.deadline = Some(Instant::now() + Duration::from_millis(50));
        let agent = Agent::new(cfg, Vec::new());
        if !model_gate {
            agent.steer(user("still queued"));
        }
        let sink = Arc::new(RecordingSink::default());
        let report = tokio::time::timeout(
            Duration::from_secs(3),
            agent.prompt(vec![user("initial")], CancellationToken::new(), sink.clone()),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(report.end, RunEnd::Deadline);
        let message = report.model_messages().unwrap().last().unwrap().as_assistant().cloned().unwrap();
        assert_eq!(message.stop_reason, StopReason::Aborted);
        assert_eq!(message.error_message.as_deref(), Some("Deadline exceeded"));
        assert!(provider.contexts.lock().unwrap().is_empty());
        assert_eq!(agent.queued_counts(), (usize::from(!model_gate), 0));
        assert!(!agent.is_busy());
        assert_eq!(types(&sink).await.last().unwrap(), "agent_end");
    }
    let hooks = Arc::new(AdmissionHooks::default());
    hooks.queued_admission.lock().unwrap().push_back(AdmissionAction::Wait(Arc::new(tokio::sync::Notify::new())));
    let provider = ScriptedProvider::new(vec![]);
    let mut cfg = config(provider.clone(), vec![], hooks);
    cfg.deadline = Some(Instant::now() + Duration::from_millis(50));
    let agent = Agent::new(cfg, Vec::new());
    agent.steer(user("idle queued"));
    let result =
        tokio::time::timeout(Duration::from_secs(3), agent.continue_run(CancellationToken::new(), Arc::new(NullSink)))
            .await
            .unwrap();
    assert!(matches!(result, Err(AgentError::Admission(AdmissionError::Cancelled))));
    assert_eq!(agent.queued_counts(), (1, 0));
    assert!(!agent.is_busy());
    assert!(agent.messages().await.unwrap().is_empty());
    assert!(provider.contexts.lock().unwrap().is_empty());
}

#[tokio::test]
async fn follow_up_admission_failure_commits_already_dequeued_late_steering() {
    let hooks = Arc::new(AdmissionHooks::default());
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    hooks.queued_admission.lock().unwrap().extend([
        AdmissionAction::Allow,
        AdmissionAction::Allow,
        AdmissionAction::WaitThenAllow(entered.clone(), release.clone()),
        AdmissionAction::Block,
    ]);
    let follow_up = owned_input(&user("follow-up retained"), "follow");
    hooks.queues.follow_up.lock().unwrap().push(follow_up.clone());
    let provider = ScriptedProvider::new(vec![reply("done", &[], StopReason::Stop)]);
    let agent = Agent::new(config(provider.clone(), vec![], hooks.clone()), Vec::new());
    let sink = Arc::new(InputSink::default());
    let running = {
        let agent = agent.clone();
        let sink = sink.clone();
        tokio::spawn(async move { agent.prompt(vec![user("initial")], CancellationToken::new(), sink).await })
    };
    tokio::time::timeout(Duration::from_secs(3), entered.notified()).await.unwrap();
    let steering = owned_input(&user("late steer accepted"), "steer");
    agent.steer_input(steering.clone());
    release.notify_one();
    let report = tokio::time::timeout(Duration::from_secs(3), running).await.unwrap().unwrap().unwrap();
    assert_eq!(report.end, RunEnd::Error);
    assert_eq!(user_texts(&agent.messages().await.unwrap()), ["initial", "late steer accepted"]);
    assert_input_identity(&sink.inputs.lock().unwrap()[1..], &[steering]);
    assert_input_identity(&hooks.queues.follow_up.lock().unwrap(), &[follow_up]);
    assert_eq!(agent.queued_counts(), (0, 0));
    assert_eq!(*hooks.dequeues.lock().unwrap(), [3, 0]);
    assert_eq!(provider.contexts.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn model_admission_blocks_first_call_and_tool_continuation_without_provider_replay() {
    for continuation in [false, true] {
        for failure in 0..4 {
            let entered = Arc::new(tokio::sync::Notify::new());
            let hooks = Arc::new(AdmissionHooks::default());
            if continuation {
                hooks.model_admission.lock().unwrap().push_back(AdmissionAction::Allow);
            }
            hooks.model_admission.lock().unwrap().push_back(match failure {
                0 => AdmissionAction::Block,
                1 => AdmissionAction::Cancel,
                2 => AdmissionAction::CancelOnReturn,
                _ => AdmissionAction::Wait(entered.clone()),
            });
            let provider = ScriptedProvider::new(vec![reply(
                "",
                &[("once", "echo", json!({"text":"once"}))],
                StopReason::ToolUse,
            )]);
            let (tool, effects) = echo(0, Concurrency::Exclusive);
            let agent = Agent::new(config(provider.clone(), vec![tool], hooks.clone()), Vec::new());
            let sink = Arc::new(RecordingSink::default());
            let cancel = CancellationToken::new();
            let running = {
                let agent = agent.clone();
                let cancel = cancel.clone();
                let sink = sink.clone();
                tokio::spawn(async move { agent.prompt(vec![user("initial")], cancel, sink).await })
            };
            if failure == 3 {
                tokio::time::timeout(Duration::from_secs(3), entered.notified()).await.unwrap();
                cancel.cancel();
            }
            let report = tokio::time::timeout(Duration::from_secs(3), running).await.unwrap().unwrap().unwrap();
            assert_eq!(report.end, if failure == 0 { RunEnd::Error } else { RunEnd::Aborted });
            assert_eq!(provider.contexts.lock().unwrap().len(), usize::from(continuation));
            assert_eq!(*hooks.model_admissions.lock().unwrap(), 1 + usize::from(continuation));
            assert_eq!(
                *effects.lock().unwrap(),
                if continuation { vec!["start:once", "end:once"] } else { Vec::<&str>::new() }
            );
            assert!(!agent.is_busy());
            assert_eq!(types(&sink).await.last().unwrap(), "agent_end");
            if continuation {
                assert_eq!(
                    report
                        .model_messages()
                        .unwrap()
                        .into_iter()
                        .filter(|m| matches!(m, Message::ToolResult(_)))
                        .count(),
                    1
                );
            }
        }
    }
}

#[tokio::test]
async fn idle_continue_does_not_dequeue_a_second_steer_before_the_first_model_call() {
    let provider =
        ScriptedProvider::new(vec![reply("answer a", &[], StopReason::Stop), reply("answer b", &[], StopReason::Stop)]);
    let agent = Agent::new(
        config(provider.clone(), vec![], Arc::new(NoHooks)),
        vec![Message::Assistant(AssistantMessage::empty("test", "test", "test"))],
    );
    agent.steer(user("steer-a"));
    agent.steer(user("steer-b"));
    let report = agent.continue_run(CancellationToken::new(), Arc::new(NullSink)).await.unwrap();
    assert_eq!(report.end, RunEnd::Completed);
    {
        let contexts = provider.contexts.lock().unwrap();
        assert_eq!(contexts.len(), 2);
        assert_eq!(user_texts(&contexts[0].messages), ["steer-a"]);
        assert_eq!(user_texts(&contexts[1].messages), ["steer-a", "steer-b"]);
    }
    assert_eq!(user_texts(&agent.messages().await.unwrap()), ["steer-a", "steer-b"]);
}

#[tokio::test]
async fn stateful_agent_queue_peek_pop_and_clear_keep_fifo_order() {
    let provider = ScriptedProvider::new(vec![reply("done", &[], StopReason::Stop)]);
    let agent = Agent::new(config(provider, vec![], Arc::new(NoHooks)), Vec::new());
    agent.steer(user("first"));
    agent.steer(user("second"));
    agent.follow_up(user("later"));
    assert_eq!(user_texts(&agent.peek_steering_queue().unwrap()), ["first", "second"]);
    assert_eq!(user_texts(&agent.peek_follow_up_queue().unwrap()), ["later"]);
    assert_eq!(agent.queued_counts(), (2, 1));
    assert!(agent.has_queued_messages());
    assert_eq!(user_texts(&[agent.pop_last_steer().unwrap()]), ["second"]);
    assert_eq!(agent.queued_counts(), (1, 1));
    agent.clear_follow_up_queue();
    assert!(agent.pop_last_follow_up().is_none());
    let report = agent.prompt(vec![user("initial")], CancellationToken::new(), Arc::new(NullSink)).await.unwrap();
    assert_eq!(report.end, RunEnd::Completed);
    assert_eq!(user_texts(&agent.messages().await.unwrap()), ["initial", "first"]);
    agent.steer(user("remove"));
    agent.follow_up(user("remove too"));
    agent.clear_steering_queue();
    assert_eq!(agent.queued_counts(), (0, 1));
    agent.clear_all_queues();
    assert_eq!(agent.queued_counts(), (0, 0));
}

#[tokio::test]
async fn stateful_agent_can_retract_a_follow_up_during_a_running_tool() {
    let provider = ScriptedProvider::new(vec![
        reply("", &[("one", "echo", json!({"text": "once"}))], StopReason::ToolUse),
        reply("done", &[], StopReason::Stop),
    ]);
    let (tool, log) = echo(100, Concurrency::Shared);
    let agent = Agent::new(config(provider.clone(), vec![tool], Arc::new(NoHooks)), Vec::new());
    let running = {
        let agent = agent.clone();
        tokio::spawn(
            async move { agent.prompt(vec![user("initial")], CancellationToken::new(), Arc::new(NullSink)).await },
        )
    };
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if log.lock().unwrap().iter().any(|v| v == "start:one") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    agent.follow_up(user("withdrawn"));
    assert_eq!(user_texts(&[agent.pop_last_follow_up().unwrap()]), ["withdrawn"]);
    assert_eq!(running.await.unwrap().unwrap().end, RunEnd::Completed);
    assert_eq!(user_texts(&agent.messages().await.unwrap()), ["initial"]);
    assert_eq!(provider.contexts.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn stateful_agent_idle_continue_keeps_queue_on_cancel_and_refuses_unknown_tail() {
    let provider =
        ScriptedProvider::new(vec![reply("first", &[], StopReason::Stop), reply("second", &[], StopReason::Stop)]);
    let agent = Agent::new(config(provider.clone(), vec![], Arc::new(NoHooks)), Vec::new());
    let first = agent.prompt(vec![user("initial")], CancellationToken::new(), Arc::new(NullSink)).await.unwrap();
    assert_eq!(first.end, RunEnd::Completed);
    agent.follow_up(user("queued"));
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(matches!(agent.continue_run(cancelled, Arc::new(NullSink)).await, Err(AgentError::CannotContinue(_))));
    assert_eq!(agent.queued_counts(), (0, 1));
    let second = agent.continue_run(CancellationToken::new(), Arc::new(NullSink)).await.unwrap();
    assert_eq!(second.end, RunEnd::Completed);
    assert_eq!(user_texts(&agent.messages().await.unwrap()), ["initial", "queued"]);
    assert_eq!(provider.contexts.lock().unwrap().len(), 2);

    let mut tail = AssistantMessage::empty("test", "test", "test");
    tail.stop_reason = StopReason::ToolUse;
    tail.content.push(AssistantBlock::ToolCall(call_block("unknown", "echo", &json!({"text": "x"}))));
    let unsafe_agent = Agent::new(config(provider, vec![], Arc::new(NoHooks)), vec![Message::Assistant(tail)]);
    unsafe_agent.steer(user("must remain"));
    let err = unsafe_agent.continue_run(CancellationToken::new(), Arc::new(NullSink)).await.unwrap_err();
    assert!(matches!(err, AgentError::CannotContinue(_)));
    let err =
        unsafe_agent.prompt(vec![user("new prompt")], CancellationToken::new(), Arc::new(NullSink)).await.unwrap_err();
    assert!(matches!(err, AgentError::CannotContinue(_)));
    assert_eq!(unsafe_agent.queued_counts(), (1, 0));
    assert_eq!(unsafe_agent.messages().await.unwrap().len(), 1);
}

#[tokio::test]
async fn stateful_agent_idle_continue_drains_host_hook_and_requeues_cancelled_dequeue() {
    let provider = ScriptedProvider::new(vec![
        reply("first", &[], StopReason::Stop),
        reply("after host queue", &[], StopReason::Stop),
        reply("after cancelled dequeue", &[], StopReason::Stop),
    ]);
    let hooks = Arc::new(Hooks::default());
    let agent = Agent::new(config(provider.clone(), vec![], hooks.clone()), Vec::new());
    agent.prompt(vec![user("initial")], CancellationToken::new(), Arc::new(NullSink)).await.unwrap();
    hooks.follow_up.lock().unwrap().push(user("from host"));
    let next = agent.continue_run(CancellationToken::new(), Arc::new(NullSink)).await.unwrap();
    assert_eq!(next.end, RunEnd::Completed);
    assert_eq!(user_texts(&agent.messages().await.unwrap()), ["initial", "from host"]);

    let slow_hooks = Arc::new(Hooks { slow_dequeue_ms: 100, ..Default::default() });
    let slow_agent = Agent::new(config(provider.clone(), vec![], slow_hooks.clone()), agent.messages().await.unwrap());
    slow_hooks.steering.lock().unwrap().push(user("from slow hook"));
    let cancel = CancellationToken::new();
    let pending = {
        let agent = slow_agent.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move { agent.continue_run(cancel, Arc::new(NullSink)).await })
    };
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if slow_hooks.steering.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    assert!(matches!(pending.await.unwrap(), Err(AgentError::CannotContinue(_))));
    assert_eq!(slow_agent.queued_counts(), (1, 0));
    assert_eq!(user_texts(&slow_agent.messages().await.unwrap()), ["initial", "from host"]);
    let resumed = slow_agent.continue_run(CancellationToken::new(), Arc::new(NullSink)).await.unwrap();
    assert_eq!(resumed.end, RunEnd::Completed);
    assert_eq!(user_texts(&slow_agent.messages().await.unwrap()), ["initial", "from host", "from slow hook"]);
    assert_eq!(provider.contexts.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn stateful_agent_prefers_steer_enqueued_during_host_hook_wait() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct SlowEmptyHook(Arc<AtomicBool>);
    #[async_trait]
    impl LoopHooks for SlowEmptyHook {
        async fn steering_messages(&self) -> Vec<Message> {
            self.0.store(true, Ordering::Release);
            tokio::time::sleep(Duration::from_millis(80)).await;
            Vec::new()
        }
    }
    let provider = ScriptedProvider::new(vec![
        reply("steering answer", &[], StopReason::Stop),
        reply("follow-up answer", &[], StopReason::Stop),
    ]);
    let started = Arc::new(AtomicBool::new(false));
    let agent = Agent::new(
        config(provider, vec![], Arc::new(SlowEmptyHook(started.clone()))),
        vec![Message::Assistant(AssistantMessage::empty("test", "test", "test"))],
    );
    agent.follow_up(user("follow-up"));
    let pending = {
        let agent = agent.clone();
        tokio::spawn(async move { agent.continue_run(CancellationToken::new(), Arc::new(NullSink)).await })
    };
    tokio::time::timeout(Duration::from_secs(3), async {
        while !started.load(Ordering::Acquire) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    agent.steer(user("late steer"));
    assert_eq!(pending.await.unwrap().unwrap().end, RunEnd::Completed);
    assert_eq!(user_texts(&agent.messages().await.unwrap()), ["late steer", "follow-up"]);
}

#[tokio::test]
async fn stateful_agent_abort_only_cancels_its_run() {
    let provider = ScriptedProvider::new(vec![Turn::HangAfterPartial]);
    let agent = Agent::new(config(provider, vec![], Arc::new(NoHooks)), Vec::new());
    let parent_cancel = CancellationToken::new();
    let sink = Arc::new(RecordingSink::default());
    let running = {
        let agent = agent.clone();
        let cancel = parent_cancel.clone();
        let sink = sink.clone();
        tokio::spawn(async move { agent.prompt(vec![user("start")], cancel, sink).await })
    };
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if sink.events.lock().await.iter().any(|event| matches!(event, AgentEvent::MessageUpdate { .. })) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    agent.abort();
    let report = tokio::time::timeout(Duration::from_secs(3), running).await.unwrap().unwrap().unwrap();
    assert_eq!(report.end, RunEnd::Aborted);
    assert!(!parent_cancel.is_cancelled());
    assert!(!agent.is_busy());
}

#[tokio::test]
async fn stateful_agent_survives_dropped_waiter_without_replaying_tool() {
    let provider = ScriptedProvider::new(vec![
        reply("", &[("one", "echo", json!({"text": "once"}))], StopReason::ToolUse),
        reply("done", &[], StopReason::Stop),
    ]);
    let (tool, log) = echo(100, Concurrency::Shared);
    let agent = Agent::new(config(provider, vec![tool], Arc::new(NoHooks)), Vec::new());
    let waiting = {
        let agent = agent.clone();
        tokio::spawn(async move { agent.prompt(vec![user("run")], CancellationToken::new(), Arc::new(NullSink)).await })
    };
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if log.lock().unwrap().iter().any(|v| v == "start:one") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    waiting.abort();
    tokio::time::timeout(Duration::from_secs(3), async {
        while agent.is_busy() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(*log.lock().unwrap(), vec!["start:one", "end:one"]);
    assert_eq!(agent.messages().await.unwrap().last().unwrap().as_assistant().unwrap().text(), "done");
}

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
    .await
    .model_messages()
    .unwrap();
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
async fn stop_with_tool_calls_is_a_completed_compaction_span_after_receipt_and_answer() {
    let provider = ScriptedProvider::new(vec![
        reply("", &[("c1", "echo", json!({"text": "one"}))], StopReason::Stop),
        reply("done", &[], StopReason::Stop),
    ]);
    let (tool, _) = echo(1, Concurrency::Shared);
    let sink = RecordingSink::default();
    let mut context = Vec::new();
    let report = agent_loop(
        vec![user("go")],
        &mut context,
        &config(provider, vec![tool], Arc::new(NoHooks)),
        &CancellationToken::new(),
        &sink,
    )
    .await;
    assert_eq!(report.end, RunEnd::Completed);
    assert_eq!(context.len(), 4);
    let context = checked_model_messages(&context).unwrap();
    assert!(matches!(context[2], Message::ToolResult(_)));
    let ids: Vec<_> = (1..=context.len()).map(|index| format!("e{index}")).collect();
    let sources: Vec<_> =
        ids.iter().zip(&context).map(|(id, message)| SummarySource { entry_id: id, message }).collect();
    assert!(build_summary_prompt(&sources, None).is_ok());
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
    .await
    .model_messages()
    .unwrap();
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
async fn legacy_schema_validation_blocks_invalid_effect_and_replays_both_results() {
    let provider = ScriptedProvider::new(vec![
        reply(
            "",
            &[
                ("bad", "legacy_schema", json!({"item":"ok","pair":["a",1,"extra"]})),
                ("good", "legacy_schema", json!({"item":"ok","pair":["a",1]})),
            ],
            StopReason::ToolUse,
        ),
        reply("checked", &[], StopReason::Stop),
    ]);
    let effects = Arc::new(Mutex::new(Vec::new()));
    let tool: Arc<dyn AgentTool> = Arc::new(LegacySchemaTool { effects: effects.clone() });
    let new = agent_loop(
        vec![user("check the pair")],
        &mut Vec::new(),
        &config(provider.clone(), vec![tool], Arc::new(NoHooks)),
        &CancellationToken::new(),
        &NullSink,
    )
    .await
    .model_messages()
    .unwrap();
    let results = new
        .iter()
        .filter_map(|message| match message {
            Message::ToolResult(result) => Some(result),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].tool_call_id, "bad");
    assert!(results[0].is_error);
    assert!(result_text(&Message::ToolResult(results[0].clone())).contains("pair/2: must not match false schema"));
    assert_eq!(results[1].tool_call_id, "good");
    assert!(!results[1].is_error);
    assert_eq!(result_text(&Message::ToolResult(results[1].clone())), "legacy tool executed");
    assert_eq!(*effects.lock().unwrap(), vec![json!({"item":"ok","pair":["a",1]})]);
    let contexts = provider.contexts.lock().unwrap();
    assert!(contexts[1].messages.iter().any(|message| matches!(message,
        Message::ToolResult(result) if result.tool_call_id == "bad" && result.is_error)));
    assert!(contexts[1].messages.iter().any(|message| matches!(message,
        Message::ToolResult(result) if result.tool_call_id == "good" && !result.is_error)));
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
    .await
    .model_messages()
    .unwrap();
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
    .await
    .model_messages()
    .unwrap();
    assert_eq!(result_text(&new[2]), "denied by host policy");
    assert!(log.lock().unwrap().is_empty(), "blocked call has no effect");

    // Fixed edit-auto-generated-regressions.test.ts:305, B-c7cc09c8e7.
    // A non-throwing self-error must preserve its text and explicit wire flag.
    struct SelfError;
    #[async_trait]
    impl AgentTool for SelfError {
        fn definition(&self) -> Tool {
            Tool { name: "edit".into(), parameters: json!({"type":"object"}), ..Default::default() }
        }
        async fn execute(
            &self,
            _: &str,
            _: JsonObject,
            _: CancellationToken,
            _: UpdateFn,
        ) -> Result<ToolOutput, ToolError> {
            let mut result = ToolOutput::text("intentional non-throwing failure");
            result.details = Some(json!({}));
            result.is_error = true;
            Ok(result)
        }
    }
    let provider = ScriptedProvider::new(vec![
        reply("", &[("call_self_error", "edit", json!({}))], StopReason::ToolUse),
        reply("done", &[], StopReason::Stop),
    ]);
    let sink = RecordingSink::default();
    let report = agent_loop(
        vec![user("trigger self-error")],
        &mut Vec::new(),
        &config(provider, vec![Arc::new(SelfError)], Arc::new(NoHooks)),
        &CancellationToken::new(),
        &sink,
    )
    .await;
    let messages = report.model_messages().unwrap();
    let result = messages
        .iter()
        .find_map(|message| match message {
            Message::ToolResult(result) if result.tool_call_id == "call_self_error" => Some(result),
            _ => None,
        })
        .unwrap();
    assert!(result.is_error);
    assert!(result_text(&Message::ToolResult(result.clone())).contains("intentional non-throwing failure"));
    let events = sink.events.lock().await;
    let event = events
        .iter()
        .find(
            |event| matches!(event, AgentEvent::ToolExecutionEnd {tool_call_id,..} if tool_call_id=="call_self_error"),
        )
        .unwrap();
    assert_eq!(event.printable().unwrap()["isError"], true);
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
    .await
    .model_messages()
    .unwrap();
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
    .await
    .model_messages()
    .unwrap();
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
    .await
    .model_messages()
    .unwrap();
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
    .await
    .model_messages()
    .unwrap();
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
    let _ = agent_loop(
        vec![user("go")],
        &mut Vec::new(),
        &config(provider, vec![shared], Arc::new(NoHooks)),
        &CancellationToken::new(),
        &NullSink,
    )
    .await
    .model_messages()
    .unwrap();
    let shared_elapsed = started.elapsed();
    assert!(shared_elapsed < Duration::from_millis(1000), "two batches of overlapping 300ms calls: {shared_elapsed:?}");

    let provider = ScriptedProvider::new(vec![
        reply("", &[("a", "echo", json!({"text": "a"})), ("b", "echo", json!({"text": "b"}))], StopReason::ToolUse),
        reply("end", &[], StopReason::Stop),
    ]);
    let (exclusive, log) = echo(200, Concurrency::Exclusive);
    let started = Instant::now();
    let _ = agent_loop(
        vec![user("go")],
        &mut Vec::new(),
        &config(provider, vec![exclusive], Arc::new(NoHooks)),
        &CancellationToken::new(),
        &NullSink,
    )
    .await
    .model_messages()
    .unwrap();
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
    let new = agent_loop(vec![user("go")], &mut Vec::new(), &cfg, &CancellationToken::new(), &NullSink)
        .await
        .model_messages()
        .unwrap();
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
    let new = agent_loop(vec![user("go")], &mut Vec::new(), &cfg, &CancellationToken::new(), &NullSink)
        .await
        .model_messages()
        .unwrap();
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
    let new = agent_loop(vec![user("go")], &mut Vec::new(), &cfg, &CancellationToken::new(), &NullSink)
        .await
        .model_messages()
        .unwrap();
    assert!(new.iter().any(|m| matches!(m, Message::User(u) if u.content.plain_text() == "late steer")), "{new:?}");
}

#[tokio::test]
async fn panicking_tool_becomes_error_result() {
    struct Boom;
    #[async_trait]
    impl AgentTool for Boom {
        fn definition(&self) -> Tool {
            Tool {
                name: "boom".into(),
                description: String::new(),
                parameters: json!({"type": "object"}),
                ..Default::default()
            }
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
    .await
    .model_messages()
    .unwrap();
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
    let new = agent_loop(vec![user("go")], &mut Vec::new(), &config(provider, vec![tool], hooks), &cancel, &NullSink)
        .await
        .model_messages()
        .unwrap();
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
        ..Default::default()
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
    let new = agent_loop(vec![user("go")], &mut Vec::new(), &cfg, &CancellationToken::new(), &NullSink)
        .await
        .model_messages()
        .unwrap();
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
        &mut vec![user("q").into(), Message::Assistant(done).into()],
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
    let mut ctx: Vec<AgentMessage> = vec![user("q").into(), Message::Assistant(tail).into()];
    let err = agent_loop_continue(&mut ctx.clone(), &cfg, &CancellationToken::new(), &NullSink, UnpairedTail::Refuse)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("effects are unknown"), "{err}");
    assert!(log.lock().unwrap().is_empty(), "refused tail never executes");
    let new = agent_loop_continue(&mut ctx, &cfg, &CancellationToken::new(), &NullSink, UnpairedTail::Execute)
        .await
        .unwrap()
        .model_messages()
        .unwrap();
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
    let agent = Agent::new(cfg, Vec::new());
    let new = agent
        .prompt(vec![user("use echo")], CancellationToken::new(), Arc::new(NullSink))
        .await
        .unwrap()
        .model_messages()
        .unwrap();
    assert_eq!(agent.messages().await.unwrap(), new);
    assert_eq!(*log.lock().unwrap(), vec!["start:call_1", "end:call_1"]);
    assert_eq!(new.last().unwrap().as_assistant().unwrap().text(), "all done");
    let reqs = server.requests.lock().await;
    let msgs = reqs[1]["body"]["messages"].as_array().unwrap();
    assert_eq!(msgs[2]["tool_calls"][0]["id"], json!("call_1"));
    assert_eq!(msgs[3], json!({"role": "tool", "content": "echo: wire", "tool_call_id": "call_1"}));
}

#[tokio::test]
async fn real_responses_http_chain_rejects_invalid_legacy_arguments_without_an_effect() {
    use ara_testkit::{FakeUpstream, Script};
    let script: Script = serde_json::from_value(json!({"responses": [
        {"events": [
            {"data":{"type":"response.output_item.done","output_index":0,
                "item":{"type":"function_call","id":"fc_bad","call_id":"call_bad",
                    "name":"legacy_schema","arguments":"{\"item\":\"ok\",\"pair\":[\"a\",1,\"extra\"]}"}}},
            {"data":{"type":"response.output_item.done","output_index":1,
                "item":{"type":"function_call","id":"fc_good","call_id":"call_good",
                    "name":"legacy_schema","arguments":"{\"item\":\"ok\",\"pair\":[\"a\",1]}"}}},
            {"data":{"type":"response.completed","response":{"status":"completed"}}}
        ]},
        {"events": [
            {"data":{"type":"response.output_item.done","output_index":0,
                "item":{"type":"message","content":[{"type":"output_text","text":"checked"}]}}},
            {"data":{"type":"response.completed","response":{"status":"completed"}}}
        ]}
    ]}))
    .unwrap();
    let server = FakeUpstream::start(script, None).await.unwrap();
    let provider = Arc::new(OpenAIResponsesProvider { client: reqwest::Client::new(), base: Default::default() });
    let effects = Arc::new(Mutex::new(Vec::new()));
    let tool: Arc<dyn AgentTool> = Arc::new(LegacySchemaTool { effects: effects.clone() });
    let mut cfg = config(provider, vec![tool], Arc::new(NoHooks));
    cfg.model.api = "openai-responses".into();
    cfg.model.base_url = server.base_url();
    let agent = Agent::new(cfg, Vec::new());
    let new = agent
        .prompt(vec![user("check the pair")], CancellationToken::new(), Arc::new(NullSink))
        .await
        .unwrap()
        .model_messages()
        .unwrap();
    let results = new
        .iter()
        .filter_map(|message| match message {
            Message::ToolResult(result) => Some(result),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 2);
    assert!(results[0].is_error);
    assert_eq!(results[0].tool_call_id, "call_bad|fc_bad");
    assert!(!results[1].is_error);
    assert_eq!(results[1].tool_call_id, "call_good|fc_good");
    assert_eq!(*effects.lock().unwrap(), vec![json!({"item":"ok","pair":["a",1]})]);
    assert_eq!(new.last().unwrap().as_assistant().unwrap().text(), "checked");
    let requests = server.requests.lock().await;
    assert_eq!(requests.len(), 2);
    let tool_schema = &requests[0]["body"]["tools"][0]["parameters"];
    assert_eq!(tool_schema["properties"]["pair"]["prefixItems"], json!([{"type":"string"},{"type":"integer"}]));
    assert_eq!(tool_schema["properties"]["pair"]["items"], false);
    let next_input = requests[1]["body"]["input"].as_array().unwrap();
    assert!(next_input.iter().any(|item| item["type"] == "function_call_output" && item["call_id"] == "call_bad"));
    assert!(next_input.iter().any(|item| item["type"] == "function_call_output" && item["call_id"] == "call_good"));
}

#[tokio::test]
async fn real_responses_wire_postprocess_matches_tool_argument_validation() {
    use ara_testkit::{FakeUpstream, Script};
    let script: Script = serde_json::from_value(json!({"responses": [
        {"events": [
            {"data":{"type":"response.output_item.done","output_index":0,
                "item":{"type":"function_call","id":"fc_invalid","call_id":"call_invalid",
                    "name":"schema_wire","arguments":"{\"skip\":\"bad\",\"mode\":\"a\",\"flag\":true}"}}},
            {"data":{"type":"response.output_item.done","output_index":1,
                "item":{"type":"function_call","id":"fc_valid","call_id":"call_valid",
                    "name":"schema_wire","arguments":"{\"skip\":null,\"mode\":\"b\",\"flag\":true}"}}},
            {"data":{"type":"response.completed","response":{"status":"completed"}}}
        ]},
        {"events": [
            {"data":{"type":"response.output_item.done","output_index":0,
                "item":{"type":"message","content":[{"type":"output_text","text":"checked"}]}}},
            {"data":{"type":"response.completed","response":{"status":"completed"}}}
        ]}
    ]}))
    .unwrap();
    let server = FakeUpstream::start(script, None).await.unwrap();
    let provider = Arc::new(OpenAIResponsesProvider { client: reqwest::Client::new(), base: Default::default() });
    let effects = Arc::new(Mutex::new(Vec::new()));
    let tool: Arc<dyn AgentTool> = Arc::new(WireSchemaTool { effects: effects.clone() });
    let mut cfg = config(provider, vec![tool], Arc::new(NoHooks));
    cfg.model.api = "openai-responses".into();
    cfg.model.base_url = server.base_url();
    let agent = Agent::new(cfg, Vec::new());
    let messages = agent
        .prompt(vec![user("validate wire arguments")], CancellationToken::new(), Arc::new(NullSink))
        .await
        .unwrap()
        .model_messages()
        .unwrap();
    let results = messages
        .iter()
        .filter_map(|message| match message {
            Message::ToolResult(result) => Some(result),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 2);
    assert!(results[0].is_error);
    assert!(!results[1].is_error);
    assert_eq!(*effects.lock().unwrap(), vec![json!({"skip":null,"mode":"b","flag":true})]);
    assert_eq!(messages.last().unwrap().as_assistant().unwrap().text(), "checked");
    let requests = server.requests.lock().await;
    assert_eq!(requests.len(), 2);
    let schema = &requests[0]["body"]["tools"][0]["parameters"];
    assert_eq!(schema["properties"]["skip"]["type"], json!(["integer", "null"]));
    assert_eq!(schema["properties"]["mode"], json!({"type":"string","enum":["a","b"],"description":"mode"}));
    assert_eq!(schema["properties"]["flag"], json!({"type":"boolean","enum":[true,false]}));
    let input = requests[1]["body"]["input"].as_array().unwrap();
    assert!(input.iter().any(|item| item["type"] == "function_call_output" && item["call_id"] == "call_invalid"));
    assert!(input.iter().any(|item| item["type"] == "function_call_output" && item["call_id"] == "call_valid"));
}

#[tokio::test]
async fn real_anthropic_wire_postprocess_matches_tool_argument_validation() {
    use ara_testkit::{FakeUpstream, Script};

    fn frame(value: Value) -> Value {
        let name = value["type"].as_str().unwrap();
        json!({"raw":format!("event: {name}\ndata: {value}\n\n")})
    }

    let script: Script = serde_json::from_value(json!({"responses":[
        {"events":[
            frame(json!({"type":"message_start","message":{"id":"msg_tools"}})),
            frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_bad","name":"schema_wire","input":{}}})),
            frame(json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"skip\":\"bad\",\"mode\":\"a\",\"flag\":true}"}})),
            frame(json!({"type":"content_block_stop","index":0})),
            frame(json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_good","name":"schema_wire","input":{}}})),
            frame(json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"skip\":null,\"mode\":\"b\",\"flag\":true}"}})),
            frame(json!({"type":"content_block_stop","index":1})),
            frame(json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}})),
            frame(json!({"type":"message_stop"}))
        ]},
        {"events":[
            frame(json!({"type":"message_start","message":{"id":"msg_final"}})),
            frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"checked"}})),
            frame(json!({"type":"content_block_stop","index":0})),
            frame(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}})),
            frame(json!({"type":"message_stop"}))
        ]}
    ]}))
    .unwrap();
    let server = FakeUpstream::start(script, None).await.unwrap();
    let provider = Arc::new(AnthropicMessagesProvider {
        client: reqwest::Client::new(),
        base: ara_ai::providers::anthropic::StreamOptions {
            api_key: Some("anthropic-test-secret".into()),
            ..Default::default()
        },
    });
    let effects = Arc::new(Mutex::new(Vec::new()));
    let tool: Arc<dyn AgentTool> = Arc::new(WireSchemaTool { effects: effects.clone() });
    let mut cfg = config(provider, vec![tool], Arc::new(NoHooks));
    cfg.model.api = "anthropic-messages".into();
    cfg.model.base_url = server.base_url();
    let agent = Agent::new(cfg, Vec::new());
    let messages = agent
        .prompt(vec![user("validate wire arguments")], CancellationToken::new(), Arc::new(NullSink))
        .await
        .unwrap()
        .model_messages()
        .unwrap();
    let results = messages
        .iter()
        .filter_map(|message| match message {
            Message::ToolResult(result) => Some(result),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].tool_call_id, "toolu_bad");
    assert!(results[0].is_error);
    assert_eq!(results[1].tool_call_id, "toolu_good");
    assert!(!results[1].is_error);
    assert_eq!(*effects.lock().unwrap(), vec![json!({"skip":null,"mode":"b","flag":true})]);
    assert_eq!(messages.last().unwrap().as_assistant().unwrap().text(), "checked");
    let requests = server.requests.lock().await;
    assert_eq!(requests.len(), 2);
    let schema = &requests[0]["body"]["tools"][0]["input_schema"];
    assert_eq!(schema["properties"]["skip"]["type"], json!(["integer", "null"]));
    assert_eq!(schema["properties"]["mode"], json!({"type":"string","enum":["a","b"],"description":"mode"}));
    assert_eq!(schema["properties"]["flag"], json!({"type":"boolean","enum":[true,false]}));
    let content = requests[1]["body"]["messages"].as_array().unwrap();
    let tool_results = content
        .iter()
        .flat_map(|message| message["content"].as_array().into_iter().flatten())
        .filter(|block| block["type"] == "tool_result")
        .collect::<Vec<_>>();
    assert_eq!(tool_results.len(), 2);
    assert_eq!(tool_results[0]["tool_use_id"], "toolu_bad");
    assert_eq!(tool_results[1]["tool_use_id"], "toolu_good");
}

#[tokio::test]
async fn agent_replays_responses_tool_history_with_paired_anthropic_wire_ids() {
    use ara_testkit::{FakeUpstream, Script};

    let frame = |value: Value| {
        let name = value["type"].as_str().unwrap();
        json!({"raw":format!("event: {name}\ndata: {value}\n\n")})
    };
    let script: Script = serde_json::from_value(json!({"responses":[{"events":[
        frame(json!({"type":"message_start","message":{"id":"msg_replay"}})),
        frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"continued"}})),
        frame(json!({"type":"content_block_stop","index":0})),
        frame(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}})),
        frame(json!({"type":"message_stop"}))
    ]}]}))
    .unwrap();
    let server = FakeUpstream::start(script, None).await.unwrap();
    let provider = Arc::new(AnthropicMessagesProvider {
        client: reqwest::Client::new(),
        base: ara_ai::providers::anthropic::StreamOptions {
            api_key: Some("anthropic-test-secret".into()),
            ..Default::default()
        },
    });
    let mut cfg = config(provider, Vec::new(), Arc::new(NoHooks));
    cfg.model.api = "anthropic-messages".into();
    cfg.model.base_url = server.base_url();
    cfg.max_model_calls = Some(1);
    let mut previous = AssistantMessage::empty("openai-responses", "openai", "responses-fixture");
    previous.content = vec![AssistantBlock::ToolCall(call_block("call_1|fc_A", "read", &json!({"path":"a"})))];
    previous.stop_reason = StopReason::ToolUse;
    let agent = Agent::new(
        cfg,
        vec![
            user("read a"),
            Message::Assistant(previous),
            Message::ToolResult(ToolResultMessage {
                tool_call_id: "call_1|fc_B".into(),
                tool_name: "read".into(),
                content: vec![UserBlock::text("file contents")],
                details: None,
                is_error: false,
                timestamp: 1,
                provider_metadata: None,
                useless: false,
            }),
        ],
    );
    let report = agent.prompt(vec![user("continue")], CancellationToken::new(), Arc::new(NullSink)).await.unwrap();
    assert_eq!(report.end, RunEnd::Completed);
    assert_eq!(report.model_messages().unwrap().last().unwrap().as_assistant().cloned().unwrap().text(), "continued");
    assert_eq!(server.served(), 1);
    let requests = server.requests.lock().await;
    let messages = requests[0]["body"]["messages"].as_array().unwrap();
    let call = messages
        .iter()
        .find_map(|message| message["content"].as_array()?.iter().find(|block| block["type"] == "tool_use"))
        .unwrap();
    let result = messages
        .iter()
        .find_map(|message| message["content"].as_array()?.iter().find(|block| block["type"] == "tool_result"))
        .unwrap();
    assert_eq!(call["id"], "call_1_fc_A");
    assert_eq!(result["tool_use_id"], call["id"]);
    assert_eq!(result["content"][0]["text"], "file contents");
}

#[tokio::test]
async fn strict_anthropic_fallback_executes_one_tool_and_correlates_followup() {
    use ara_testkit::{FakeUpstream, Script};

    fn frame(value: Value) -> Value {
        let name = value["type"].as_str().unwrap();
        json!({"raw":format!("event: {name}\ndata: {value}\n\n")})
    }

    let script: Script = serde_json::from_value(json!({"responses":[
        {"status":400,"body":json!({"error":{"type":"invalid_request_error","message":"The compiled grammar is too large"}}).to_string()},
        {"events":[
            frame(json!({"type":"message_start","message":{"id":"msg_tool","usage":{"input_tokens":5,"output_tokens":0}}})),
            frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_edit","name":"edit","input":{}}})),
            frame(json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"safe.txt\"}"}})),
            frame(json!({"type":"content_block_stop","index":0})),
            frame(json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":3}})),
            frame(json!({"type":"message_stop"}))
        ]},
        {"events":[
            frame(json!({"type":"message_start","message":{"id":"msg_final","usage":{"input_tokens":7,"output_tokens":0}}})),
            frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"done"}})),
            frame(json!({"type":"content_block_stop","index":0})),
            frame(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}})),
            frame(json!({"type":"message_stop"}))
        ]}
    ]}))
    .unwrap();
    let server = FakeUpstream::start(script, None).await.unwrap();
    let provider = Arc::new(AnthropicMessagesProvider {
        client: reqwest::Client::new(),
        base: ara_ai::providers::anthropic::StreamOptions {
            api_key: Some("anthropic-test-secret".into()),
            strict_tools: Some(true),
            provider_session_state: Some(Arc::new(Default::default())),
            ..Default::default()
        },
    });
    let effects = Arc::new(Mutex::new(Vec::new()));
    let tool: Arc<dyn AgentTool> = Arc::new(StrictEditTool { effects: effects.clone() });
    let mut cfg = config(provider, vec![tool], Arc::new(NoHooks));
    cfg.model.api = "anthropic-messages".into();
    cfg.model.base_url = server.base_url();
    cfg.max_model_calls = Some(2);
    let agent = Agent::new(cfg, Vec::new());
    let sink = Arc::new(RecordingSink::default());
    let report = agent.prompt(vec![user("edit safe.txt")], CancellationToken::new(), sink.clone()).await.unwrap();
    assert_eq!(report.end, RunEnd::Completed);
    assert_eq!(*effects.lock().unwrap(), vec![json!({"path":"safe.txt"})]);
    let results = report
        .model_messages()
        .unwrap()
        .into_iter()
        .filter_map(|message| match message {
            Message::ToolResult(result) => Some(result),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].tool_call_id, "toolu_edit");
    assert!(!results[0].is_error);
    assert_eq!(result_text(&Message::ToolResult(results[0].clone())), "edited");
    assert_eq!(report.model_messages().unwrap().last().unwrap().as_assistant().cloned().unwrap().text(), "done");
    let events = sink.events.lock().await;
    assert_eq!(events.iter().filter(|event| matches!(event, AgentEvent::ToolExecutionStart { .. })).count(), 1);
    assert_eq!(events.iter().filter(|event| matches!(event, AgentEvent::ToolExecutionEnd { .. })).count(), 1);
    drop(events);
    assert_eq!(server.served(), 3);
    let requests = server.requests.lock().await;
    assert_eq!(requests[0]["body"]["tools"][0]["strict"], true);
    assert!(requests[1]["body"]["tools"][0].get("strict").is_none());
    assert!(requests[2]["body"]["tools"][0].get("strict").is_none());
    assert_eq!(requests[0]["body"]["messages"], requests[1]["body"]["messages"]);
    let followup = requests[2]["body"]["messages"].as_array().unwrap();
    assert!(followup.iter().any(|message| {
        message["content"]
            .as_array()
            .is_some_and(|blocks| blocks.iter().any(|block| block["type"] == "tool_use" && block["id"] == "toolu_edit"))
    }));
    assert!(followup.iter().any(|message| message["content"].as_array().is_some_and(|blocks| {
        blocks.iter().any(|block| block["type"] == "tool_result" && block["tool_use_id"] == "toolu_edit")
    })));
}

#[tokio::test]
async fn model_call_budget_stops_before_the_next_call() {
    let provider = ScriptedProvider::new(vec![
        reply("", &[("c1", "echo", json!({"text": "x"}))], StopReason::ToolUse),
        reply("never", &[], StopReason::Stop),
    ]);
    let (tool, log) = echo(1, Concurrency::Shared);
    let mut cfg = config(provider.clone(), vec![tool], Arc::new(NoHooks));
    cfg.max_model_calls = Some(1);
    let new = agent_loop(vec![user("go")], &mut Vec::new(), &cfg, &CancellationToken::new(), &NullSink)
        .await
        .model_messages()
        .unwrap();
    assert_eq!(provider.contexts.lock().unwrap().len(), 1);
    assert_eq!(*log.lock().unwrap(), vec!["start:c1", "end:c1"], "calls of the last allowed turn still run");
    assert_eq!(new.last().unwrap().role(), "toolResult");
}

#[tokio::test]
async fn run_end_reasons() {
    let run = |turns: Vec<Turn>, deadline: Option<Duration>, budget: Option<usize>| async move {
        let (tool, _) = echo(300, Concurrency::Shared);
        let mut cfg = config(ScriptedProvider::new(turns), vec![tool], Arc::new(NoHooks));
        cfg.deadline = deadline.map(|d| Instant::now() + d);
        cfg.max_model_calls = budget;
        agent_loop(vec![user("go")], &mut Vec::new(), &cfg, &CancellationToken::new(), &NullSink).await.end
    };
    let tool_turn = || reply("", &[("c", "echo", json!({"text": "x"}))], StopReason::ToolUse);
    assert_eq!(run(vec![reply("ok", &[], StopReason::Stop)], None, None).await, RunEnd::Completed);
    assert_eq!(run(vec![tool_turn()], None, Some(1)).await, RunEnd::ModelCallBudget);
    assert_eq!(run(vec![], None, Some(0)).await, RunEnd::ModelCallBudget, "zero budget makes no call");
    assert_eq!(
        run(vec![tool_turn(), tool_turn()], Some(Duration::from_millis(100)), None).await,
        RunEnd::Deadline,
        "deadline hit during a tool"
    );
    assert_eq!(run(vec![Turn::HangAfterPartial], Some(Duration::from_millis(100)), None).await, RunEnd::Deadline);
    assert_eq!(run(vec![Turn::ErrorAfterToolCall("boom".into())], None, None).await, RunEnd::Error);
    let cancel = CancellationToken::new();
    cancel.cancel();
    let cfg = config(ScriptedProvider::new(vec![]), vec![], Arc::new(NoHooks));
    assert_eq!(agent_loop(vec![user("go")], &mut Vec::new(), &cfg, &cancel, &NullSink).await.end, RunEnd::Aborted);
}

// Legacy assertions explicitly inspect owned model snapshots; new raw families
// below continue to assert canonical record identity and lossless bodies.
fn ended_model_message(event: &AgentEvent) -> Option<Message> {
    match event {
        AgentEvent::MessageEnd { message } => message.checked_model_snapshot().unwrap(),
        _ => None,
    }
}
