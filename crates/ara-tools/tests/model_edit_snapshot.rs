//! Grouped model-mode snapshots, actual file display/effects and grammar metadata.
use ara_agent::{AgentTool, Concurrency, ToolError, ToolOutput};
use ara_ai::UserBlock;
use ara_edit::EditMode;
use ara_tools::edit::EditTool;
use ara_tools::grep::GrepTool;
use ara_tools::read::ReadTool;
use ara_tools::write::WriteTool;
use ara_tools::{ContentUriPort, ContentUriRoute, ToolContext, UriResource};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

struct CustomEditProvider {
    raw: String,
    calls: std::sync::atomic::AtomicUsize,
}

impl ara_ai::ModelProvider for CustomEditProvider {
    fn stream(
        &self,
        model: &ara_ai::Model,
        context: &ara_ai::Context,
        _options: ara_ai::CallOptions,
    ) -> ara_ai::AssistantStream {
        let first = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0;
        let definitions = context.tools.as_ref().expect("registered tool projection");
        assert_eq!(definitions.len(), 1);
        assert_eq!(definitions[0].name, "edit", "logical grant stays edit");
        assert_eq!(definitions[0].custom_wire_name.as_deref(), Some("apply_patch"));
        assert!(definitions[0].custom_format.is_some());
        let (sink, rx) = ara_ai::EventSink::channel();
        let mut output = ara_ai::AssistantMessage::empty(&model.api, &model.provider, &model.id);
        if first {
            output.content.push(ara_ai::AssistantBlock::ToolCall(ara_ai::ToolCall {
                id: "call_engine|ctc_engine".into(),
                name: "apply_patch".into(),
                arguments: json!({"input":self.raw}).as_object().unwrap().clone(),
                custom_wire_name: Some("apply_patch".into()),
                ..Default::default()
            }));
            output.stop_reason = ara_ai::StopReason::ToolUse;
        } else {
            output.content.push(ara_ai::AssistantBlock::text("effect reviewed"));
            output.stop_reason = ara_ai::StopReason::Stop;
        }
        // Exercise the same serde projection that existing Session journals use.
        let output: ara_ai::AssistantMessage = serde_json::from_value(serde_json::to_value(output).unwrap()).unwrap();
        tokio::spawn(async move {
            sink.push(ara_ai::AssistantMessageEvent::Start { partial: output.clone() }).await;
            if first {
                sink.push(ara_ai::AssistantMessageEvent::ToolcallEnd {
                    content_index: 0,
                    tool_call: output.tool_calls().next().unwrap().clone(),
                    partial: output.clone(),
                })
                .await;
            }
            sink.push(ara_ai::AssistantMessageEvent::Done { reason: output.stop_reason, message: output }).await;
        });
        rx
    }
}

struct CustomEditApproval {
    denied: bool,
    seen: std::sync::Mutex<Vec<String>>,
}

#[async_trait]
impl ara_agent::LoopHooks for CustomEditApproval {
    async fn before_tool_call(
        &self,
        call: &ara_ai::ToolCall,
        args: &ara_ai::JsonObject,
        _cancel: &CancellationToken,
    ) -> ara_agent::ToolDecision {
        assert_eq!(call.name, "apply_patch");
        assert_eq!(call.custom_wire_name.as_deref(), Some("apply_patch"));
        self.seen.lock().unwrap().push(args["input"].as_str().unwrap().into());
        if self.denied {
            ara_agent::ToolDecision::Block(Some("fixture denies edit".into()))
        } else {
            ara_agent::ToolDecision::Allow
        }
    }
}

#[tokio::test]
async fn custom_alias_uses_existing_edit_session_after_schema_and_approval_group() {
    for (denied, invalid_raw) in [(false, false), (true, false), (false, true)] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("file.txt");
        std::fs::write(&path, "alpha\n").unwrap();
        let raw = if invalid_raw {
            "not an apply patch document"
        } else {
            "*** Begin Patch\n*** Update File: file.txt\n@@\n-alpha\n+beta\n*** End Patch"
        };
        let edit: Arc<dyn AgentTool> =
            Arc::new(EditTool::new(ToolContext::new(directory.path()).with_edit(EditMode::ApplyPatch, true)));
        let provider = Arc::new(CustomEditProvider { raw: raw.into(), calls: std::sync::atomic::AtomicUsize::new(0) });
        let hooks = Arc::new(CustomEditApproval { denied, seen: Default::default() });
        let config = ara_agent::AgentConfig {
            model: ara_ai::Model {
                id: "engine-fixture".into(),
                api: "openai-responses".into(),
                provider: "fixture".into(),
                base_url: "https://fixture.example/v1".into(),
                reasoning: false,
                max_tokens: None,
                context_window: None,
                tokenizer: None,
            },
            provider: provider.clone(),
            system_prompt: vec![],
            tools: vec![edit],
            tool_choice: None,
            max_tokens: None,
            temperature: None,
            deadline: None,
            max_model_calls: None,
            hooks: hooks.clone(),
            interrupt_mode: ara_agent::InterruptMode::Immediate,
        };
        let messages = ara_agent::Agent::new(config, vec![])
            .prompt(
                vec![ara_ai::Message::User(ara_ai::UserMessage::text("apply the fixture"))],
                CancellationToken::new(),
                Arc::new(ara_agent::NullSink),
            )
            .await
            .unwrap()
            .model_messages()
            .unwrap();
        assert_eq!(*hooks.seen.lock().unwrap(), vec![raw.to_string()], "raw input still follows schema and approval");
        let result = messages
            .iter()
            .find_map(|message| match message {
                ara_ai::Message::ToolResult(result) => Some(result),
                _ => None,
            })
            .unwrap();
        assert_eq!(result.tool_name, "apply_patch");
        assert_eq!(result.tool_call_id, "call_engine|ctc_engine");
        assert_eq!(result.is_error, denied || invalid_raw);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), if denied || invalid_raw { "alpha\n" } else { "beta\n" });
        assert_eq!(
            provider.calls.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "the receipt is replayed once to the model"
        );
        assert_eq!(messages.last().unwrap().as_assistant().unwrap().text(), "effect reviewed");
    }
}

struct MemoryUri;

#[async_trait]
impl ContentUriPort for MemoryUri {
    fn route(&self, _: &str) -> ContentUriRoute {
        ContentUriRoute::Registered { writable: false }
    }
    async fn read(&self, _: &str, _: CancellationToken) -> Result<UriResource, ToolError> {
        Ok(UriResource {
            content: "immutable receipt\n".into(),
            content_type: "text/plain".into(),
            notes: vec![],
            immutable: true,
        })
    }
    async fn write(&self, _: &str, _: &str, _: CancellationToken) -> Result<(), ToolError> {
        Err(ToolError("read-only fixture".into()))
    }
}

fn text(output: &ToolOutput) -> String {
    output
        .content
        .iter()
        .filter_map(|part| match part {
            UserBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn run(tool: &dyn AgentTool, arguments: Value) -> ToolOutput {
    tool.execute("snapshot", arguments.as_object().unwrap().clone(), CancellationToken::new(), Arc::new(|_| {}))
        .await
        .unwrap()
}

#[tokio::test]
async fn immutable_mode_clones_preserve_session_store_skills_uri_and_file_contracts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "alpha\nbeta\n").unwrap();
    let uri: Arc<dyn ContentUriPort> = Arc::new(MemoryUri);
    let initial = ToolContext::new(dir.path()).with_uri_port(uri.clone());
    let first = initial.edit_store.record(&path, "alpha\nbeta\n", Some(&[1]));
    let replacement = initial.clone().with_edit_modes(EditMode::Replace, EditMode::Replace, true);
    let split = initial.clone().with_edit_modes(EditMode::Patch, EditMode::Hashline, true);
    assert_eq!(initial.edit_mode(), EditMode::Hashline);
    assert_eq!(replacement.edit_mode(), EditMode::Replace);
    assert_eq!(replacement.display_edit_mode(), EditMode::Replace);
    assert!(initial.hashlines() && split.hashlines() && !replacement.hashlines());
    assert!(Arc::ptr_eq(&initial.skills, &replacement.skills));
    assert!(Arc::ptr_eq(&uri, &replacement.content_uri_port().unwrap()));
    assert_eq!(replacement.edit_store.head(&path).unwrap().hash, first);
    let second = replacement.edit_store.record(&path, "alpha\ngamma\n", Some(&[2]));
    assert_eq!(initial.edit_store.head(&path).unwrap().hash, second);
    assert!(ToolContext::new(dir.path()).edit_store.head(&path).is_none());
    // Restore the observed file version before exercising the actual engines.
    initial.edit_store.record(&path, "alpha\nbeta\n", Some(&[1, 2]));
    let old_read = ReadTool { ctx: initial.clone() };
    let new_read = ReadTool { ctx: replacement.clone() };
    assert!(text(&run(&old_read, json!({"path":"file.txt"})).await).starts_with("[file.txt#"));
    assert_eq!(text(&run(&new_read, json!({"path":"file.txt"})).await), "alpha\nbeta");
    assert_eq!(text(&run(&old_read, json!({"path":"file.txt:raw"})).await), "alpha\nbeta\n");
    assert!(!text(&run(&old_read, json!({"path":"artifact://receipt"})).await).contains("#"));
    let old_grep = GrepTool::new(initial.clone());
    let new_grep = GrepTool::new(replacement.clone());
    assert!(text(&run(&old_grep, json!({"path":"file.txt", "pattern":"beta"})).await).contains("[file.txt#"));
    assert!(!text(&run(&new_grep, json!({"path":"file.txt", "pattern":"beta"})).await).contains("[file.txt#"));
    let new_edit = EditTool::new(replacement.clone());
    let edited = run(&new_edit, json!({"path":"file.txt", "old_string":"beta", "new_string":"gamma"})).await;
    assert!(!edited.is_error);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "alpha\ngamma\n");
    let old_write = WriteTool::new(initial.clone());
    let new_write = WriteTool::new(replacement.clone());
    assert!(
        text(&run(&old_write, json!({"path":"anchored.txt", "content":"alpha\n"})).await).contains("[anchored.txt#")
    );
    assert!(!text(&run(&new_write, json!({"path":"plain.txt", "content":"alpha\n"})).await).contains("[plain.txt#"));
    assert_eq!(split.edit_mode(), EditMode::Patch);
    assert_eq!(split.display_edit_mode(), EditMode::Hashline);
    let disabled = initial.clone().with_edit_modes(EditMode::Replace, EditMode::Hashline, false);
    assert!(!disabled.hashlines());
    for mode in [EditMode::Hashline, EditMode::Replace, EditMode::Patch, EditMode::ApplyPatch, EditMode::Sloppy] {
        let legacy = initial.clone().with_edit(mode, true);
        assert_eq!(legacy.edit_mode(), mode);
        assert_eq!(legacy.display_edit_mode(), mode);
    }
}

#[test]
fn mode_schema_grammar_alias_and_exclusive_execution_remain_separate_from_wire_projection() {
    for mode in [EditMode::Hashline, EditMode::Replace, EditMode::Patch, EditMode::ApplyPatch, EditMode::Sloppy] {
        let tool = EditTool::new(ToolContext::new(".").with_edit(mode, true));
        let definition = tool.definition();
        assert_eq!(definition.name, "edit");
        assert_eq!(definition.description, ara_edit::description(mode));
        assert_eq!(definition.parameters, ara_tools::edit::parameters(mode));
        assert_eq!(tool.concurrency(&Default::default()), Concurrency::Exclusive);
        assert_eq!(tool.grammar(), ara_edit::grammar(mode));
        assert_eq!(tool.custom_wire_name(), (mode == EditMode::ApplyPatch).then_some("apply_patch"));
        for gate in [None, Some("function"), Some("FREEFORM")] {
            assert_eq!(tool.freeform_wire_name(gate), None);
        }
        let expected = match mode {
            EditMode::ApplyPatch => Some("apply_patch"),
            EditMode::Hashline | EditMode::Sloppy => Some("edit"),
            _ => None,
        };
        assert_eq!(tool.freeform_wire_name(Some("freeform")), expected);
    }
}
