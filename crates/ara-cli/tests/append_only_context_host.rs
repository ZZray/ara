//! Grouped Host context acceptance: settings subscriptions, owned publication,
//! explicit fresh boundaries and side snapshots. Real YAML/process cases use
//! the reference Host's actual settings owner in the binary/RPC test group.

use ara_agent::{AgentConfig, BuildOptions, ExecutionSnapshot, LoopHooks};
use ara_ai::{Context, Message, Model, ModelProvider, UserMessage};
use ara_cli::append_only_context_mode::{
    AppendOnlyContextMode as Mode, AppendOnlyContextModel, AppendOnlyContextSettings, AppendOnlyContextSettingsStore,
    HostContextPreferences, SessionContextController, SnapshotPublication, parse_intent_flag,
    should_enable_append_only_context,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex, RwLock,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Default)]
struct Store {
    mode: Mutex<Mode>,
    fail: Mutex<bool>,
}
impl AppendOnlyContextSettingsStore for Store {
    fn load(&self) -> Result<HostContextPreferences, String> {
        if *self.fail.lock().unwrap() {
            return Err("controlled read failure".into());
        }
        Ok(HostContextPreferences { mode: *self.mode.lock().unwrap(), ..Default::default() })
    }
    fn store_mode(&self, mode: Mode) -> Result<(), String> {
        if *self.fail.lock().unwrap() {
            return Err("controlled store failure".into());
        }
        *self.mode.lock().unwrap() = mode;
        Ok(())
    }
}

fn model(provider: &str) -> Model {
    Model {
        id: "deepseek-v4.1-flash".into(),
        api: "openai-completions".into(),
        provider: provider.into(),
        base_url: "https://remote.example/v1".into(),
        reasoning: false,
        max_tokens: None,
        context_window: Some(8192.0),
        tokenizer: None,
    }
}

struct NoProvider;
impl ModelProvider for NoProvider {
    fn stream(&self, _: &Model, _: &Context, _: ara_ai::CallOptions) -> ara_ai::AssistantStream {
        panic!("context preparation must not call the provider")
    }
}

fn config(selected: Model) -> AgentConfig {
    AgentConfig {
        model: selected,
        provider: Arc::new(NoProvider),
        system_prompt: vec!["owned-prompt".into()],
        tools: vec![],
        tool_choice: None,
        max_tokens: None,
        temperature: None,
        deadline: None,
        max_model_calls: None,
        hooks: Arc::new(ara_agent::NoHooks),
        interrupt_mode: ara_agent::InterruptMode::Immediate,
    }
}

fn controller(settings: &Arc<AppendOnlyContextSettings>, provider: &str) -> SessionContextController {
    SessionContextController::new(
        settings.clone(),
        &model(provider),
        None,
        BuildOptions { intent_tracing: true, prune_tool_descriptions: Some(false), suppress_intent_schema: false },
        "auto",
    )
}

fn seed(controller: &SessionContextController) -> ara_agent::SharedAppendOnlyContextManager {
    let manager = controller.binding().append_only_context.unwrap();
    let mut guard = manager.lock().unwrap();
    guard.build(&["stable-prefix".into()], &[], &controller.binding().build_options).unwrap();
    guard.sync_messages(&[Message::User(UserMessage::text("retained source"))]).unwrap();
    drop(guard);
    manager
}

#[test]
fn mode_sparse_metadata_and_session_start_flags_group() {
    let ordinary =
        AppendOnlyContextModel::from_selected(&model("custom"), Some(&json!({"compat":{"supportsStore":true}})));
    assert!(!should_enable_append_only_context(Mode::Auto, Some(&ordinary)), "resolved default is not authored intent");
    for sparse in [None, Some(false), Some(true)] {
        let metadata = sparse.map(|value| json!({"compatConfig":{"supportsStore":value}}));
        let selected = AppendOnlyContextModel::from_selected(&model("custom"), metadata.as_ref());
        assert_eq!(should_enable_append_only_context(Mode::Auto, Some(&selected)), sparse == Some(true));
        assert!(should_enable_append_only_context(Mode::On, Some(&selected)));
        assert!(!should_enable_append_only_context(Mode::Off, Some(&selected)));
    }
    for provider in ["deepseek", "ollama", "ollama-cloud", "lm-studio", "llama.cpp"] {
        assert!(should_enable_append_only_context(
            Mode::Auto,
            Some(&AppendOnlyContextModel::from_selected(&model(provider), None))
        ));
    }
    for base in [
        "http://localhost:8000/v1",
        "http://127.0.0.1:8000",
        "http://[::1]/",
        "http://10.4.2.1/",
        "http://172.16.2.1/",
        "http://192.168.1.1/",
        "http://box.local/",
    ] {
        let mut selected = ordinary.clone();
        selected.base_url = base.into();
        assert!(should_enable_append_only_context(Mode::Auto, Some(&selected)), "{base}");
    }
    for value in ["1", "Y", "y", "TRUE", "true", "YES", "yes", "ON", "on"] {
        assert!(parse_intent_flag(Some(value), false));
    }
    for value in ["True", " true", "true ", "off", "0"] {
        assert!(!parse_intent_flag(Some(value), true));
    }
    assert!(parse_intent_flag(None, true));
    assert!(parse_intent_flag(Some(""), true));
    let preferences = HostContextPreferences::default();
    let gemini = preferences.build_options("gemini-2.5-pro", None, Some("1")).unwrap();
    assert_eq!(gemini.prune_tool_descriptions, Some(true));
    assert!(gemini.intent_tracing);
    assert!(gemini.suppress_intent_schema);
    assert!(!preferences.build_options("deepseek-v4.1-flash", Some("off"), None).unwrap().intent_tracing);
}

#[test]
fn settings_two_sessions_noop_failures_reload_drop_and_fresh_group() {
    let store = Arc::new(Store::default());
    let settings = AppendOnlyContextSettings::new(store.clone()).unwrap();
    let first = controller(&settings, "deepseek");
    let second = controller(&settings, "custom");
    assert_eq!(settings.subscription_count(), 2);
    let manager = seed(&first);
    assert!(second.binding().append_only_context.is_none());
    settings.set(Mode::On).unwrap();
    assert!(second.binding().append_only_context.is_some());
    assert!(Arc::ptr_eq(&manager, &first.binding().append_only_context.unwrap()));
    assert_eq!(manager.lock().unwrap().log().len(), 1, "same enable/provider tuple is a no-op");
    first.publish_model(&model("deepseek"), None);
    assert_eq!(manager.lock().unwrap().sync_cursor(), 1, "ordinary route/epoch publication is not fresh");
    *store.fail.lock().unwrap() = true;
    assert!(settings.set(Mode::Off).is_err());
    assert!(settings.reload().is_err());
    assert_eq!(settings.get(), Mode::On);
    assert_eq!(manager.lock().unwrap().log().len(), 1);
    *store.fail.lock().unwrap() = false;
    *store.mode.lock().unwrap() = Mode::Off;
    settings.reload().unwrap();
    assert!(first.binding().append_only_context.is_none());
    assert!(second.binding().append_only_context.is_none());
    settings.set(Mode::On).unwrap();
    let replacement = seed(&first);
    assert!(!Arc::ptr_eq(&manager, &replacement));
    first.fresh_provider_session();
    assert_eq!(replacement.lock().unwrap().sync_cursor(), 0);
    assert!(!replacement.lock().unwrap().prefix().built());
    seed(&first);
    first.publish_model(&model("different-provider"), None);
    assert_eq!(replacement.lock().unwrap().log().len(), 0, "enabled provider change invalidates");
    assert_eq!(
        first.binding().build_options.prune_tool_descriptions,
        Some(false),
        "Session-start options remain fixed"
    );
    drop(second);
    assert_eq!(settings.subscription_count(), 1);
    drop(first);
    assert_eq!(settings.subscription_count(), 0);
}

#[test]
fn snapshot_adoption_fence_candidate_isolation_and_active_setter_group() {
    let settings = AppendOnlyContextSettings::new(Arc::new(Store::default())).unwrap();
    let old = controller(&settings, "deepseek");
    let old_manager = seed(&old);
    let slot = Arc::new(RwLock::new(ExecutionSnapshot::from_config(&config(model("deepseek")))));
    let publication = SnapshotPublication::new(&slot);
    publication.publish(&old, ExecutionSnapshot::from_config(&config(model("deepseek"))), None);
    {
        let candidate = controller(&settings, "custom");
        assert!(candidate.binding().append_only_context.is_none());
        assert!(Arc::ptr_eq(
            &old_manager,
            &slot.read().unwrap().provider_context.as_ref().unwrap().append_only_context.clone().unwrap()
        ));
    }
    assert_eq!(settings.subscription_count(), 1, "failed candidate subscription is released");
    let adopted = controller(&settings, "deepseek");
    let adopted_manager = seed(&adopted);
    publication.publish(&adopted, ExecutionSnapshot::from_config(&config(model("deepseek"))), None);
    old.set_append_only_context(None);
    assert!(
        Arc::ptr_eq(
            &adopted_manager,
            &slot.read().unwrap().provider_context.as_ref().unwrap().append_only_context.clone().unwrap()
        ),
        "detached callbacks cannot change the adopted snapshot"
    );
    publication.set_append_only_context(None).unwrap();
    assert!(adopted.binding().append_only_context.is_none());
    assert!(slot.read().unwrap().provider_context.as_ref().unwrap().append_only_context.is_none());
    publication.set_append_only_context(Some(adopted_manager)).unwrap();
    let before = slot.read().unwrap().clone();
    let mut changed = config(model("custom"));
    changed.system_prompt = vec!["new-model-prompt".into()];
    publication.publish(
        &adopted,
        ExecutionSnapshot::from_config(&changed),
        Some(&json!({"supportsTools":false,"compatConfig":{"supportsStore":true}})),
    );
    let current = slot.read().unwrap();
    assert_eq!(current.model.as_ref().unwrap().model.provider, "custom");
    assert_eq!(current.system_prompt, ["new-model-prompt"]);
    assert!(current.provider_context.as_ref().unwrap().owned_dialect);
    assert_eq!(before.model.as_ref().unwrap().model.provider, "deepseek");
    assert!(!before.provider_context.unwrap().owned_dialect, "admitted snapshot keeps its owned binding");
}

struct SideHooks {
    snapshot: ExecutionSnapshot,
    calls: AtomicUsize,
    transforms: AtomicUsize,
}
#[async_trait]
impl LoopHooks for SideHooks {
    fn execution_snapshot(&self) -> Option<ExecutionSnapshot> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Some(self.snapshot.clone())
    }
    fn provider_context_binding(&self) -> Option<ara_agent::ProviderContextBinding> {
        panic!("side must not resample live binding after its owned snapshot")
    }
    async fn transform_provider_context(&self, mut context: Context, _: &Model) -> Context {
        self.transforms.fetch_add(1, Ordering::SeqCst);
        context.system_prompt.push("transformed".into());
        context
    }
}

#[tokio::test]
async fn side_owned_snapshot_preserves_log_cursor_prefix_and_runs_transform_group() {
    let settings = AppendOnlyContextSettings::new(Arc::new(Store::default())).unwrap();
    let controller = controller(&settings, "deepseek");
    let manager = seed(&controller);
    let original = manager.lock().unwrap().to_context().unwrap();
    let version = manager.lock().unwrap().prefix().version();
    let mut captured = ExecutionSnapshot::from_config(&config(model("deepseek")));
    captured.provider_context = Some(controller.binding());
    captured.system_prompt = vec!["snapshot-prompt".into()];
    let hooks =
        Arc::new(SideHooks { snapshot: captured.clone(), calls: AtomicUsize::new(0), transforms: AtomicUsize::new(0) });
    let mut live = config(model("custom"));
    live.hooks = hooks.clone();
    let side =
        ara_agent::build_side_request_context(&live, vec![Message::User(UserMessage::text("side source"))], None)
            .await
            .unwrap();
    assert_eq!(side.system_prompt, ["snapshot-prompt", "transformed"]);
    assert_eq!(hooks.calls.load(Ordering::SeqCst), 1);
    assert_eq!(hooks.transforms.load(Ordering::SeqCst), 1);
    assert_eq!(manager.lock().unwrap().to_context().unwrap(), original);
    assert_eq!(manager.lock().unwrap().sync_cursor(), 1);
    assert_eq!(manager.lock().unwrap().prefix().version(), version);
    captured.provider_context.as_mut().unwrap().owned_dialect = true;
    let error = ara_agent::build_side_request_context_from_snapshot(&live, captured, vec![], None).await.unwrap_err();
    assert_eq!(error, ara_agent::ProviderContextError::UnsupportedOwnedDialect);
    assert_eq!(
        hooks.transforms.load(Ordering::SeqCst),
        1,
        "owned port validation fails before external transformation/network"
    );
}

struct RpcProcess {
    child: std::process::Child,
    input: Option<std::process::ChildStdin>,
    frames: std::sync::mpsc::Receiver<Value>,
    errors: Arc<Mutex<String>>,
    readers: Vec<std::thread::JoinHandle<()>>,
}
impl RpcProcess {
    fn spawn(mut command: std::process::Command) -> Self {
        use std::io::{BufRead, BufReader, Read};
        use std::process::Stdio;
        let mut child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let input = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let (sender, frames) = std::sync::mpsc::channel();
        let output = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let frame = serde_json::from_str(&line).unwrap();
                if sender.send(frame).is_err() {
                    break;
                }
            }
        });
        let errors = Arc::new(Mutex::new(String::new()));
        let captured = errors.clone();
        let error = std::thread::spawn(move || {
            let mut bytes = [0; 4096];
            while let Ok(count) = stderr.read(&mut bytes) {
                if count == 0 {
                    break;
                }
                captured.lock().unwrap().push_str(&String::from_utf8_lossy(&bytes[..count]));
            }
        });
        Self { child, input, frames, errors, readers: vec![output, error] }
    }
    fn send(&mut self, value: Value) {
        use std::io::Write;
        let input = self.input.as_mut().unwrap();
        writeln!(input, "{value}").unwrap();
        input.flush().unwrap();
    }
    fn until(&self, predicate: impl Fn(&Value) -> bool) -> Value {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            let frame = self
                .frames
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                .unwrap_or_else(|error| panic!("RPC deadline: {error}; {}", self.errors.lock().unwrap()));
            if predicate(&frame) {
                return frame;
            }
        }
    }
    fn response(&self, id: &str, success: bool) -> Value {
        let frame = self.until(|frame| frame["type"] == "response" && frame["id"] == id);
        assert_eq!(frame["success"], success, "{frame}");
        frame
    }
    fn finish(&mut self) {
        self.input.take();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "{}", self.errors.lock().unwrap());
                break;
            }
            assert!(std::time::Instant::now() < deadline, "RPC EOF deadline");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        for reader in self.readers.drain(..) {
            reader.join().unwrap();
        }
    }
}
impl Drop for RpcProcess {
    fn drop(&mut self) {
        self.input.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_rpc_settings_inline_prompt_failed_candidate_and_fresh_session_group() {
    use ara_testkit::chunks::{done, finish, text};
    use ara_testkit::{FakeUpstream, Script};
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let agent_dir = home.path().join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let settings_path = agent_dir.join("config.yml");
    let settings = "provider: {appendOnlyContext: on}\ninlineToolDescriptors: on\ncustom: {preserve: fixture}\n";
    std::fs::write(&settings_path, settings).unwrap();
    let up = FakeUpstream::start(
        serde_json::from_value::<Script>(json!({"responses":[
            {"events":[text("first answer"),finish("stop"),done()]},
            {"events":[text("second answer"),finish("stop"),done()]},
            {"events":[text("fresh answer"),finish("stop"),done()]}
        ]}))
        .unwrap(),
        None,
    )
    .await
    .unwrap();
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_ara"));
    for key in [
        "ARA_MODEL",
        "ARA_PROVIDER",
        "ARA_BASE_URL",
        "ARA_TEST_BASE_URL",
        "ARA_TEST_MODEL_ID",
        "ARA_TEST_API_KEY",
        "ARA_TOKENIZER",
        "ARA_API",
        "ARA_INTENT_TRACING",
        "ARA_NO_INTENT",
        "OPENROUTER_API_KEY",
        "OPENROUTER_BASE_URL",
        "CLAUDE_CONFIG_DIR",
        "WSL_DISTRO_NAME",
        "WSL_INTEROP",
    ] {
        command.env_remove(key);
    }
    command
        .env("HOME", home.path())
        .env("ARA_HOME", home.path())
        .env("ARA_API_KEY", "sk-context-fixture")
        .args([
            "--mode",
            "rpc",
            "--model",
            "fake-model",
            "--base-url",
            &up.base_url(),
            "--tools",
            "read",
            "--no-skills",
            "--compact-threshold",
            "0",
            "--cwd",
        ])
        .arg(work.path())
        .arg("--session-dir")
        .arg(home.path().join("sessions"));
    let mut child = RpcProcess::spawn(command);
    child.until(|frame| frame["type"] == "ready");
    child.send(json!({"type":"get_append_only_context","id":"initial-mode"}));
    assert_eq!(child.response("initial-mode", true)["data"], json!({"mode":"on","enabled":true}));
    child.send(json!({"type":"prompt","id":"first-turn","message":"alpha retained"}));
    child.response("first-turn", true);
    child.until(|frame| frame["type"] == "agent_end");
    child.send(json!({"type":"set_append_only_context","id":"disable","mode":"off"}));
    assert_eq!(child.response("disable", true)["data"]["enabled"], false);
    let written = std::fs::read_to_string(&settings_path).unwrap();
    assert!(written.contains("fixture"));
    assert!(written.contains("off"));
    std::fs::write(&settings_path, "provider: {appendOnlyContext: broken}\n").unwrap();
    child.send(json!({"type":"new_session","id":"failed-candidate"}));
    child.response("failed-candidate", false);
    child.send(json!({"type":"reload_append_only_context","id":"failed-reload"}));
    child.response("failed-reload", false);
    child.send(json!({"type":"get_append_only_context","id":"still-old"}));
    assert_eq!(child.response("still-old", true)["data"], json!({"mode":"off","enabled":false}));
    std::fs::write(&settings_path, settings).unwrap();
    child.send(json!({"type":"reload_append_only_context","id":"enable-again"}));
    child.response("enable-again", true);
    child.send(json!({"type":"prompt","id":"second-turn","message":"beta current"}));
    child.response("second-turn", true);
    child.until(|frame| frame["type"] == "agent_end");
    child.send(json!({"type":"new_session","id":"fresh-session"}));
    child.response("fresh-session", true);
    child.send(json!({"type":"prompt","id":"fresh-turn","message":"gamma fresh"}));
    child.response("fresh-turn", true);
    child.until(|frame| frame["type"] == "agent_end");
    child.finish();
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 3);
    for request in requests.iter() {
        let messages = request["body"]["messages"].to_string();
        assert!(messages.contains("namespace functions"), "full catalog is in the provider prompt: {messages}");
        assert_eq!(request["body"]["tools"][0]["function"]["description"], "");
        assert!(request["body"]["tools"][0]["function"]["parameters"]["properties"].get("i").is_some());
    }
    assert!(requests[1]["body"]["messages"].to_string().contains("alpha retained"));
    assert!(!requests[2]["body"]["messages"].to_string().contains("alpha retained"));
    assert!(!requests[2]["body"]["messages"].to_string().contains("beta current"));
    assert!(requests[2]["body"]["messages"].to_string().contains("gamma fresh"));
}

#[test]
fn actual_binding_candidate_staging_and_publication_refresh_group() {
    let settings = AppendOnlyContextSettings::new(Arc::new(Store::default())).unwrap();
    let typed = controller(&settings, "custom");
    typed.stage_actual_values(None).unwrap();
    let slot = Arc::new(RwLock::new(ExecutionSnapshot::from_config(&config(model("custom")))));
    let publication = SnapshotPublication::new(&slot);
    publication.publish(&typed, ExecutionSnapshot::from_config(&config(model("custom"))), None);
    // NoRuntime is confined to this non-executing identity fixture.
    let heap = ara_rpc::value_heap::ValueHeap::new().unwrap();
    let selected =
        Arc::new(ara_agent::SessionValues::new(heap.clone(), Arc::new(ara_rpc::value_heap::NoRuntime)).unwrap());
    let alias = Arc::new(ara_agent::SessionValues::new(heap, Arc::new(ara_rpc::value_heap::NoRuntime)).unwrap());
    let port: Arc<dyn ara_agent::OwnedProviderContextPort> =
        Arc::new(ara_cli::actual_provider::ActualNativeProviderPort::default());
    let other_port: Arc<dyn ara_agent::OwnedProviderContextPort> =
        Arc::new(ara_cli::actual_provider::ActualNativeProviderPort::default());
    let direct = controller(&settings, "custom");
    direct.stage_actual_binding(selected.clone(), port.clone()).unwrap();
    let direct_binding = direct.binding();
    assert!(Arc::ptr_eq(direct_binding.actual_values.as_ref().unwrap(), &selected));
    assert!(Arc::ptr_eq(direct_binding.owned_port.as_ref().unwrap(), &port));
    assert!(direct.stage_actual_binding(alias.clone(), port.clone()).is_err());
    assert!(direct.stage_actual_binding(selected.clone(), other_port.clone()).is_err());
    assert!(typed.stage_actual_binding(selected.clone(), port.clone()).is_err());
    assert!(typed.binding().owned_port.is_none());
    assert!(typed.stage_actual_values(Some(selected.clone())).is_err());
    assert!(typed.binding().actual_values.is_none());
    assert!(slot.read().unwrap().provider_context.as_ref().unwrap().actual_values.is_none());
    let candidate = controller(&settings, "custom");
    candidate.stage_actual_values(Some(selected.clone())).unwrap();
    candidate.stage_actual_binding(selected.clone(), port.clone()).unwrap();
    candidate.stage_actual_binding(selected.clone(), port.clone()).unwrap();
    assert!(candidate.stage_actual_binding(alias.clone(), port.clone()).is_err());
    assert!(candidate.stage_actual_binding(selected.clone(), other_port.clone()).is_err());
    candidate.stage_actual_values(Some(selected.clone())).unwrap();
    assert!(candidate.stage_actual_values(Some(alias.clone())).is_err());
    assert!(candidate.stage_actual_values(None).is_err());
    assert!(
        slot.read().unwrap().provider_context.as_ref().unwrap().actual_values.is_none(),
        "candidate stays unpublished"
    );
    publication.publish(&candidate, ExecutionSnapshot::from_config(&config(model("custom"))), None);
    settings.set(Mode::On).unwrap();
    publication.set_append_only_context(None).unwrap();
    settings.set(Mode::Off).unwrap();
    candidate.publish_model(&model("different-provider"), None);
    candidate.stage_actual_binding(selected.clone(), port.clone()).unwrap();
    assert!(candidate.stage_actual_binding(alias.clone(), port.clone()).is_err());
    assert!(candidate.stage_actual_binding(selected.clone(), other_port).is_err());
    assert!(Arc::ptr_eq(slot.read().unwrap().provider_context.as_ref().unwrap().owned_port.as_ref().unwrap(), &port,));
    candidate.stage_actual_values(Some(selected.clone())).unwrap();
    assert!(candidate.stage_actual_values(Some(alias)).is_err());
    assert!(candidate.stage_actual_values(None).is_err());
    assert!(Arc::ptr_eq(
        &slot.read().unwrap().provider_context.as_ref().unwrap().actual_values.clone().unwrap(),
        &selected
    ));
    typed.set_append_only_context(None);
    assert!(Arc::ptr_eq(
        &slot.read().unwrap().provider_context.as_ref().unwrap().actual_values.clone().unwrap(),
        &selected
    ));
}
