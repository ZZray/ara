//! Fixed-OMP append-context module groups. Scripted ports prove Core boundaries;
//! the delivery runner separately exercises actual CLI/RPC processes and a bounded model task.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use ara_agent::append_only_context::{hash_utf16, message_digest, message_digest_projection, prefix_fingerprint};
use ara_agent::*;
use ara_ai::event::EventSink;
use ara_ai::*;
use async_trait::async_trait;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

fn user(text: &str) -> Message {
    Message::User(UserMessage::text(text))
}

fn model(id: &str, provider: &str) -> Model {
    Model {
        id: id.into(),
        provider: provider.into(),
        api: "openai-completions".into(),
        base_url: "http://fixture.invalid".into(),
        reasoning: false,
        max_tokens: None,
        context_window: None,
        tokenizer: None,
    }
}

fn call(id: &str, name: &str, arguments: Value) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        arguments: arguments.as_object().unwrap().clone(),
        ..Default::default()
    }
}

#[derive(Clone)]
struct Turn {
    text: String,
    calls: Vec<ToolCall>,
    incomplete: Option<ToolCall>,
    stop: StopReason,
}

#[derive(Default)]
struct Provider {
    turns: Mutex<Vec<Turn>>,
    contexts: Mutex<Vec<(Model, Context, Option<ToolChoice>)>>,
}

impl ModelProvider for Provider {
    fn stream(&self, model: &Model, context: &Context, options: CallOptions) -> AssistantStream {
        self.contexts.lock().unwrap().push((model.clone(), context.clone(), options.tool_choice));
        let turn = {
            let mut turns = self.turns.lock().unwrap();
            if turns.is_empty() {
                Turn { text: "done".into(), calls: Vec::new(), incomplete: None, stop: StopReason::Stop }
            } else {
                turns.remove(0)
            }
        };
        let mut message = AssistantMessage::empty(&model.api, &model.provider, &model.id);
        let (sink, stream) = EventSink::channel();
        tokio::spawn(async move {
            sink.push(AssistantMessageEvent::Start { partial: message.clone() }).await;
            if !turn.text.is_empty() {
                message.content.push(AssistantBlock::text(turn.text));
            }
            for tool_call in turn.calls {
                let content_index = message.content.len();
                message.content.push(AssistantBlock::ToolCall(tool_call.clone()));
                sink.push(AssistantMessageEvent::ToolcallEnd { content_index, tool_call, partial: message.clone() })
                    .await;
            }
            if let Some(tool_call) = turn.incomplete {
                let content_index = message.content.len();
                message.content.push(AssistantBlock::ToolCall(tool_call));
                sink.push(AssistantMessageEvent::ToolcallStart { content_index, partial: message.clone() }).await;
            }
            message.stop_reason = turn.stop;
            if matches!(turn.stop, StopReason::Error | StopReason::Aborted) {
                message.error_message = Some("fixture terminal".into());
                sink.push(AssistantMessageEvent::Error { reason: turn.stop, error: message }).await;
            } else {
                sink.push(AssistantMessageEvent::Done { reason: turn.stop, message }).await;
            }
        });
        stream
    }
}

struct ProbeTool {
    definition: Tool,
    mode: ToolIntentMode,
    effects: Arc<Mutex<Vec<(String, JsonObject)>>>,
    derived: AtomicUsize,
    panic_derive: bool,
}

impl ProbeTool {
    fn new(name: &str, mode: ToolIntentMode) -> Arc<Self> {
        Arc::new(Self {
            definition: Tool {
                name: name.into(),
                description: "probe".into(),
                parameters: json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":false}),
                ..Default::default()
            },
            mode,
            effects: Arc::new(Mutex::new(Vec::new())),
            derived: AtomicUsize::new(0),
            panic_derive: false,
        })
    }
}

#[async_trait]
impl AgentTool for ProbeTool {
    fn definition(&self) -> Tool {
        self.definition.clone()
    }
    fn intent_mode(&self) -> ToolIntentMode {
        self.mode
    }
    fn derive_intent(&self, args: &JsonObject) -> Option<String> {
        self.derived.fetch_add(1, Ordering::SeqCst);
        assert!(!args.contains_key("i"));
        if self.panic_derive {
            panic!("derived fixture failure");
        }
        Some(format!("  derived {}.  ", args.get("text").and_then(Value::as_str).unwrap_or("")))
    }
    fn concurrency(&self, args: &JsonObject) -> Concurrency {
        assert!(!args.contains_key("i"));
        Concurrency::Shared
    }
    async fn execute(
        &self,
        id: &str,
        args: JsonObject,
        _cancel: CancellationToken,
        _update: UpdateFn,
    ) -> Result<ToolOutput, ToolError> {
        self.effects.lock().unwrap().push((id.into(), args));
        Ok(ToolOutput::text("executed"))
    }
}

fn config(provider: Arc<dyn ModelProvider>, tools: Vec<Arc<dyn AgentTool>>, hooks: Arc<dyn LoopHooks>) -> AgentConfig {
    AgentConfig {
        model: model("initial", "fixture"),
        provider,
        system_prompt: vec!["initial prompt".into()],
        tools,
        tool_choice: None,
        max_tokens: None,
        temperature: None,
        deadline: None,
        max_model_calls: Some(1),
        hooks,
        interrupt_mode: ara_agent::InterruptMode::Immediate,
    }
}

#[test]
fn cache_and_source_projection_group() {
    upgraded_original_prefix_bank();
    // Original StablePrefix/manager/log case bank (upstream tests:45-502).
    // Keep these transitions in the module entrance, rather than 33 micro-tests.
    {
        let options = BuildOptions::default();
        let mut read = Arc::try_unwrap(ProbeTool::new("read", ToolIntentMode::Require)).ok().unwrap();
        read.definition.description = "Original desc".into();
        let read: Arc<dyn AgentTool> = Arc::new(read);
        let write: Arc<dyn AgentTool> = ProbeTool::new("write", ToolIntentMode::Require);
        let prompt = vec!["Old prompt".into()];
        let tools = vec![read.clone()];
        let mut prefix = StablePrefix::default();
        assert_eq!(prefix.version(), 0);
        assert_eq!(prefix.fingerprint(), "<unbuilt>");
        assert_eq!(prefix.to_context().unwrap_err(), ProviderContextError::PrefixNotBuilt);
        assert!(prefix.build(&prompt, &tools, &options).unwrap());
        assert!(prefix.built());
        assert_eq!(prefix.version(), 1);
        let (cached_prompt, cached_tools) = prefix.to_context().unwrap();
        assert_eq!(cached_prompt, prompt);
        assert_eq!(cached_tools[0].name, "read");
        let old_fingerprint = prefix.fingerprint().to_owned();
        assert!(!prefix.build(&prompt.clone(), &tools.clone(), &options).unwrap());
        assert_eq!(prefix.fingerprint(), old_fingerprint);
        assert_eq!(prefix.version(), 1);
        let changed_prompt = vec!["New prompt".into()];
        assert!(prefix.build(&changed_prompt, &tools, &options).unwrap());
        assert_ne!(prefix.fingerprint(), old_fingerprint);
        assert_eq!(prefix.version(), 2);
        assert!(!prefix.build(&changed_prompt, &tools, &options).unwrap());
        assert_eq!(prefix.version(), 2);
        let before_tools = prefix.fingerprint().to_owned();
        let two_tools = vec![read.clone(), write.clone()];
        assert!(prefix.build(&changed_prompt, &two_tools, &options).unwrap());
        assert_ne!(prefix.fingerprint(), before_tools);
        let mut changed_read = Arc::try_unwrap(ProbeTool::new("read", ToolIntentMode::Require)).ok().unwrap();
        changed_read.definition.description = "Updated desc".into();
        let changed_tools: Vec<Arc<dyn AgentTool>> = vec![Arc::new(changed_read), write.clone()];
        let before_description = prefix.fingerprint().to_owned();
        assert!(prefix.build(&changed_prompt, &changed_tools, &options).unwrap());
        assert_ne!(prefix.fingerprint(), before_description);
        prefix.invalidate();
        assert!(!prefix.built());
        assert!(prefix.build(&changed_prompt, &changed_tools, &options).unwrap());
        let mut identical = StablePrefix::default();
        identical.build(&changed_prompt, &changed_tools, &options).unwrap();
        assert_eq!(identical.fingerprint(), prefix.fingerprint());
        let before_order = prefix.fingerprint().to_owned();
        prefix.build(&changed_prompt, &[changed_tools[1].clone(), changed_tools[0].clone()], &options).unwrap();
        assert_ne!(prefix.fingerprint(), before_order);
        prefix.build(&["A".into(), "B".into()], &tools, &options).unwrap();
        let separate = prefix.fingerprint().to_owned();
        prefix.build(&["A\nB".into()], &tools, &options).unwrap();
        assert_ne!(prefix.fingerprint(), separate, "prompt array shape, not joined text, is hashed");

        let mut log = AppendOnlyLog::default();
        assert!(log.is_empty());
        assert!(log.to_messages().is_empty());
        log.replace_tail(user("no-op"));
        assert!(log.is_empty());
        log.append(user("hello"));
        let mut assistant = AssistantMessage::empty("a", "p", "m");
        assistant.content.push(AssistantBlock::text("world"));
        log.append(Message::Assistant(assistant));
        assert_eq!(log.len(), 2);
        assert_eq!(log.entries()[0], user("hello"));
        let mut copy = log.to_messages();
        copy.pop();
        assert_eq!(log.len(), 2, "toMessages array mutation does not change the log");
        log.replace_tail(user("new"));
        assert_eq!(log.len(), 2);
        assert_eq!(log.entries()[1], user("new"));
        log.clear();
        assert!(log.is_empty());
        log.extend([user("a"), user("b")]);
        assert_eq!(log.len(), 2);
        assert_eq!(log.entries(), &[user("a"), user("b")]);

        let mut manager = AppendOnlyContextManager::new();
        let first = manager.build(&prompt, &tools, &options).unwrap();
        assert_eq!(first.system_prompt, prompt);
        assert_eq!(first.tools.as_ref().unwrap()[0].name, "read");
        assert!(first.messages.is_empty());
        let repeated = manager.build(&prompt, &tools, &options).unwrap();
        assert_eq!(repeated.system_prompt, first.system_prompt);
        assert_eq!(repeated.tools, first.tools);
        assert_eq!(manager.build(&changed_prompt, &tools, &options).unwrap().system_prompt, changed_prompt);
        let before = manager.prefix().fingerprint().to_owned();
        manager.build(&changed_prompt, &two_tools, &options).unwrap();
        assert_ne!(manager.prefix().fingerprint(), before);
        manager.append_message(user("q1"));
        let first_turn = manager.build(&prompt, &tools, &options).unwrap();
        assert_eq!(first_turn.messages, vec![user("q1")]);
        manager.append_message(user("a1"));
        assert_eq!(manager.build(&prompt, &tools, &options).unwrap().messages, vec![user("q1"), user("a1")]);
        manager.replace_tail_message(user("new"));
        assert_eq!(manager.build(&prompt, &tools, &options).unwrap().messages, vec![user("q1"), user("new")]);
        manager.invalidate();
        assert!(!manager.prefix().built());
        assert_eq!(manager.build(&changed_prompt, &tools, &options).unwrap().system_prompt, changed_prompt);
        manager.reset(&["Fresh start".into()], &tools, &options).unwrap();
        assert_eq!(manager.to_context().unwrap().system_prompt, vec!["Fresh start"]);
        assert!(manager.to_context().unwrap().messages.is_empty());
        let empty = manager.build(&prompt, &[], &options).unwrap();
        assert!(empty.tools.unwrap().is_empty());
        let mut bare = Arc::try_unwrap(ProbeTool::new("bare", ToolIntentMode::Require)).ok().unwrap();
        bare.definition.description.clear();
        assert!(manager.build(&prompt, &[Arc::new(bare)], &options).unwrap().tools.unwrap()[0].description.is_empty());
        manager.reset_sync_cursor();
        manager.sync_messages(&[user("old")]).unwrap();
        let before_noop = manager.log().to_messages();
        manager.sync_messages(&[user("old")]).unwrap();
        assert_eq!(manager.log().entries(), before_noop);
        manager.reset_sync_cursor();
        assert!(manager.build(&prompt,&[],&options).unwrap().messages.is_empty(),
            "b91 reset empties build immediately, before the next sync");
        manager.sync_messages(&[user("fresh")]).unwrap();
        assert_eq!(manager.to_context().unwrap().messages, vec![user("fresh")]);
    }
    assert_eq!(hash_utf16("😀"), 1_772_899, "JS iterates UTF-16 code units");
    assert_eq!(message_digest(&Value::Null), 0);
    assert_eq!(message_digest(&json!(false)), 0);
    assert_ne!(message_digest(&json!([])), 0, "JS Array is an object with a null field projection");
    assert_eq!(
        message_digest_projection(&json!({"role":"r","content":"c","providerPayload":{"x":1},
        "toolCalls":null,"tool_calls":[1],"toolCallId":null,"tool_call_id":"s","toolName":null,"name":"n",
        "isError":false,"is_error":true,"id":"k"}))
        .unwrap(),
        json!({"r":"r","c":"c","pp":{"x":1},"tc":[1],"tcid":"s","tn":"n","err":false,"id":"k"})
    );
    assert_eq!(message_digest_projection(&json!({"is_error":true})).unwrap()["err"], Value::Null);

    let tool: Arc<dyn AgentTool> = ProbeTool::new("read", ToolIntentMode::Require);
    let tools = vec![tool];
    let options = BuildOptions::default();
    let prompt = vec!["stable 😀".to_string()];
    let mut manager = AppendOnlyContextManager::new();
    assert!(manager.to_context().is_err());
    let context = manager.build(&prompt, &tools, &options).unwrap();
    assert!(context.messages.is_empty());
    assert_eq!(manager.prefix().version(), 1);
    manager.build(&prompt, &tools, &options).unwrap();
    assert_eq!(manager.prefix().version(), 1);
    let mut messages = vec![user("first"), user("second")];
    manager.sync_messages(&messages).unwrap();
    messages.push(user("third"));
    manager.sync_messages(&messages).unwrap();
    assert_eq!(manager.log().entries(), messages.as_slice());
    messages[1] = user("rewritten");
    manager.sync_messages(&messages).unwrap();
    assert_eq!(manager.log().entries(), messages.as_slice());
    messages[1] = user("second");
    manager.sync_messages(&messages).unwrap();
    assert_eq!(manager.log().entries(), messages.as_slice(), "revert invalidates the owned slot memo");
    let mut assistant = AssistantMessage::empty("a", "p", "m");
    assistant.provider_payload = Some(serde_json::from_str(r#"{"a":1,"b":2}"#).unwrap());
    messages.push(Message::Assistant(assistant.clone()));
    manager.sync_messages(&messages).unwrap();
    assistant.provider_payload = Some(serde_json::from_str(r#"{"b":2,"a":1}"#).unwrap());
    messages[3] = Message::Assistant(assistant);
    manager.sync_messages(&messages).unwrap();
    assert_eq!(
        ara_prompt::js::json_stringify(
            manager.log().entries()[3].as_assistant().unwrap().provider_payload.as_ref().unwrap()
        ),
        r#"{"b":2,"a":1}"#,
        "key-order-only rewrite is not serde Value equality"
    );
    manager.log_mut().clear();
    manager.sync_messages(&messages).unwrap();
    assert_eq!(manager.sync_cursor(), 4);
    assert_eq!(manager.log().len(), 4, "public clear cannot leave an advanced cursor");
    messages.truncate(2);
    manager.sync_messages(&messages).unwrap();
    assert_eq!(manager.log().entries(), messages.as_slice());
    manager.append_message(user("manual"));
    manager.replace_tail_message(user("replacement"));
    assert_eq!(manager.log().len(), 3);
    manager.log_mut().truncate(-1);
    manager.log_mut().replace_tail(user("empty no-op"));
    assert!(manager.log().is_empty());
    manager.log_mut().extend(messages.clone());
    assert_eq!(manager.log().to_messages(), messages);
    manager.reset_sync_cursor();
    assert_eq!(manager.sync_cursor(), 0);
    manager.invalidate();
    assert!(!manager.prefix().built());
    manager.reset(&prompt, &tools, &options).unwrap();
    assert!(manager.prefix().built());
    manager.sync_messages(&messages).unwrap();
    manager.invalidate_for_model_change();
    assert!(!manager.prefix().built());
    assert!(manager.log().is_empty());

    let normalized = normalize_tools(&tools, &options).unwrap();
    let mut explicit_false = options;
    explicit_false.prune_tool_descriptions = Some(false);
    assert_ne!(
        prefix_fingerprint(&prompt, &normalized, &options),
        prefix_fingerprint(&prompt, &normalized, &explicit_false)
    );
    let mut strict = normalized.clone();
    strict[0].strict = Some(false);
    assert_ne!(prefix_fingerprint(&prompt, &normalized, &options), prefix_fingerprint(&prompt, &strict, &options));
    strict[0].custom_wire_name = Some("wire".into());
    strict[0].custom_format =
        Some(CustomToolFormat { syntax: CustomToolSyntax::Lark, definition: "start: /.+/".into() });
    assert_ne!(prefix_fingerprint(&prompt, &normalized, &options), prefix_fingerprint(&prompt, &strict, &options));
    let mut numbers = normalized.clone();
    numbers[0].parameters = serde_json::from_str(r#"{"2":2,"1":1.0,"x":1}"#).unwrap();
    let mut reordered = numbers.clone();
    reordered[0].parameters = serde_json::from_str(r#"{"1":1,"2":2,"x":1}"#).unwrap();
    assert_eq!(
        prefix_fingerprint(&prompt, &numbers, &options),
        prefix_fingerprint(&prompt, &reordered, &options),
        "JS integer keys sort numerically and 1.0 stringifies as 1"
    );

    // Same existing coarse entrance: original Graph append state and callbacks.
    // Controlled Core observations; not a production Host/real-model receipt.
    {
        use ara_agent::append_only_context::actual::{
            clear_log_actual, log_entries_actual, prepare_append_only_actual, publish_owner_estimate_version_actual,
            sync_cursor_actual,
        };
        use ara_rpc::{
            WireString,
            value_heap::{
                CallableTarget, CapabilityId, NoRuntime, PropertyDescriptor, RuntimeReply, RuntimeRequest, ValueError,
                ValueHandle, ValueHeap, ValueRuntime, ValueSnapshot,
            },
        };
        use std::sync::{
            Weak,
            atomic::{AtomicBool, AtomicI32},
        };

        struct GraphRuntime {
            heap: Arc<ValueHeap>,
            manager: SharedAppendOnlyContextManager,
            values: Mutex<Weak<SessionValues>>,
            trace: Arc<Mutex<Vec<&'static str>>>,
            content: Mutex<ValueHandle>,
            thrown: ValueHandle,
            version: AtomicI32,
            writes: AtomicUsize,
            fail_content: AtomicBool,
            reenter: AtomicBool,
            reenter_role: AtomicBool,
            reentries: AtomicUsize,
            ark_schema: Mutex<Option<ValueHandle>>,
            ark_materializations: AtomicUsize,
        }
        impl ValueRuntime for GraphRuntime {
            fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
                let RuntimeRequest::Call { capability: CapabilityId(capability), arguments, receiver } = request else {
                    return Err(ValueError::MissingCapability(request.capability()));
                };
                if matches!(capability, 861 | 862 | 863) {
                    let value = match capability {
                        861 => {
                            self.trace.lock().unwrap().push("ark-toJsonSchema-get");
                            self.heap.function(CapabilityId(863))?
                        }
                        862 => {
                            self.trace.lock().unwrap().push("ark-assert-get");
                            self.heap.function(CapabilityId(864))?
                        }
                        863 => {
                            self.trace.lock().unwrap().push("ark-toJsonSchema-call");
                            assert!(receiver.same_identity(self.ark_schema.lock().unwrap().as_ref().unwrap()));
                            self.ark_materializations.fetch_add(1, Ordering::SeqCst);
                            let options = arguments.first().expect("Ark conversion options");
                            let target = self.heap.get(options, &"target".into(), &NoRuntime)?;
                            assert!(matches!(self.heap.snapshot(&target)?, ValueSnapshot::String(text)
                                if text.units() == "draft-2020-12".encode_utf16().collect::<Vec<_>>().as_slice()));
                            let fallback = self.heap.get(options, &"fallback".into(), &NoRuntime)?;
                            let base = self.heap.symbol(Some("Ark fallback original base".into()))?;
                            let context = self.heap.object()?;
                            self.heap.define_own_data(&context, "base".into(), base.clone(), &NoRuntime)?;
                            assert!(self.heap.call(&fallback, &self.heap.undefined()?, &[context], self)?.same_identity(&base));
                            // Controlled callable result, generated by the actual
                            // conversion Call; the input remains a Function.
                            self.heap.import_serde(&json!({"$schema":"https://json-schema.org/draft/2020-12/schema",
                                "type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},
                                "required":["path","content"]}))?
                        }
                        _ => unreachable!(),
                    };
                    return Ok(RuntimeReply::Value(value));
                }
                assert!(!matches!(capability, 860 | 864), "normalization must not invoke the Ark validator");
                if capability == 850 {
                    self.trace.lock().unwrap().push("version-write");
                    let Some(argument) = arguments.first() else {
                        return Err(ValueError::WrongRuntimeReply("version setter argument"));
                    };
                    let ValueSnapshot::Number(version) = self.heap.snapshot(argument)? else {
                        return Err(ValueError::WrongRuntimeReply("numeric signed32 version"));
                    };
                    self.version.store(version as i32, Ordering::SeqCst);
                    self.writes.fetch_add(1, Ordering::SeqCst);
                    return Ok(RuntimeReply::Value(self.heap.undefined()?));
                }
                let name = match capability {
                    801 => "version",
                    802 => "role",
                    803 => "content",
                    804 => "providerPayload",
                    805 => "toolCalls",
                    806 => "tool_calls",
                    807 => "toolCallId",
                    808 => "tool_call_id",
                    809 => "toolName",
                    810 => "name",
                    811 => "isError",
                    812 => "id",
                    813 => "is_error",
                    _ => return Err(ValueError::MissingCapability(CapabilityId(capability))),
                };
                self.trace.lock().unwrap().push(name);
                let value = match capability {
                    801 => {
                        if self.reenter.swap(false, Ordering::SeqCst) {
                            // No runtime/trace lock is held over this same-manager reset.
                            self.manager.lock().unwrap().reset_sync_cursor();
                            let values = self.values.lock().unwrap().upgrade().expect("selected Session values");
                            assert_eq!(sync_cursor_actual(&self.manager, &values).unwrap(), 0);
                            self.trace.lock().unwrap().push("reenter-reset");
                            self.reentries.fetch_add(1, Ordering::SeqCst);
                        }
                        self.heap.number(f64::from(self.version.load(Ordering::SeqCst)))?
                    }
                    802 => {
                        if self.reenter_role.swap(false, Ordering::SeqCst) {
                            self.manager.lock().unwrap().reset_sync_cursor();
                            self.trace.lock().unwrap().push("reenter-role-reset");
                            self.reentries.fetch_add(1, Ordering::SeqCst);
                        }
                        self.heap.string("assistant".into())?
                    }
                    803 => {
                        if self.fail_content.swap(false, Ordering::SeqCst) {
                            return Err(ValueError::Thrown(self.thrown.clone()));
                        }
                        self.content.lock().unwrap().clone()
                    }
                    812 => self.heap.string("alias-id".into())?,
                    _ => self.heap.null()?,
                };
                Ok(RuntimeReply::Value(value))
            }
        }
        struct GraphOrderTool {
            inner: Arc<ProbeTool>,
            trace: Arc<Mutex<Vec<&'static str>>>,
        }
        #[async_trait]
        impl AgentTool for GraphOrderTool {
            fn definition(&self) -> Tool {
                self.trace.lock().unwrap().push("normalize");
                self.inner.definition()
            }
            async fn execute(
                &self,
                _: &str,
                _: JsonObject,
                _: CancellationToken,
                _: UpdateFn,
            ) -> Result<ToolOutput, ToolError> {
                panic!("append context observation cannot execute a tool")
            }
        }

        let heap = ValueHeap::new().unwrap();
        let manager = Arc::new(Mutex::new(AppendOnlyContextManager::new()));
        let trace = Arc::new(Mutex::new(Vec::new()));
        let runtime = Arc::new(GraphRuntime {
            heap: heap.clone(),
            manager: manager.clone(),
            values: Mutex::new(Weak::new()),
            trace: trace.clone(),
            content: Mutex::new(heap.string("before".into()).unwrap()),
            thrown: heap.symbol(Some("digest-fault".into())).unwrap(),
            version: AtomicI32::new(0),
            writes: AtomicUsize::new(0),
            fail_content: AtomicBool::new(false),
            reenter: AtomicBool::new(false),
            reenter_role: AtomicBool::new(false),
            reentries: AtomicUsize::new(0),
            ark_schema: Mutex::new(None),
            ark_materializations: AtomicUsize::new(0),
        });
        let values = Arc::new(SessionValues::new(heap.clone(), runtime.clone()).unwrap());
        *runtime.values.lock().unwrap() = Arc::downgrade(&values);
        let version_key =
            heap.module_symbol("ara-agent.compaction.message-cache", "omp.messageEstimateVersion").unwrap();
        let make_observed = || {
            let message = heap.object().unwrap();
            for (key, capability) in [
                ("role", 802),
                ("content", 803),
                ("providerPayload", 804),
                ("toolCalls", 805),
                ("tool_calls", 806),
                ("toolCallId", 807),
                ("tool_call_id", 808),
                ("toolName", 809),
                ("name", 810),
                ("isError", 811),
                ("id", 812),
                ("is_error", 813),
            ] {
                heap.define_own(
                    &message,
                    key.into(),
                    PropertyDescriptor::Accessor {
                        get: Some(CallableTarget::Host(CapabilityId(capability))),
                        set: None,
                        enumerable: true,
                        configurable: true,
                    },
                    &NoRuntime,
                )
                .unwrap();
            }
            heap.define_own(
                &message,
                version_key.clone(),
                PropertyDescriptor::Accessor {
                    get: Some(CallableTarget::Host(CapabilityId(801))),
                    set: Some(CallableTarget::Host(CapabilityId(850))),
                    enumerable: true,
                    configurable: true,
                },
                &NoRuntime,
            )
            .unwrap();
            message
        };
        let get = |object: &ValueHandle, key: &str| heap.get(object, &key.into(), &NoRuntime).unwrap();
        let first = make_observed();
        let second = heap.import_serde(&json!({"role":"user","content":"tail"})).unwrap();
        let tools: Vec<Arc<dyn AgentTool>> = vec![Arc::new(GraphOrderTool {
            inner: ProbeTool::new("read", ToolIntentMode::Require),
            trace: trace.clone(),
        })];
        let prompt = vec!["stable Graph prefix".to_owned()];
        let options = BuildOptions::default();
        let prepare = |messages: &[ValueHandle]| {
            prepare_append_only_actual(&manager, &prompt, &tools, messages, &options, &values)
        };
        let original = prepare(&[first.clone()]).unwrap();
        let root = original.original_context.as_ref().unwrap();
        let original_prompt = get(root, "systemPrompt");
        let original_tools = get(root, "tools");
        let original_log = log_entries_actual(&manager, &values).unwrap();
        let first_trace = trace.lock().unwrap().clone();
        assert_eq!(
            &first_trace[..13],
            &[
                "version",
                "role",
                "content",
                "providerPayload",
                "toolCalls",
                "tool_calls",
                "toolCallId",
                "tool_call_id",
                "toolName",
                "name",
                "isError",
                "id",
                "normalize",
            ],
            "sync field/alias Get order finishes before tool normalization"
        );
        assert!(!first_trace.contains(&"is_error"), "snake error field is not an original digest selector");
        assert!(original.messages[0].same_identity(&first));
        assert_eq!(runtime.writes.load(Ordering::SeqCst), 0, "initial memo does not publish a version");

        trace.lock().unwrap().clear();
        let repeated = prepare(&[first.clone()]).unwrap();
        assert!(get(repeated.original_context.as_ref().unwrap(), "systemPrompt").same_identity(&original_prompt));
        assert!(get(repeated.original_context.as_ref().unwrap(), "tools").same_identity(&original_tools));
        assert!(repeated.messages[0].same_identity(&first));
        let repeat_trace = trace.lock().unwrap().clone();
        assert_eq!(repeat_trace.iter().filter(|entry| **entry == "version").count(), 1);
        assert!(!repeat_trace.iter().any(|entry| matches!(*entry, "role" | "content" | "providerPayload")));
        assert_eq!(runtime.writes.load(Ordering::SeqCst), 0);

        prepare(&[first.clone(), second.clone()]).unwrap();
        let changed_tail = heap.import_serde(&json!({"role":"user","content":"rewritten tail"})).unwrap();
        let rewritten = prepare(&[first.clone(), changed_tail.clone()]).unwrap();
        assert!(
            log_entries_actual(&manager, &values).unwrap().same_identity(&original_log),
            "truncate preserves the log array"
        );
        assert!(rewritten.messages[0].same_identity(&first), "deep rewrite keeps prior original members");
        assert!(rewritten.messages[1].same_identity(&changed_tail));
        assert!(get(rewritten.original_context.as_ref().unwrap(), "tools").same_identity(&original_tools));

        // Same original member mutated in place: signed32 adapter invalidates its
        // identity memo; no typed/serde clone can stand in for that root.
        let raw_units = vec![0xd83d, 0xde00, 0xd800];
        let content = heap.string(WireString::from_units(raw_units.clone())).unwrap();
        *runtime.content.lock().unwrap() = content.clone();
        publish_owner_estimate_version_actual(&first, u64::from(u32::MAX), &values).unwrap();
        assert_eq!(runtime.version.load(Ordering::SeqCst), -1);
        assert_eq!(runtime.writes.load(Ordering::SeqCst), 1);
        trace.lock().unwrap().clear();
        let invalidated = prepare(&[first.clone(), changed_tail.clone()]).unwrap();
        let invalidate_trace = trace.lock().unwrap().clone();
        assert_eq!(invalidate_trace.iter().filter(|entry| **entry == "role").count(), 1);
        assert_eq!(invalidate_trace.iter().filter(|entry| **entry == "content").count(), 1);
        assert_eq!(
            invalidate_trace.iter().filter(|entry| **entry == "version").count(),
            2,
            "prefix miss then appended-tail memo hit"
        );
        assert!(invalidated.messages[0].same_identity(&first));
        assert!(
            matches!(heap.snapshot(&content).unwrap(), ValueSnapshot::String(text) if text.units() == raw_units.as_slice())
        );

        manager.lock().unwrap().reset_sync_cursor();
        assert_eq!(sync_cursor_actual(&manager, &values).unwrap(), 0);
        trace.lock().unwrap().clear();
        let replayed = prepare(&[first.clone(), changed_tail.clone()]).unwrap();
        assert!(!log_entries_actual(&manager, &values).unwrap().same_identity(&original_log));
        assert!(get(replayed.original_context.as_ref().unwrap(), "systemPrompt").same_identity(&original_prompt));
        assert!(!trace.lock().unwrap().iter().any(|entry| matches!(*entry, "role" | "content")));
        let prefix_version = manager.lock().unwrap().prefix().version();
        manager.lock().unwrap().invalidate_for_model_change();
        trace.lock().unwrap().clear();
        let model_reset = prepare(&[first.clone(), changed_tail.clone()]).unwrap();
        assert!(!get(model_reset.original_context.as_ref().unwrap(), "systemPrompt").same_identity(&original_prompt));
        assert_eq!(manager.lock().unwrap().prefix().version(), prefix_version + 1);
        assert!(
            !trace.lock().unwrap().iter().any(|entry| matches!(*entry, "role" | "content")),
            "model reset retains digest memo"
        );

        let before_clear = log_entries_actual(&manager, &values).unwrap();
        clear_log_actual(&manager, &values).unwrap();
        assert_eq!(sync_cursor_actual(&manager, &values).unwrap(), 2, "public log clear keeps the advanced cursor");
        let after_clear = log_entries_actual(&manager, &values).unwrap();
        assert!(!after_clear.same_identity(&before_clear));
        let after_clear_context = prepare(&[first.clone(), changed_tail.clone()]).unwrap();
        assert_eq!(after_clear_context.messages.len(), 2);
        assert!(log_entries_actual(&manager, &values).unwrap().same_identity(&after_clear));

        let failed = make_observed();
        runtime.fail_content.store(true, Ordering::SeqCst);
        trace.lock().unwrap().clear();
        let failure = prepare(&[first.clone(), changed_tail.clone(), failed.clone()]).unwrap_err();
        assert!(matches!(failure, ProviderContextError::Actual(error)
            if matches!(error.as_ref(), ActualProviderError::Value(ValueError::Thrown(thrown)) if thrown.same_identity(&runtime.thrown))));
        let partial = log_entries_actual(&manager, &values).unwrap();
        assert_eq!(heap.array_length(&partial, &NoRuntime).unwrap(), 3, "append occurs before thrown digest");
        assert!(get(&partial, "2").same_identity(&failed));
        assert_eq!(sync_cursor_actual(&manager, &values).unwrap(), 2, "failure does not publish the final cursor");
        let failure_trace = trace.lock().unwrap().clone();
        assert!(!failure_trace.contains(&"normalize"), "failed sync must not normalize/build");
        assert!(failure_trace.ends_with(&["version", "role", "content"]), "no later field Get after exact throw");

        manager.lock().unwrap().reset_sync_cursor();
        prepare(&[first.clone(), changed_tail.clone()]).unwrap();
        runtime.reenter.store(true, Ordering::SeqCst);
        trace.lock().unwrap().clear();
        let reentered = prepare(&[first.clone(), changed_tail.clone()]).unwrap();
        assert_eq!(runtime.reentries.load(Ordering::SeqCst), 1);
        assert!(trace.lock().unwrap().contains(&"reenter-reset"));
        assert!(reentered.messages[0].same_identity(&first));
        assert!(reentered.messages[1].same_identity(&changed_tail));
        assert_eq!(
            sync_cursor_actual(&manager, &values).unwrap(),
            2,
            "outer sync observes reset and repopulates the live log"
        );
        assert!(
            !trace.lock().unwrap().iter().any(|entry| matches!(*entry, "role" | "content")),
            "memo survives same-manager callback reset"
        );
        assert_eq!(runtime.writes.load(Ordering::SeqCst), 1, "ordinary builds/resets do not write owner generations");

        // A new first-tail digest captures its push receiver before role Get.
        // reset replaces that receiver; h1 must not be pushed into live [h2].
        let tail_manager = Arc::new(Mutex::new(AppendOnlyContextManager::new()));
        let tail_trace = Arc::new(Mutex::new(Vec::new()));
        let tail_runtime = Arc::new(GraphRuntime {
            heap: heap.clone(),
            manager: tail_manager.clone(),
            values: Mutex::new(Weak::new()),
            trace: tail_trace.clone(),
            content: Mutex::new(heap.string("m1".into()).unwrap()),
            thrown: runtime.thrown.clone(),
            version: AtomicI32::new(0),
            writes: AtomicUsize::new(0),
            fail_content: AtomicBool::new(false),
            reenter: AtomicBool::new(false),
            reenter_role: AtomicBool::new(true),
            reentries: AtomicUsize::new(0),
            ark_schema: Mutex::new(None),
            ark_materializations: AtomicUsize::new(0),
        });
        let tail_values = Arc::new(SessionValues::new(heap.clone(), tail_runtime.clone()).unwrap());
        *tail_runtime.values.lock().unwrap() = Arc::downgrade(&tail_values);
        let m1 = make_observed();
        let m2 = heap.import_serde(&json!({"role":"user","content":"m2"})).unwrap();
        let first_tail =
            prepare_append_only_actual(&tail_manager, &prompt, &[], &[m1.clone(), m2.clone()], &options, &tail_values)
                .unwrap();
        assert_eq!(tail_runtime.reentries.load(Ordering::SeqCst), 1);
        assert!(tail_trace.lock().unwrap().contains(&"reenter-role-reset"));
        assert_eq!(first_tail.messages.len(), 1, "reset during m1 digest leaves only m2 in the replacement log");
        assert!(first_tail.messages[0].same_identity(&m2));
        assert_eq!(sync_cursor_actual(&tail_manager, &tail_values).unwrap(), 2);
        let second_tail =
            prepare_append_only_actual(&tail_manager, &prompt, &[], &[m1.clone(), m2.clone()], &options, &tail_values)
                .unwrap();
        assert_eq!(second_tail.messages.len(), 2);
        assert!(
            second_tail.messages[0].same_identity(&m1),
            "captured old digest receiver cannot make the next sync keep m2 as m1"
        );
        assert!(second_tail.messages[1].same_identity(&m2), "second sync has no duplicated m2");
        // Original message-sync case bank (tests:515-850,967): compare original
        // members, not exported equality. A metadata-only change must replace
        // the suffix even when visible content has not changed.
        for (before, after) in [
            (
                json!({"role":"toolResult","content":[{"type":"text","text":"same output"}],
                    "toolCallId":"old-call","toolName":"read","isError":false}),
                json!({"role":"toolResult","content":[{"type":"text","text":"same output"}],
                    "toolCallId":"new-call","toolName":"write","isError":true}),
            ),
            (
                json!({"role":"assistant","content":[{"type":"text","text":"same visible output"}],
                    "id":"assistant-1","providerPayload":{"type":"openaiResponsesHistory","provider":"openai",
                    "items":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"old native"}]}]}}),
                json!({"role":"assistant","content":[{"type":"text","text":"same visible output"}],
                    "id":"assistant-1","providerPayload":{"type":"openaiResponsesHistory","provider":"openai",
                    "items":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"new native"}]}]}}),
            ),
        ] {
            let sequence_manager = Arc::new(Mutex::new(AppendOnlyContextManager::new()));
            let prefix_member = heap.import_serde(&json!({"role":"user","content":"q1"})).unwrap();
            let previous = heap.import_serde(&before).unwrap();
            let tail = heap.import_serde(&json!({"role":"user","content":"q2"})).unwrap();
            let initial = prepare_append_only_actual(&sequence_manager, &prompt, &[],
                &[prefix_member.clone(), previous.clone(), tail.clone()], &options, &values).unwrap();
            assert_eq!(initial.messages.len(), 3);
            let fresh_prefix = heap.import_serde(&json!({"role":"user","content":"q1"})).unwrap();
            let changed = heap.import_serde(&after).unwrap();
            let changed_tail = heap.import_serde(&json!({"role":"user","content":"q2-rewritten"})).unwrap();
            let result = prepare_append_only_actual(&sequence_manager, &prompt, &[],
                &[fresh_prefix, changed.clone(), changed_tail.clone()], &options, &values).unwrap();
            assert_eq!(result.messages.len(), 3);
            assert!(result.messages[0].same_identity(&prefix_member));
            assert!(result.messages[1].same_identity(&changed));
            assert!(!result.messages[1].same_identity(&previous));
            assert!(result.messages[2].same_identity(&changed_tail), "metadata divergence replaces the later suffix");
            for field in ["toolCallId", "toolName", "isError", "providerPayload"] {
                if let Some(expected) = after.get(field) {
                    let actual = get(&result.messages[1], field);
                    // Only the already-JSON fixture field is inspected; the
                    // owner/member identity assertions above stay original.
                    assert_eq!(heap.export_serde(&actual).unwrap(), *expected);
                }
            }
        }

        let sequence_manager = Arc::new(Mutex::new(AppendOnlyContextManager::new()));
        let originals_json = vec![json!({"role":"user","content":"q1"}), json!({"role":"assistant","content":"a1"})];
        let originals: Vec<_> = originals_json.iter().map(|value| heap.import_serde(value).unwrap()).collect();
        let prepare_sequence = |messages: &[ValueHandle]| {
            prepare_append_only_actual(&sequence_manager, &prompt, &[], messages, &options, &values).unwrap()
        };
        let initial = prepare_sequence(&originals);
        let copies: Vec<_> = originals_json.iter().map(|value| heap.import_serde(value).unwrap()).collect();
        let unchanged = prepare_sequence(&copies);
        for index in 0..2 {
            assert!(unchanged.messages[index].same_identity(&originals[index]), "fresh equal bytes retain prior members");
            assert!(!unchanged.messages[index].same_identity(&copies[index]));
        }
        let pruned = heap.import_serde(&json!({"role":"assistant","content":"[pruned]"})).unwrap();
        let new_tail = heap.import_serde(&json!({"role":"user","content":"q2"})).unwrap();
        let grown = prepare_sequence(&[copies[0].clone(), pruned.clone(), new_tail.clone()]);
        assert!(grown.messages[0].same_identity(&originals[0]));
        assert!(grown.messages[1].same_identity(&pruned));
        assert!(grown.messages[2].same_identity(&new_tail));
        let rewritten_tail = heap.import_serde(&json!({"role":"user","content":"q2-rewritten"})).unwrap();
        let rewritten = prepare_sequence(&[copies[0].clone(), pruned.clone(), rewritten_tail.clone()]);
        assert!(rewritten.messages[0].same_identity(&originals[0]));
        assert!(rewritten.messages[1].same_identity(&pruned));
        assert!(rewritten.messages[2].same_identity(&rewritten_tail));
        let reverted = heap.import_serde(&originals_json[1]).unwrap();
        let replayed = prepare_sequence(&[copies[0].clone(), reverted.clone(), rewritten_tail.clone()]);
        assert!(replayed.messages[0].same_identity(&originals[0]));
        assert!(replayed.messages[1].same_identity(&reverted));
        assert!(replayed.messages[2].same_identity(&rewritten_tail));
        let compacted = prepare_sequence(&[new_tail.clone()]);
        assert_eq!(compacted.messages.len(), 1);
        assert!(compacted.messages[0].same_identity(&new_tail));
        let first_changed = heap.import_serde(&json!({"role":"user","content":"world"})).unwrap();
        assert!(prepare_sequence(&[first_changed.clone()]).messages[0].same_identity(&first_changed));
        let copied_array = get(initial.original_context.as_ref().unwrap(), "messages");
        let retained_log = log_entries_actual(&sequence_manager, &values).unwrap();
        assert!(!copied_array.same_identity(&retained_log));
        heap.array_set_length(&copied_array, 0, &NoRuntime).unwrap();
        assert_eq!(heap.array_length(&retained_log, &NoRuntime).unwrap(), 1, "returned array is a slice");
        assert!(log_entries_actual(&sequence_manager, &values).unwrap().same_identity(&retained_log));

        // In-place wire tool_calls mutation obeys the shared invalidation
        // contract stated by the fixed source; bump the original owner version.
        let wire_manager = Arc::new(Mutex::new(AppendOnlyContextManager::new()));
        let wire_prefix = heap.import_serde(&json!({"role":"user","content":"q"})).unwrap();
        let wire_assistant = heap.import_serde(&json!({"role":"assistant","content":null,
            "tool_calls":[{"id":"c1","type":"function","function":{"name":"read","arguments":"{\"path\":\"/a\"}"}}]})).unwrap();
        let wire_following = heap.import_serde(&json!({"role":"user","content":"same tail"})).unwrap();
        prepare_append_only_actual(&wire_manager, &prompt, &[],
            &[wire_prefix.clone(), wire_assistant.clone(), wire_following.clone()], &options, &values).unwrap();
        let function = get(&get(&get(&wire_assistant, "tool_calls"), "0"), "function");
        heap.assign(&function, "arguments".into(), heap.string("{\"path\":\"/b\"}".into()).unwrap(), &NoRuntime).unwrap();
        publish_owner_estimate_version_actual(&wire_assistant, 1, &values).unwrap();
        let fresh_following = heap.import_serde(&json!({"role":"user","content":"same tail"})).unwrap();
        let mutated = prepare_append_only_actual(&wire_manager, &prompt, &[],
            &[wire_prefix.clone(), wire_assistant.clone(), fresh_following.clone()], &options, &values).unwrap();
        assert_eq!(mutated.messages.len(), 3);
        assert!(mutated.messages[0].same_identity(&wire_prefix));
        assert!(mutated.messages[1].same_identity(&wire_assistant));
        assert!(mutated.messages[2].same_identity(&fresh_following), "wire-only digest change replaces the later suffix");
        assert!(matches!(heap.snapshot(&get(&function,"arguments")).unwrap(), ValueSnapshot::String(text)
            if text.units() == "{\"path\":\"/b\"}".encode_utf16().collect::<Vec<_>>().as_slice()));
        let restored = heap.import_serde(&json!({"role":"assistant","content":null,
            "tool_calls":[{"id":"c1","type":"function","function":{"name":"read","arguments":"{\"path\":\"/a\"}"}}]})).unwrap();
        let restored_context = prepare_append_only_actual(&wire_manager, &prompt, &[],
            &[wire_prefix.clone(), restored.clone(), fresh_following.clone()], &options, &values).unwrap();
        assert!(restored_context.messages[1].same_identity(&restored), "replay cannot retain a pre-invalidation digest");
        // Original long steady/growth/split-5/revert sequence (test:744).
        let long_manager = Arc::new(Mutex::new(AppendOnlyContextManager::new()));
        let mut authored = Vec::new();
        for index in 0..4 {
            authored.push(json!({"role":"user","content":format!("q{index}")}));
            authored.push(json!({"role":"assistant","content":format!("a{index}")}));
        }
        let mut members: Vec<_> = authored.iter().map(|value| heap.import_serde(value).unwrap()).collect();
        let prepare_long = |messages: &[ValueHandle]| {
            prepare_append_only_actual(&long_manager, &prompt, &[], messages, &options, &values).unwrap()
        };
        let first_long = prepare_long(&members);
        let steady_long = prepare_long(&members);
        for index in 0..8 {
            assert!(steady_long.messages[index].same_identity(&first_long.messages[index]));
        }
        members.push(heap.import_serde(&json!({"role":"assistant","content":"new turn"})).unwrap());
        let growth = prepare_long(&members);
        for index in 0..8 {
            assert!(growth.messages[index].same_identity(&first_long.messages[index]));
        }
        assert!(growth.messages[8].same_identity(&members[8]));
        let mut long_rewrite = members.clone();
        long_rewrite[5] = heap.import_serde(&json!({"role":"assistant","content":"[pruned]"})).unwrap();
        let split = prepare_long(&long_rewrite);
        for index in 0..5 {
            assert!(split.messages[index].same_identity(&first_long.messages[index]));
        }
        for index in 5..9 {
            assert!(split.messages[index].same_identity(&long_rewrite[index]));
        }
        let mut reverted_members: Vec<_> = authored.iter().map(|value| heap.import_serde(value).unwrap()).collect();
        reverted_members.push(heap.import_serde(&json!({"role":"assistant","content":"new turn"})).unwrap());
        let reverted_long = prepare_long(&reverted_members);
        for index in 0..5 {
            assert!(reverted_long.messages[index].same_identity(&first_long.messages[index]));
        }
        for index in 5..9 {
            assert!(reverted_long.messages[index].same_identity(&reverted_members[index]));
        }
        // Original owner content invalidation followed by a fresh-byte replay
        // (test:793), separate from the wire-only tool call above.
        let alias_manager = Arc::new(Mutex::new(AppendOnlyContextManager::new()));
        let alias = heap.import_serde(&json!({"role":"assistant","content":[{"type":"text","text":"original"}]})).unwrap();
        let initial_alias = prepare_append_only_actual(&alias_manager, &prompt, &[], &[alias.clone()], &options, &values).unwrap();
        assert!(initial_alias.messages[0].same_identity(&alias));
        let redacted = heap.import_serde(&json!([{"type":"text","text":"[redacted]"}])).unwrap();
        heap.assign(&alias, "content".into(), redacted.clone(), &NoRuntime).unwrap();
        publish_owner_estimate_version_actual(&alias, 1, &values).unwrap();
        let updated_alias = prepare_append_only_actual(&alias_manager, &prompt, &[], &[alias.clone()], &options, &values).unwrap();
        assert!(updated_alias.messages[0].same_identity(&alias));
        assert!(get(&updated_alias.messages[0], "content").same_identity(&redacted));
        let fresh_original = heap.import_serde(&json!({"role":"assistant","content":[{"type":"text","text":"original"}]})).unwrap();
        let replay = prepare_append_only_actual(&alias_manager, &prompt, &[], &[fresh_original.clone()], &options, &values).unwrap();
        assert!(replay.messages[0].same_identity(&fresh_original));
        assert!(!replay.messages[0].same_identity(&alias));
        // Original missing-description and Ark callable inputs now use the
        // production original-tool-source entrance, in this same coarse bank.
        struct OriginalTools(ValueHandle);
        impl ActualToolSource for OriginalTools {
            fn current_tools(&self, values: &SessionValues) -> Result<Option<ValueHandle>, ActualProviderError> {
                values.check_handle(&self.0)?;
                Ok(Some(self.0.clone()))
            }
            fn bind_selected(&self, _: &ValueHandle, _: &SessionValues) -> Result<ActualToolBinding, ActualProviderError> {
                Err(ActualProviderError::Unsupported("context-only fixture cannot execute"))
            }
        }
        let schema = heap.function(CapabilityId(860)).unwrap();
        for (key, capability) in [("toJsonSchema",861),("assert",862)] {
            heap.define_own(&schema, key.into(), PropertyDescriptor::Accessor {
                get: Some(CallableTarget::Host(CapabilityId(capability))), set: None,
                enumerable: true, configurable: true,
            }, &NoRuntime).unwrap();
        }
        *runtime.ark_schema.lock().unwrap() = Some(schema.clone());
        let original_tool = heap.import_serde(&json!({"name":"write","description":"Write","intent":"require"})).unwrap();
        heap.define_own_data(&original_tool,"parameters".into(),schema.clone(),&NoRuntime).unwrap();
        let bare = heap.import_serde(&json!({"name":"bare","parameters":{"type":"object","properties":{}}})).unwrap();
        assert!(!heap.has(&bare,&"description".into(),&NoRuntime).unwrap());
        let source = OriginalTools(heap.array(vec![original_tool.clone(),bare.clone()]).unwrap());
        let original_manager = Arc::new(Mutex::new(AppendOnlyContextManager::new()));
        let intent_options = BuildOptions { intent_tracing: true, ..Default::default() };
        let original_context = ara_agent::append_only_context::actual::prepare_append_only_original_actual(
            &original_manager,&prompt,&[],&[],&intent_options,&values,&source).unwrap();
        let original_tools = original_context.tools.as_ref().unwrap();
        assert_eq!(original_tools.len(),2);
        let properties = get(&get(&original_tools[0],"parameters"),"properties");
        let keys = heap.own_keys(&properties,&NoRuntime).unwrap();
        assert_eq!(keys, vec!["i".into(),"path".into(),"content".into()]);
        let required = get(&get(&original_tools[0],"parameters"),"required");
        assert!(heap.export_serde(&required).unwrap().as_array().unwrap().contains(&json!("i")));
        assert_eq!(heap.export_serde(&get(&original_tools[1],"description")).unwrap(),json!(""));
        assert!(get(&original_tool,"parameters").same_identity(&schema));
        assert!(!heap.has(&bare,&"description".into(),&NoRuntime).unwrap(), "normalization leaves the missing source field absent");
        assert_eq!(runtime.ark_materializations.load(Ordering::SeqCst),1);
        let cached_tool_array = get(original_context.original_context.as_ref().unwrap(),"tools");
        let repeated_original = ara_agent::append_only_context::actual::prepare_append_only_original_actual(
            &original_manager,&prompt,&[],&[],&intent_options,&values,&source).unwrap();
        assert_eq!(runtime.ark_materializations.load(Ordering::SeqCst),1,"same Ark Function reuses its wire schema stamp");
        assert!(get(repeated_original.original_context.as_ref().unwrap(),"tools").same_identity(&cached_tool_array));
        let graph_version=original_manager.lock().unwrap().prefix().version();
        let graph_fingerprint=original_manager.lock().unwrap().prefix().fingerprint().to_owned();
        assert!(original_manager.lock().unwrap().prefix().built());
        assert!(matches!(original_manager.lock().unwrap().prefix().to_context().unwrap_err(),
            ProviderContextError::Actual(error) if matches!(error.as_ref(),ActualProviderError::Unsupported(_))));
        let mut typed_write=Arc::try_unwrap(ProbeTool::new("write",ToolIntentMode::Require)).ok().unwrap();
        typed_write.definition.description="Write".into();
        typed_write.definition.parameters=json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},
            "required":["path","content"],"additionalProperties":false});
        let mut typed_bare=Arc::try_unwrap(ProbeTool::new("bare",ToolIntentMode::Require)).ok().unwrap();
        typed_bare.definition.description.clear();typed_bare.definition.parameters=json!({"type":"object","properties":{}});
        let same_bytes_tools:Vec<Arc<dyn AgentTool>>=vec![Arc::new(typed_write),Arc::new(typed_bare)];
        original_manager.lock().unwrap().build(&prompt,&same_bytes_tools,&intent_options).unwrap();
        assert_eq!(original_manager.lock().unwrap().prefix().fingerprint(),graph_fingerprint,"typed and original source domains may produce identical bytes");
        assert!(original_manager.lock().unwrap().prefix().to_context().is_ok());
        assert_eq!(original_manager.lock().unwrap().prefix().version(),graph_version+1);
        let typed_context=prepare_append_only_actual(&original_manager,&prompt,&same_bytes_tools,&[],&intent_options,&values).unwrap();
        assert!(!get(typed_context.original_context.as_ref().unwrap(),"tools").same_identity(&cached_tool_array),
            "equal fingerprint must not reuse a prefix from the other source domain");
        let rebuilt=ara_agent::append_only_context::actual::prepare_append_only_original_actual(
            &original_manager,&prompt,&[],&[],&intent_options,&values,&source).unwrap();
        assert_eq!(original_manager.lock().unwrap().prefix().version(),graph_version+2);
        assert!(!get(rebuilt.original_context.as_ref().unwrap(),"tools").same_identity(&cached_tool_array));
        assert_eq!(runtime.ark_materializations.load(Ordering::SeqCst),1,"domain switch retains same-schema wire cache");
    }
}

#[test]
fn normalizer_legacy_metadata_and_examples_group() {
    let legacy: Tool =
        serde_json::from_value(json!({"name":"legacy","description":"d","parameters":{"type":"object"}})).unwrap();
    let wire = serde_json::to_value(&legacy).unwrap();
    assert!(wire.get("strict").is_none() && wire.get("examples").is_none());
    let call: ToolCall = serde_json::from_value(json!({"id":"old","name":"legacy","arguments":{}})).unwrap();
    assert!(serde_json::to_value(call).unwrap().get("intent").is_none());
    let explicit: Tool = serde_json::from_value(
        json!({"name":"explicit","description":"d","parameters":{},"strict":false,"examples":[]}),
    )
    .unwrap();
    let wire = serde_json::to_value(explicit).unwrap();
    assert_eq!(wire["strict"], false);
    assert_eq!(wire["examples"], json!([]));

    // Original build() option/example cache transitions (tests:859-958).
    // Authored JSON order is exercised here. The original ArkType callable
    // materialization case remains separately OPEN; JSON is not its substitute.
    {
        let mut find = Arc::try_unwrap(ProbeTool::new("find", ToolIntentMode::Require)).ok().unwrap();
        find.definition.description = "Find files.".into();
        find.definition.parameters = serde_json::from_str(r#"{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}"#).unwrap();
        let without_examples: Arc<dyn AgentTool> = Arc::new(find);
        let mut find = Arc::try_unwrap(ProbeTool::new("find", ToolIntentMode::Require)).ok().unwrap();
        find.definition = without_examples.definition();
        find.definition.examples = Some(vec![ToolExample::Call {
            call: json!({"paths":["src/**/*.ts"]}).as_object().unwrap().clone(),
            caption: Some("Find files".into()),
        }]);
        let with_examples: Arc<dyn AgentTool> = Arc::new(find);
        let mut manager = AppendOnlyContextManager::new();
        let prompt = vec!["stable".into()];
        let no_intent = BuildOptions::default();
        let intent = BuildOptions { intent_tracing: true, ..Default::default() };
        let absent = manager.build(&prompt, &[without_examples.clone()], &no_intent).unwrap();
        let absent_fingerprint = manager.prefix().fingerprint().to_owned();
        assert!(absent.tools.as_ref().unwrap()[0].parameters["properties"].get("i").is_none());
        assert!(!absent.tools.as_ref().unwrap()[0].parameters["required"].as_array().unwrap().contains(&json!("i")));
        assert!(!absent.tools.as_ref().unwrap()[0].description.contains("<examples>"));
        let on = manager.build(&prompt, &[without_examples.clone()], &intent).unwrap();
        assert_ne!(manager.prefix().fingerprint(), absent_fingerprint);
        assert_eq!(ara_prompt::js::entries(on.tools.as_ref().unwrap()[0].parameters["properties"].as_object().unwrap())
            .into_iter().map(|(key,_)| key.as_str()).collect::<Vec<_>>(), vec!["i","path","content"]);
        assert!(on.tools.as_ref().unwrap()[0].parameters["required"].as_array().unwrap().contains(&json!("i")));
        manager.build(&prompt, &[without_examples], &no_intent).unwrap();
        assert_eq!(manager.prefix().fingerprint(), absent_fingerprint, "false/true/false returns to the original bytes");
        manager.invalidate();
        let examples = manager.build(&prompt, &[with_examples.clone()], &no_intent).unwrap();
        let examples_fingerprint = manager.prefix().fingerprint().to_owned();
        assert_ne!(examples_fingerprint, absent_fingerprint);
        let description = &examples.tools.as_ref().unwrap()[0].description;
        assert!(description.contains("<examples>"));
        assert!(description.contains("# Find files"));
        assert!(description.contains("find(paths=[\"src/**/*.ts\"])"));
        assert!(!description.contains("i=\"…\""));
        let examples_on = manager.build(&prompt, &[with_examples.clone()], &intent).unwrap();
        assert!(examples_on.tools.as_ref().unwrap()[0].description.contains("i=\"…\""));
        assert_ne!(manager.prefix().fingerprint(), examples_fingerprint);
        let repeated_version = manager.prefix().version();
        manager.build(&prompt, &[with_examples], &intent).unwrap();
        assert_eq!(manager.prefix().version(), repeated_version);
    }

    let mut probe = Arc::try_unwrap(ProbeTool::new("write", ToolIntentMode::Require)).ok().unwrap();
    probe.definition.parameters = serde_json::from_str(r#"{"type":"object","description":"root",
        "properties":{"text":{"type":"string","description":"text"},"description":{"type":"string","description":"annotation"}},
        "default":{"description":"literal"},"const":{"description":"literal"},"examples":[{"description":"literal"}]}"#).unwrap();
    probe.definition.examples = Some(vec![
        ToolExample::Call {
            call: json!({"text":"bare\npayload"}).as_object().unwrap().clone(),
            caption: Some("payload".into()),
        },
        ToolExample::Compare {
            bad: serde_json::from_str(r#"{"n":1,"text":"unsafe\n\"\"\""}"#).unwrap(),
            good: json!({"n":2,"text":"safe\npayload","ok":true,"none":null}).as_object().unwrap().clone(),
            caption: Some("compare".into()),
        },
        ToolExample::Note { caption: "note  ".into(), note: Some("remember".into()) },
    ]);
    let tool: Arc<dyn AgentTool> = Arc::new(probe);
    let tools = vec![tool];
    let options = BuildOptions { intent_tracing: true, ..Default::default() };
    let normalized = normalize_tools(&tools, &options).unwrap();
    let definition = &normalized[0];
    assert_eq!(ara_prompt::js::entries(definition.parameters["properties"].as_object().unwrap())[0].0, "i");
    assert!(definition.parameters["required"].as_array().unwrap().contains(&json!("i")));
    assert!(definition.description.contains("<example i=\"…\">\nbare\npayload\n</example>"));
    assert!(definition.description.contains("text=\"\"\"safe\npayload\"\"\""));
    assert!(definition.description.contains("ok=True, none=None"));
    assert!(definition.description.contains("WRONG:\n"));
    assert!(definition.description.contains("# note\nremember"));
    assert!(definition.description.contains("unsafe\\n\\\"\\\"\\\""), "unsafe fence uses escaped single-line literal");
    let jsdoc = render_tool_examples_jsdoc(&tools[0].definition());
    assert!(jsdoc.contains("@example \"payload\"\nbare\npayload"));
    assert!(jsdoc.contains("@example \"note  \"\nremember"));
    assert!(!jsdoc.contains("i=\"…\""));
    let pruned = normalize_tools(&tools, &BuildOptions { prune_tool_descriptions: Some(true), ..options }).unwrap();
    assert!(pruned[0].description.is_empty());
    assert!(pruned[0].parameters.get("description").is_none());
    assert_eq!(pruned[0].parameters["properties"]["description"]["type"], "string");
    assert!(pruned[0].parameters["properties"]["description"].get("description").is_none());
    for field in ["default", "const"] {
        assert_eq!(pruned[0].parameters[field]["description"], "literal");
    }
    assert_eq!(pruned[0].parameters["examples"][0]["description"], "literal");
    assert!(pruned[0].parameters["properties"]["i"].get("description").is_none());
    let suppressed = normalize_tools(&tools, &BuildOptions { suppress_intent_schema: true, ..options }).unwrap();
    assert!(suppressed[0].parameters["properties"].get("i").is_none());

    let mut union = Arc::try_unwrap(ProbeTool::new("union", ToolIntentMode::Optional)).ok().unwrap();
    union.definition.parameters = json!({"anyOf":[{"type":"object","properties":{"x":{"type":"string"}},"additionalProperties":false},
        {"type":"object","properties":{"y":{"type":"string"}},"additionalProperties":false}]});
    let union = normalize_tools(&[Arc::new(union)], &options).unwrap();
    assert!(union[0].parameters.get("properties").is_none());
    for branch in union[0].parameters["anyOf"].as_array().unwrap() {
        assert!(branch["properties"].get("i").is_some());
        assert!(branch.get("required").is_none_or(|value| !value.as_array().unwrap().contains(&json!("i"))));
    }
}

#[derive(Default)]
struct JournalSink {
    rows: Mutex<Vec<Value>>,
    events: RecordingSink,
}

#[async_trait]
impl AgentEventSink for JournalSink {
    async fn emit(&self, event: AgentEvent) {
        if let AgentEvent::MessageEnd { message } = &event {
            self.rows.lock().unwrap().push(serde_json::to_value(message).unwrap());
        }
        self.events.emit(event).await;
    }
}

struct LiveHooks {
    live: Mutex<ExecutionSnapshot>,
    admitted: Mutex<Option<ExecutionSnapshot>>,
    replacement_after_admission: Mutex<Option<ExecutionSnapshot>>,
    getters: AtomicUsize,
    snapshots: AtomicUsize,
    approvals: Mutex<Vec<(Option<String>, JsonObject)>>,
}

#[async_trait]
impl LoopHooks for LiveHooks {
    fn execution_snapshot(&self) -> Option<ExecutionSnapshot> {
        self.snapshots.fetch_add(1, Ordering::SeqCst);
        Some(self.live.lock().unwrap().clone())
    }
    fn provider_context_binding(&self) -> Option<ProviderContextBinding> {
        self.getters.fetch_add(1, Ordering::SeqCst);
        self.live.lock().unwrap().provider_context.clone()
    }
    fn set_append_only_context(
        &self,
        manager: Option<SharedAppendOnlyContextManager>,
    ) -> Result<(), ProviderContextError> {
        let mut snapshot = self.live.lock().unwrap();
        snapshot.provider_context.get_or_insert_with(ProviderContextBinding::default).append_only_context = manager;
        Ok(())
    }
    async fn before_model_call(
        &self,
        snapshot: ExecutionSnapshot,
        _cancel: &CancellationToken,
    ) -> Result<ExecutionSnapshot, AdmissionError> {
        let admitted = self.admitted.lock().unwrap().take();
        if let Some(admitted) = admitted {
            *self.live.lock().unwrap() = self.replacement_after_admission.lock().unwrap().take().unwrap();
            Ok(admitted)
        } else {
            Ok(snapshot)
        }
    }
    async fn before_tool_call(&self, call: &ToolCall, args: &JsonObject, _cancel: &CancellationToken) -> ToolDecision {
        self.approvals.lock().unwrap().push((call.intent.clone(), args.clone()));
        ToolDecision::Allow
    }
    async fn transform_provider_context(&self, mut context: Context, model: &Model) -> Context {
        context.system_prompt.push(format!("transformed {}", model.id));
        context
    }
}

#[tokio::test]
async fn loop_intent_atomic_admission_and_live_side_group() {
    upgraded_typed_owned_intent_bank().await;
    let required = ProbeTool::new("required", ToolIntentMode::Require);
    let derived = ProbeTool::new("derived", ToolIntentMode::Derived);
    let mut failure = Arc::try_unwrap(ProbeTool::new("failure", ToolIntentMode::Derived)).ok().unwrap();
    failure.panic_derive = true;
    let failure = Arc::new(failure);
    let provider = Arc::new(Provider {
        turns: Mutex::new(vec![Turn {
            text: String::new(),
            stop: StopReason::ToolUse,
            incomplete: None,
            calls: vec![
                call("r", "required", json!({"i":"\u{feff}  explicit...  ","text":"r"})),
                call("d", "derived", json!({"i":null,"text":"d"})),
                call("f", "failure", json!({"text":"f"})),
            ],
        }]),
        ..Default::default()
    });
    let manager = Arc::new(Mutex::new(AppendOnlyContextManager::new()));
    let later_manager = Arc::new(Mutex::new(AppendOnlyContextManager::new()));
    let base = config(provider.clone(), Vec::new(), Arc::new(NoHooks));
    let initial = ExecutionSnapshot::from_config(&base);
    let mut selected = initial.clone();
    selected.system_prompt = vec!["selected prompt".into()];
    selected.tools = vec![required.clone(), derived.clone(), failure.clone()];
    selected.model.as_mut().unwrap().model = model("selected", "fixture");
    selected.provider_context = Some(ProviderContextBinding {
        build_options: BuildOptions { intent_tracing: true, suppress_intent_schema: true, ..Default::default() },
        append_only_context: Some(manager.clone()),
        ..Default::default()
    });
    let mut later = initial.clone();
    later.system_prompt = vec!["later prompt".into()];
    later.tools = vec![required.clone()];
    later.model.as_mut().unwrap().model = model("later", "cerebras");
    later.provider_context =
        Some(ProviderContextBinding { append_only_context: Some(later_manager.clone()), ..Default::default() });
    let hooks = Arc::new(LiveHooks {
        live: Mutex::new(initial),
        admitted: Mutex::new(Some(selected)),
        replacement_after_admission: Mutex::new(Some(later)),
        getters: AtomicUsize::new(0),
        snapshots: AtomicUsize::new(0),
        approvals: Mutex::new(Vec::new()),
    });
    let mut cfg = base;
    cfg.hooks = hooks.clone();
    let agent = Agent::new(cfg.clone(), Vec::new());
    let journal = Arc::new(JournalSink::default());
    let report = agent.prompt(vec![user("execute")], CancellationToken::new(), journal.clone()).await.unwrap();
    assert_eq!(report.end, RunEnd::ModelCallBudget);
    {
        let requests = provider.contexts.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0.id, "selected");
        assert_eq!(requests[0].1.system_prompt, vec!["selected prompt", "transformed selected"]);
        assert!(
            requests[0].1.tools.as_ref().unwrap()[0].parameters["properties"].get("i").is_none(),
            "ARA_NO_INTENT suppresses only the schema; execution tracing remains active"
        );
    }
    assert_eq!(
        hooks.getters.load(Ordering::SeqCst),
        0,
        "dynamic snapshot never falls back to a live binding getter after admission"
    );
    assert_eq!(derived.derived.load(Ordering::SeqCst), 1);
    assert_eq!(failure.derived.load(Ordering::SeqCst), 1);
    for tool in [&required, &derived, &failure] {
        let effects = tool.effects.lock().unwrap();
        assert_eq!(effects.len(), 1);
        assert!(!effects[0].1.contains_key("i"));
    }
    {
        let approvals = hooks.approvals.lock().unwrap();
        assert_eq!(approvals[0].0.as_deref(), Some("explicit"));
        assert_eq!(approvals[1].0.as_deref(), Some("derived d."), "derived callback trims but does not strip periods");
        assert_eq!(approvals[2].0, None);
    }
    {
        let rows = journal.rows.lock().unwrap();
        let raw = rows.iter().find(|row| row["role"] == "assistant").unwrap();
        let restored: Message = serde_json::from_value(raw.clone()).unwrap();
        let calls: Vec<_> = restored.as_assistant().unwrap().tool_calls().collect();
        assert_eq!(calls[0].intent.as_deref(), Some("explicit"));
        assert!(calls[0].arguments.contains_key("i"), "journal preserves original raw arguments");
        assert_eq!(calls[1].intent.as_deref(), Some("derived d."));
    }
    let events = journal.events.events.lock().await;
    let start = events
        .iter()
        .find(|event| matches!(event,AgentEvent::ToolExecutionStart{tool_call_id,..} if tool_call_id=="r"))
        .unwrap();
    assert_eq!(start.full().unwrap()["intent"], "explicit");
    assert!(start.full().unwrap()["args"].get("i").is_none());
    drop(events);

    let first_cursor = manager.lock().unwrap().sync_cursor();
    let later_cursor = later_manager.lock().unwrap().sync_cursor();
    let mut thinking = AssistantMessage::empty("a", "cerebras", "later");
    thinking.content = vec![
        AssistantBlock::Thinking(ThinkingContent { thinking: "private".into(), thinking_signature: None }),
        AssistantBlock::text("visible"),
    ];
    let snapshot_reads = hooks.snapshots.load(Ordering::SeqCst);
    let side = agent.build_side_request_context(vec![Message::Assistant(thinking)], None).await.unwrap();
    assert_eq!(hooks.snapshots.load(Ordering::SeqCst), snapshot_reads + 1, "side uses one owned active snapshot");
    assert_eq!(side.system_prompt, vec!["later prompt", "transformed later"]);
    assert_eq!(side.messages[0].as_assistant().unwrap().content.len(), 1);
    assert_eq!(
        serde_json::to_value(&agent.messages().await.unwrap()[0]).unwrap()["content"],
        "execute",
        "Cerebras normalization leaves transcript untouched"
    );
    assert_eq!(manager.lock().unwrap().sync_cursor(), first_cursor);
    assert_eq!(later_manager.lock().unwrap().sync_cursor(), later_cursor);
    assert_eq!(
        provider.contexts.lock().unwrap().len(),
        1,
        "building side Context does not consume model choice or call network"
    );
    let replacement = Arc::new(Mutex::new(AppendOnlyContextManager::new()));
    agent.set_append_only_context(Some(replacement.clone())).unwrap();
    assert!(Arc::ptr_eq(&agent.append_only_context().unwrap(), &replacement));
    assert_eq!(agent.set_provider_context_binding(None).unwrap_err(), ProviderContextError::HostOwnedMutation);

    for stop in [StopReason::Error, StopReason::Aborted] {
        let tool = ProbeTool::new("terminal", ToolIntentMode::Require);
        let provider = Arc::new(Provider {
            turns: Mutex::new(vec![Turn {
                text: String::new(),
                stop,
                calls: vec![call("complete", "terminal", json!({"i":" terminal... ","text":"complete"}))],
                incomplete: Some(call("unfinished", "terminal", json!({"i":"must not persist","text":"partial"}))),
            }]),
            ..Default::default()
        });
        let cfg = config(
            provider,
            vec![tool.clone()],
            Arc::new(StaticHooks {
                binding: ProviderContextBinding {
                    build_options: BuildOptions { intent_tracing: true, ..Default::default() },
                    ..Default::default()
                },
            }),
        );
        let sink = JournalSink::default();
        let report = agent_loop(vec![user("terminal")], &mut Vec::new(), &cfg, &CancellationToken::new(), &sink).await;
        assert_eq!(report.end, if stop == StopReason::Aborted { RunEnd::Aborted } else { RunEnd::Error });
        let rows = sink.rows.lock().unwrap();
        let row = rows.iter().find(|row| row["role"] == "assistant").unwrap();
        assert_eq!(row["content"].as_array().unwrap().len(), 1);
        assert_eq!(
            row["content"][0]["intent"], "terminal",
            "retained completed calls annotate before error/abort MessageEnd"
        );
        assert!(tool.effects.lock().unwrap().is_empty(), "terminal failure does not execute a tool or imply success");
    }
}

struct StaticHooks {
    binding: ProviderContextBinding,
}
#[async_trait]
impl LoopHooks for StaticHooks {
    fn provider_context_binding(&self) -> Option<ProviderContextBinding> {
        Some(self.binding.clone())
    }
}

struct OwnedPort {
    sides: AtomicUsize,
    snapshots: Mutex<Vec<Arc<[Tool]>>>,
}
impl OwnedProviderContextPort for OwnedPort {
    fn stream_prepared(
        &self,
        provider: &dyn ModelProvider,
        model: &Model,
        context: &Context,
        tools: Arc<[Tool]>,
        options: CallOptions,
    ) -> AssistantStream {
        assert!(options.suppress_native_tool_choice);
        self.snapshots.lock().unwrap().push(tools);
        self.stream(provider, model, context, options)
    }
    fn project_context(
        &self,
        _model: &Model,
        mut context: Context,
        side_request: bool,
    ) -> Result<Context, ProviderContextError> {
        if side_request {
            self.sides.fetch_add(1, Ordering::SeqCst);
        }
        context.system_prompt.push(if side_request { "owned side".into() } else { "owned tools".into() });
        for message in &mut context.messages {
            if let Message::Assistant(assistant) = message {
                assistant.content = vec![AssistantBlock::text(serde_json::to_string(&assistant.content).unwrap())];
            }
        }
        Ok(context)
    }
    fn stream(
        &self,
        provider: &dyn ModelProvider,
        model: &Model,
        context: &Context,
        options: CallOptions,
    ) -> AssistantStream {
        assert!(context.tools.is_none());
        assert!(options.tool_choice.is_none());
        let mut upstream = provider.stream(model, context, options);
        let (sink, stream) = EventSink::channel();
        tokio::spawn(async move {
            while let Some(event) = upstream.recv().await {
                if let AssistantMessageEvent::Done { reason, mut message } = event {
                    if message.text() == "owned-call" {
                        let tool_call = call("owned", "owned", json!({"text":"decoded"}));
                        message.content = vec![AssistantBlock::ToolCall(tool_call.clone())];
                        message.stop_reason = StopReason::ToolUse;
                        sink.push(AssistantMessageEvent::ToolcallEnd {
                            content_index: 0,
                            tool_call,
                            partial: message.clone(),
                        })
                        .await;
                        sink.push(AssistantMessageEvent::Done { reason: StopReason::ToolUse, message }).await;
                    } else {
                        sink.push(AssistantMessageEvent::Done { reason, message }).await;
                    }
                } else {
                    sink.push(event).await;
                }
            }
        });
        stream
    }
}

#[tokio::test]
async fn owned_pair_and_visible_schema_failure_group() {
    let tool = ProbeTool::new("owned", ToolIntentMode::Omit);
    let provider = Arc::new(Provider {
        turns: Mutex::new(vec![Turn {
            text: "owned-call".into(),
            calls: Vec::new(),
            incomplete: None,
            stop: StopReason::Stop,
        }]),
        ..Default::default()
    });
    let port = Arc::new(OwnedPort { sides: AtomicUsize::new(0), snapshots: Mutex::new(Vec::new()) });
    let manager = Arc::new(Mutex::new(AppendOnlyContextManager::new()));
    let binding = ProviderContextBinding {
        append_only_context: Some(manager.clone()),
        owned_dialect: true,
        owned_port: Some(port.clone()),
        build_options: BuildOptions { prune_tool_descriptions: Some(true), ..Default::default() },
        actual_values: None,
    };
    let mut cfg = config(provider.clone(), vec![tool.clone()], Arc::new(StaticHooks { binding }));
    cfg.tool_choice = Some(ToolChoice::Required);
    let agent = Agent::new(cfg.clone(), Vec::new());
    agent.prompt(vec![user("owned")], CancellationToken::new(), Arc::new(NullSink)).await.unwrap();
    assert_eq!(
        tool.effects.lock().unwrap()[0].1["text"],
        "decoded",
        "owned stream returns canonical calls that really dispatch"
    );
    {
        let contexts = provider.contexts.lock().unwrap();
        assert_eq!(contexts.len(), 1);
        assert!(contexts[0].1.tools.is_none());
        assert!(contexts[0].2.is_none());
        assert!(contexts[0].1.system_prompt.contains(&"owned tools".to_string()));
    }
    assert_eq!(
        manager.lock().unwrap().prefix().to_context().unwrap().1[0].description,
        "probe",
        "owned mode suppresses pruning"
    );
    let cursor = manager.lock().unwrap().sync_cursor();
    assert!(port.snapshots.lock().unwrap().iter().all(|tools| tools.len() == 1 && tools[0].name == "owned"));
    assert_eq!(port.snapshots.lock().unwrap().len(), 1, "each logical model call owns its tool snapshot");
    let side = agent.build_side_request_context(vec![user("side")], Some(vec!["override".into()])).await.unwrap();
    assert_eq!(side.system_prompt, vec!["override"]);
    assert!(side.tools.as_ref().unwrap().is_empty());
    assert_eq!(port.sides.load(Ordering::SeqCst), 0, "side preparation does not run the main owned projection");
    assert_eq!(manager.lock().unwrap().sync_cursor(), cursor);
    agent.set_append_only_context(None).unwrap();
    assert!(agent.append_only_context().is_none());

    let unsupported = ProviderContextBinding { owned_dialect: true, ..Default::default() };
    let upstream = Arc::new(Provider::default());
    let cfg = config(upstream.clone(), vec![tool], Arc::new(StaticHooks { binding: unsupported }));
    let report = agent_loop(vec![user("reject")], &mut Vec::new(), &cfg, &CancellationToken::new(), &NullSink).await;
    assert_eq!(report.end, RunEnd::Error);
    assert!(
        report
            .model_messages()
            .unwrap()
            .last()
            .unwrap()
            .as_assistant()
            .cloned()
            .unwrap()
            .error_message
            .as_ref()
            .unwrap()
            .contains("paired projection")
    );
    assert_eq!(
        build_side_request_context(&cfg, vec![user("side")], None).await.unwrap_err(),
        ProviderContextError::UnsupportedOwnedDialect
    );
    assert!(upstream.contexts.lock().unwrap().is_empty());

    let mut invalid = Arc::try_unwrap(ProbeTool::new("deep", ToolIntentMode::Require)).ok().unwrap();
    let mut schema = json!({"type":"string"});
    for _ in 0..140 {
        schema = json!({"items":schema});
    }
    invalid.definition.parameters = schema;
    let provider = Arc::new(Provider::default());
    let cfg = config(provider.clone(), vec![Arc::new(invalid)], Arc::new(NoHooks));
    let report =
        agent_loop(vec![user("rejectschema")], &mut Vec::new(), &cfg, &CancellationToken::new(), &NullSink).await;
    assert_eq!(report.end, RunEnd::Error);
    assert!(
        report
            .model_messages()
            .unwrap()
            .last()
            .unwrap()
            .as_assistant()
            .cloned()
            .unwrap()
            .error_message
            .as_ref()
            .unwrap()
            .contains("schema")
    );
    assert!(matches!(
        build_side_request_context(&cfg, vec![user("side")], None).await,
        Err(ProviderContextError::Schema(_))
    ));
    assert!(provider.contexts.lock().unwrap().is_empty(), "schema errors cannot fall back to raw definitions");
}

#[tokio::test]
async fn owned_post_hook_snapshot_empty_bypass_and_side_hook_group() {
    struct SnapshotHooks {
        binding: ProviderContextBinding,
        replacement: Option<Vec<Tool>>,
    }
    #[async_trait]
    impl LoopHooks for SnapshotHooks {
        fn provider_context_binding(&self) -> Option<ProviderContextBinding> {
            Some(self.binding.clone())
        }
        async fn transform_provider_context(&self, mut context: Context, _: &Model) -> Context {
            context.tools = self.replacement.clone();
            context
        }
    }
    #[derive(Default)]
    struct SnapshotPort {
        projected: AtomicUsize,
        prepared: Mutex<Vec<Arc<[Tool]>>>,
    }
    impl OwnedProviderContextPort for SnapshotPort {
        fn project_context(&self, _: &Model, context: Context, side: bool) -> Result<Context, ProviderContextError> {
            assert!(!side);
            self.projected.fetch_add(1, Ordering::SeqCst);
            Ok(context)
        }
        fn stream(
            &self,
            provider: &dyn ModelProvider,
            model: &Model,
            context: &Context,
            options: CallOptions,
        ) -> AssistantStream {
            provider.stream(model, context, options)
        }
        fn stream_prepared(
            &self,
            provider: &dyn ModelProvider,
            model: &Model,
            context: &Context,
            tools: Arc<[Tool]>,
            options: CallOptions,
        ) -> AssistantStream {
            self.prepared.lock().unwrap().push(tools);
            provider.stream(model, context, options)
        }
    }
    struct ObserveProvider {
        inner: Arc<Provider>,
        choices: Mutex<Vec<(Option<ToolChoice>, bool)>>,
    }
    impl ModelProvider for ObserveProvider {
        fn stream(&self, model: &Model, context: &Context, options: CallOptions) -> AssistantStream {
            self.choices.lock().unwrap().push((options.tool_choice.clone(), options.suppress_native_tool_choice));
            self.inner.stream(model, context, options)
        }
    }
    let tool = ProbeTool::new("owned", ToolIntentMode::Omit);
    let mut replacement = tool.definition();
    replacement.name = "hooked".into();
    for tools in [None, Some(Vec::new()), Some(vec![replacement.clone()])] {
        let port = Arc::new(SnapshotPort::default());
        let provider =
            Arc::new(ObserveProvider { inner: Arc::new(Provider::default()), choices: Mutex::new(Vec::new()) });
        let hooks = Arc::new(SnapshotHooks {
            binding: ProviderContextBinding {
                owned_dialect: true,
                owned_port: Some(port.clone()),
                ..Default::default()
            },
            replacement: tools.clone(),
        });
        let mut cfg = config(provider.clone(), vec![tool.clone()], hooks);
        cfg.tool_choice = Some(ToolChoice::Required);
        let report =
            agent_loop(vec![user("snapshot")], &mut Vec::new(), &cfg, &CancellationToken::new(), &NullSink).await;
        assert_eq!(report.end, RunEnd::Completed);
        assert_eq!(provider.choices.lock().unwrap().as_slice(), &[(None, true)]);
        let nonempty = tools.as_ref().is_some_and(|tools| !tools.is_empty());
        assert_eq!(port.projected.load(Ordering::SeqCst), usize::from(nonempty));
        assert_eq!(port.prepared.lock().unwrap().len(), usize::from(nonempty));
        {
            let contexts = provider.inner.contexts.lock().unwrap();
            assert_eq!(contexts.len(), 1);
            if nonempty {
                let snapshots = port.prepared.lock().unwrap();
                assert_eq!(snapshots[0][0].name, "hooked", "the stream uses the post-hook definition");
                assert!(contexts[0].1.tools.is_none());
            } else {
                assert_eq!(contexts[0].1.tools, tools, "empty and absent hook inventories keep their original shape");
            }
        }
        let projected = port.projected.load(Ordering::SeqCst);
        let side = build_side_request_context(&cfg, vec![user("side")], None).await.unwrap();
        assert_eq!(side.tools, tools, "side hook mutations survive preparation");
        assert_eq!(port.projected.load(Ordering::SeqCst), projected);
    }
    let port = Arc::new(SnapshotPort::default());
    let provider = Arc::new(ObserveProvider { inner: Arc::new(Provider::default()), choices: Mutex::new(Vec::new()) });
    let mut cfg = config(
        provider.clone(),
        vec![tool],
        Arc::new(SnapshotHooks {
            binding: ProviderContextBinding {
                owned_dialect: false,
                owned_port: Some(port.clone()),
                ..Default::default()
            },
            replacement: Some(vec![replacement]),
        }),
    );
    cfg.tool_choice = Some(ToolChoice::Required);
    assert_eq!(
        agent_loop(vec![user("ordinary")], &mut Vec::new(), &cfg, &CancellationToken::new(), &NullSink).await.end,
        RunEnd::Completed
    );
    assert_eq!(provider.choices.lock().unwrap().as_slice(), &[(Some(ToolChoice::Required), false)]);
    assert_eq!(port.projected.load(Ordering::SeqCst), 0);
    assert!(port.prepared.lock().unwrap().is_empty());
}


// b91db66e six added source cases plus original-domain identity/exception consumers.
// This bank stays in Core1; none of these helpers creates another test target.
fn upgraded_original_prefix_bank() {
    use ara_rpc::value_heap::{CallableTarget,CapabilityId,NoRuntime,PropertyDescriptor,
        RuntimeReply,RuntimeRequest,ValueError,ValueHandle,ValueHeap,ValueRuntime};
    use std::sync::atomic::AtomicBool;
    use ara_agent::append_only_context::actual::prepare_append_only_original_actual as prepare;
    struct Runtime {
        heap:Arc<ValueHeap>,manager:SharedAppendOnlyContextManager,
        schema:Mutex<ValueHandle>,normalizes:AtomicUsize,stringifies:AtomicUsize,
        fail:AtomicBool,invalidate:AtomicBool,trace:Mutex<Vec<u64>>,
        fields:Mutex<std::collections::HashMap<u64,ValueHandle>>,
    }
    impl ValueRuntime for Runtime {
        fn resolve(&self,request:RuntimeRequest<'_>)->Result<RuntimeReply,ValueError>{
            let RuntimeRequest::Call{capability:CapabilityId(id),..}=request else {
                return Err(ValueError::MissingCapability(request.capability()));
            };
            self.trace.lock().unwrap().push(id);
            let value=match id {
                9901=>{self.normalizes.fetch_add(1,Ordering::SeqCst);self.heap.undefined()?},
                9902=>{self.stringifies.fetch_add(1,Ordering::SeqCst);self.heap.undefined()?},
                9903=>{
                    if self.fail.swap(false,Ordering::SeqCst){return Err(ValueError::TypeError("key getter fault".into()))}
                    if self.invalidate.swap(false,Ordering::SeqCst){self.manager.lock().unwrap().invalidate();}
                    self.schema.lock().unwrap().clone()
                },
                id=>self.fields.lock().unwrap().get(&id).cloned()
                    .ok_or(ValueError::MissingCapability(CapabilityId(id)))?,
            };Ok(RuntimeReply::Value(value))
        }
    }
    struct Source { tools:ValueHandle,reads:AtomicUsize }
    impl ActualToolSource for Source {
        fn current_tools(&self,values:&SessionValues)->Result<Option<ValueHandle>,ActualProviderError>{
            values.check_handle(&self.tools)?;self.reads.fetch_add(1,Ordering::SeqCst);Ok(Some(self.tools.clone()))
        }
        fn bind_selected(&self,_:&ValueHandle,_:&SessionValues)->Result<ActualToolBinding,ActualProviderError>{
            Err(ActualProviderError::Unsupported("prefix-only source cannot execute"))
        }
    }
    fn accessor(heap:&Arc<ValueHeap>,object:&ValueHandle,key:&str,id:u64){
        heap.define_own(object,key.into(),PropertyDescriptor::Accessor{
            get:Some(CallableTarget::Host(CapabilityId(id))),set:None,enumerable:true,configurable:true,
        },&NoRuntime).unwrap();
    }
    let heap=ValueHeap::new().unwrap();let manager=Arc::new(Mutex::new(AppendOnlyContextManager::new()));
    let schema_json=json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":false});
    let schema=heap.import_serde(&schema_json).unwrap();
    let runtime=Arc::new(Runtime{heap:heap.clone(),manager:manager.clone(),schema:Mutex::new(schema.clone()),
        normalizes:AtomicUsize::new(0),stringifies:AtomicUsize::new(0),fail:AtomicBool::new(false),
        invalidate:AtomicBool::new(false),trace:Mutex::new(Vec::new()),fields:Mutex::new(Default::default())});
    let values=SessionValues::new(heap.clone(),runtime.clone()).unwrap();
    let get=|object:&ValueHandle,key:&str|heap.get(object,&key.into(),values.runtime()).unwrap();
    let put=|object:&ValueHandle,key:&str,value:ValueHandle|heap.define_own_data(object,key.into(),value,&NoRuntime).unwrap();
    let tool=heap.import_serde(&json!({"name":"probe","description":"initial","intent":"require"})).unwrap();
    accessor(&heap,&tool,"parameters",9903);accessor(&heap,&tool,"snapshotProbe",9901);
    let format=heap.import_serde(&json!({"type":"custom","format":"text"})).unwrap();
    accessor(&heap,&format,"toJSON",9902);put(&tool,"customFormat",format);
    let source=Source{tools:heap.array(vec![tool.clone()]).unwrap(),reads:AtomicUsize::new(0)};
    let options=BuildOptions{intent_tracing:true,..Default::default()};
    let mut prompt=vec!["one".to_string()];
    let build=|prompt:&[String],opts:&BuildOptions|prepare(&manager,prompt,&[],&[],opts,&values,&source).unwrap();
    let first=build(&prompt,&options);
    let original_array=get(first.original_context.as_ref().unwrap(),"tools");
    let before=(runtime.normalizes.load(Ordering::SeqCst),runtime.stringifies.load(Ordering::SeqCst));
    assert!(before.0>0&&before.1>0,"miss reaches normalization and fingerprint stringify");
    assert_eq!(source.reads.load(Ordering::SeqCst),2,"key and takeSnapshot read source independently");
    let second=build(&prompt,&options);
    assert!(get(second.original_context.as_ref().unwrap(),"tools").same_identity(&original_array));
    assert_eq!((runtime.normalizes.load(Ordering::SeqCst),runtime.stringifies.load(Ordering::SeqCst)),before,
        "added case1: live references skip normalization and stringify");
    assert_eq!(source.reads.load(Ordering::SeqCst),3,"hit reads key source only");
    // New source object, stable schema: key observes wire fields, not tool container identity.
    let replacement=heap.import_serde(&json!({"name":"probe","description":"swapped","intent":"require"})).unwrap();
    put(&replacement,"parameters",schema.clone());put(&source.tools,"0",replacement.clone());
    let swapped=build(&prompt,&options);
    assert_eq!(heap.export_serde(&get(&swapped.tools.as_ref().unwrap()[0],"description")).unwrap(),json!("swapped"),
        "added case2: swapped tool objects are detected");
    put(&source.tools,"0",tool.clone());build(&prompt,&options);
    let new_schema=heap.import_serde(&json!({"type":"object","properties":{"count":{"type":"number"}}})).unwrap();
    *runtime.schema.lock().unwrap()=new_schema;
    let changed=build(&prompt,&options);
    assert!(heap.has(&get(&get(&changed.tools.as_ref().unwrap()[0],"parameters"),"properties"),&"count".into(),&NoRuntime).unwrap(),
        "added case3: dynamic parameters swap under stable tool reference");
    // Replacing the wire schema with equal bytes misses once, records new key,
    // and retains the old provider array and version.
    let array=get(changed.original_context.as_ref().unwrap(),"tools");let version=manager.lock().unwrap().prefix().version();
    *runtime.schema.lock().unwrap()=heap.import_serde(&json!({"type":"object","properties":{"count":{"type":"number"}}})).unwrap();
    let equal=build(&prompt,&options);
    assert!(get(equal.original_context.as_ref().unwrap(),"tools").same_identity(&array));
    assert_eq!(manager.lock().unwrap().prefix().version(),version);
    let before=runtime.normalizes.load(Ordering::SeqCst);build(&prompt,&options);
    assert_eq!(runtime.normalizes.load(Ordering::SeqCst),before,"equal fingerprint still records the new cheap key");
    for (field,value) in [("strict",json!(true)),("customWireName",json!("wire_probe")),
        ("customFormat",json!({"type":"custom","format":"new"})),
        ("examples",json!([{"call":{"count":2},"caption":"new example"}])),("intent",json!("optional"))] {
        let version=manager.lock().unwrap().prefix().version();put(&tool,field,heap.import_serde(&value).unwrap());
        build(&prompt,&options);assert!(manager.lock().unwrap().prefix().version()>version,
            "added case4: swapped wire metadata {field} must miss");
    }
    prompt.push("two".into());let result=build(&prompt,&options);assert_eq!(result.system_prompt.len(),2,
        "added case5: in-place caller prompt mutation");
    prompt=vec!["a\0b".into(),"c".into()];build(&prompt,&options);
    let version=manager.lock().unwrap().prefix().version();prompt[0]="a".into();prompt[1]="b\0c".into();
    let result=build(&prompt,&options);assert!(manager.lock().unwrap().prefix().version()>version);
    assert_eq!(result.system_prompt.iter().map(|s|s.to_utf8().unwrap()).collect::<Vec<_>>(),prompt,
        "added case6: NUL-joined text cannot stand for segment equality");
    runtime.fail.store(true,Ordering::SeqCst);
    assert!(matches!(prepare(&manager,&prompt,&[],&[],&options,&values,&source),Err(ProviderContextError::Actual(error))
        if matches!(error.as_ref(),ActualProviderError::Value(ValueError::TypeError(message)) if message=="key getter fault")));
    let version=manager.lock().unwrap().prefix().version();build(&prompt,&options);
    assert_eq!(manager.lock().unwrap().prefix().version(),version,"failed key leaves previous snapshot and key intact");
    runtime.invalidate.store(true,Ordering::SeqCst);
    assert!(matches!(prepare(&manager,&prompt,&[],&[],&options,&values,&source),Err(ProviderContextError::PrefixNotBuilt)),
        "prev is captured before getter invalidation; equal bytes must not resurrect it");
    assert!(!manager.lock().unwrap().prefix().built());build(&prompt,&options);
    // Exact observable field Get order, including two intent Gets on the first build.
    let observed=heap.object().unwrap();
    for (key,id,value) in [("parameters",9910,schema.clone()),("customFormat",9911,heap.undefined().unwrap()),
        ("examples",9912,heap.undefined().unwrap()),("name",9913,heap.string("order".into()).unwrap()),
        ("description",9914,heap.string("order".into()).unwrap()),("strict",9915,heap.boolean(false).unwrap()),
        ("customWireName",9916,heap.undefined().unwrap()),("intent",9917,heap.string("require".into()).unwrap())] {
        runtime.fields.lock().unwrap().insert(id,value);accessor(&heap,&observed,key,id);
    }
    put(&source.tools,"0",observed);manager.lock().unwrap().invalidate();runtime.trace.lock().unwrap().clear();
    build(&prompt,&options);
    assert_eq!(&runtime.trace.lock().unwrap()[..9],&[9910,9911,9912,9913,9914,9915,9916,9917,9917]);
    runtime.fields.lock().unwrap().insert(9917,heap.symbol(Some("mode".into())).unwrap());
    assert!(matches!(prepare(&manager,&prompt,&[],&[],&options,&values,&source),Err(ProviderContextError::Actual(error))
        if matches!(error.as_ref(),ActualProviderError::Value(ValueError::TypeError(_)))),"template mode coercion rejects Symbol");
    // Four memo variants retain their parameter identity across forced rebuilds.
    put(&source.tools,"0",tool.clone());*runtime.schema.lock().unwrap()=schema;
    let mut variants=Vec::new();
    for (mode,prune) in [("require",false),("require",true),("optional",false),("optional",true)] {
        put(&tool,"intent",heap.string(mode.into()).unwrap());
        let opts=BuildOptions{intent_tracing:true,prune_tool_descriptions:Some(prune),..Default::default()};
        manager.lock().unwrap().invalidate();let result=build(&prompt,&opts);
        let parameters=get(&result.tools.as_ref().unwrap()[0],"parameters");
        manager.lock().unwrap().invalidate();let rebuilt=build(&prompt,&opts);
        assert!(get(&rebuilt.tools.as_ref().unwrap()[0],"parameters").same_identity(&parameters),"memo variant {mode}/{prune}");
        assert!(variants.iter().all(|previous:&ValueHandle|!previous.same_identity(&parameters)));variants.push(parameters);
    }
    // Ownership is determined on the authored schema (including refs), before
    // optional pruning/injection; examples never invent a harness placeholder.
    *runtime.schema.lock().unwrap()=heap.import_serde(&json!({"$ref":"#/$defs/args","$defs":{"args":{
        "type":"object","properties":{"i":{"type":"integer"}},"required":["i"],"additionalProperties":false}}})).unwrap();
    put(&tool,"examples",heap.import_serde(&json!([{"call":{"i":42,"other":1}}])).unwrap());
    manager.lock().unwrap().invalidate();let owned=build(&prompt,&options);
    let parameters=heap.export_serde(&get(&owned.tools.as_ref().unwrap()[0],"parameters")).unwrap();
    assert_eq!(parameters["$defs"]["args"]["properties"]["i"]["type"],"integer");
    let description=heap.export_serde(&get(&owned.tools.as_ref().unwrap()[0],"description")).unwrap();
    assert!(!description.as_str().unwrap().contains('…'));
}


async fn upgraded_typed_owned_intent_bank() {
    struct OwnedTool {
        definition:Mutex<Tool>,mode:ToolIntentMode,
        effects:Arc<Mutex<Vec<JsonObject>>>,derived:AtomicUsize,
    }
    #[async_trait]
    impl AgentTool for OwnedTool {
        fn definition(&self)->Tool {self.definition.lock().unwrap().clone()}
        fn intent_mode(&self)->ToolIntentMode {self.mode}
        fn derive_intent(&self,args:&JsonObject)->Option<String>{
            assert_eq!(args.get("i"),Some(&json!(42)));self.derived.fetch_add(1,Ordering::SeqCst);
            Some("derived tool value".into())
        }
        fn concurrency(&self,args:&JsonObject)->Concurrency{
            assert_eq!(args.get("i"),Some(&json!(42)));Concurrency::Shared
        }
        async fn execute(&self,_:&str,args:JsonObject,_:CancellationToken,_:UpdateFn)->Result<ToolOutput,ToolError>{
            assert_eq!(args.get("i"),Some(&json!(42)));self.effects.lock().unwrap().push(args);Ok(ToolOutput::text("own i executed"))
        }
    }
    for mode in [ToolIntentMode::Require,ToolIntentMode::Derived] {
        let tool=Arc::new(OwnedTool{definition:Mutex::new(Tool{name:"owned".into(),description:"tool-owned i".into(),
            parameters:json!({"$ref":"#/$defs/args","$defs":{"args":{"type":"object",
                "properties":{"i":{"type":"integer"}},"required":["i"],"additionalProperties":false}}}),
            examples:Some(vec![ToolExample::Call{call:json!({"i":42}).as_object().unwrap().clone(),caption:None}]),
            ..Default::default()}),mode,effects:Arc::new(Mutex::new(Vec::new())),derived:AtomicUsize::new(0)});
        let provider=Arc::new(Provider{turns:Mutex::new(vec![Turn{text:String::new(),
            calls:vec![call("own","owned",json!({"i":42}))],incomplete:None,stop:StopReason::ToolUse}]),..Default::default()});
        let opts=BuildOptions{intent_tracing:true,..Default::default()};
        let tools:Vec<Arc<dyn AgentTool>>=vec![tool.clone()];
        let mut prefix=StablePrefix::default();assert!(prefix.build(&[],&tools,&opts).unwrap());
        assert!(!prefix.build(&[],&tools,&opts).unwrap());
        tool.definition.lock().unwrap().strict=Some(true);
        assert!(prefix.build(&[],&tools,&opts).unwrap(),"owned typed values detect live metadata replacement");
        let normalized=normalize_tools(&tools,&opts).unwrap();
        assert_eq!(normalized[0].parameters["$defs"]["args"]["properties"]["i"]["type"],"integer");
        assert!(!normalized[0].description.contains('…'));
        let mut cfg=config(provider,tools,Arc::new(StaticHooks{
            binding:ProviderContextBinding{build_options:opts,..Default::default()}}));
        cfg.max_model_calls=Some(2);
        let mut history=Vec::new();
        let report=agent_loop(vec![user("use own i")],&mut history,&cfg,&CancellationToken::new(),&NullSink).await;
        assert_eq!(report.end,RunEnd::Completed);assert_eq!(tool.effects.lock().unwrap().as_slice(),&[json!({"i":42}).as_object().unwrap().clone()]);
        let recorded=history.iter().filter_map(|message|message.checked_model_snapshot().ok().flatten())
            .find_map(|message|match message {Message::Assistant(message)=>message.content.into_iter().find_map(|block|
                if let AssistantBlock::ToolCall(call)=block{Some(call)}else{None}),_=>None}).unwrap();
        assert_eq!(recorded.arguments.get("i"),Some(&json!(42)));
        if mode==ToolIntentMode::Derived {
            assert_eq!(recorded.intent.as_deref(),Some("derived tool value"));assert_eq!(tool.derived.load(Ordering::SeqCst),1);
        } else {assert!(recorded.intent.is_none());assert_eq!(tool.derived.load(Ordering::SeqCst),0);}
    }
}
