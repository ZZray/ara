//! Grouped remote policy/preparation families from fixed OMP 596f2da.
//! These typed transports exercise Core/Session contracts; the supplied-binary
//! loopback runner separately proves actual Host/wire behavior.
use ara_agent::{
    AgentConfig, NoHooks,
    compaction::SummaryOptions,
    remote::{self, RemoteCompactionTransport, RemoteSettings},
};
use ara_ai::{
    AssistantBlock, AssistantMessage, Context, Message, Model, ModelProvider, ProviderError, Usage, UserMessage,
    remote_compaction::*,
};
use ara_cli::remote_compaction::{self as host, PreparedRemoteSummary};
use ara_session::SessionJournal;
use async_trait::async_trait;
use serde_json::json;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

struct Fixture {
    results: Mutex<VecDeque<Result<RemoteResult, RemoteError>>>,
    native: Mutex<Vec<(Context, RemoteNativeRequest)>>,
    generic: Mutex<Vec<RemoteGenericRequest>>,
}
impl Fixture {
    fn new(results: Vec<Result<RemoteResult, RemoteError>>) -> Arc<Self> {
        Arc::new(Self { results: Mutex::new(results.into()), native: Mutex::new(vec![]), generic: Mutex::new(vec![]) })
    }
}
#[async_trait]
impl RemoteCompactionTransport for Fixture {
    async fn native(
        &self,
        _: &Model,
        context: &Context,
        request: RemoteNativeRequest,
        _: &CancellationToken,
    ) -> Result<RemoteResult, RemoteError> {
        self.native.lock().unwrap().push((context.clone(), request));
        self.results.lock().unwrap().pop_front().expect("only planned native invocations")
    }
    async fn generic(
        &self,
        _: &Model,
        _: &str,
        request: RemoteGenericRequest,
        _: &CancellationToken,
    ) -> Result<RemoteResult, RemoteError> {
        self.generic.lock().unwrap().push(request);
        self.results.lock().unwrap().pop_front().expect("only planned generic invocations")
    }
}
struct UnusedProvider;
impl ModelProvider for UnusedProvider {
    fn stream(&self, _: &Model, _: &Context, _: ara_ai::CallOptions) -> ara_ai::AssistantStream {
        panic!("ordinary primary model must not run in preparation")
    }
}
fn model() -> Model {
    Model {
        id: "remote-fixture".into(),
        api: "openai-responses".into(),
        provider: "openai".into(),
        base_url: "http://fixture.invalid/v1".into(),
        reasoning: false,
        max_tokens: None,
        context_window: None,
        tokenizer: None,
    }
}
fn config(model: Model) -> AgentConfig {
    AgentConfig {
        model,
        provider: Arc::new(UnusedProvider),
        system_prompt: vec!["original base instructions".into()],
        tools: vec![],
        tool_choice: None,
        max_tokens: None,
        temperature: None,
        deadline: None,
        max_model_calls: None,
        hooks: Arc::new(NoHooks),
    }
}
fn attempt(input: u64, error: bool) -> RemoteAttempt {
    RemoteAttempt {
        elapsed_ms: 1,
        usage: Usage { input: Some(input), ..Usage::unknown() },
        error: error.then(|| "fixture failure".into()),
        status: None,
    }
}
fn native_result(marker: &str) -> RemoteResult {
    let item = json!({"type":"compaction","encrypted_content":marker});
    RemoteResult {
        summary: "Remote compaction".into(),
        short_summary: None,
        preserve_data: Some(
            json!({"openaiRemoteCompaction":{"provider":"openai","compactionItem":item,"replacementHistory":[item]}}),
        ),
        usage: Usage::unknown(),
        attempts: vec![attempt(9, false)],
    }
}
fn text_result(summary: &str) -> RemoteResult {
    RemoteResult {
        summary: summary.into(),
        short_summary: None,
        preserve_data: None,
        usage: Usage::unknown(),
        attempts: vec![attempt(7, false)],
    }
}
fn turn(journal: &mut SessionJournal, text: &str) -> Vec<String> {
    let mut assistant = AssistantMessage::empty("openai-responses", "openai", "remote-fixture");
    assistant.content.push(AssistantBlock::text(format!("completed {text}")));
    vec![
        journal.append_message(&Message::User(UserMessage::text(text))).unwrap(),
        journal.append_message(&Message::Assistant(assistant)).unwrap(),
    ]
}
fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(3)
}

#[tokio::test]
async fn native_selection_fallback_auth_error_and_cancellation_family() {
    let active = model();
    let context = Context { messages: vec![Message::User(UserMessage::text("full original"))], ..Context::default() };
    let options = RemoteConfig { enabled: Some(true), v2_streaming_enabled: Some(true), ..Default::default() };
    let fixture = Fixture::new(vec![
        Err(RemoteError {
            cause: ProviderError::Config("first non-auth failure".into()),
            auth_failed: false,
            attempts: vec![attempt(2, true)],
        }),
        Err(RemoteError {
            cause: ProviderError::Config("later auth unavailable".into()),
            auth_failed: true,
            attempts: vec![attempt(3, true)],
        }),
    ]);
    let error = remote::compact_provider_native(
        fixture.as_ref(),
        &active,
        &context,
        &options,
        &RemoteSettings::default(),
        "base",
        None,
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert!(!error.auth_failed);
    assert!(error.to_string().contains("first non-auth"));
    assert_eq!(error.attempts.len(), 2);
    assert_eq!(error.attempts[1].usage.input, Some(3));
    assert_eq!(
        fixture.native.lock().unwrap().iter().map(|(_, request)| request.version).collect::<Vec<_>>(),
        [RemoteVersion::V2, RemoteVersion::V1]
    );
    let cancelled = Fixture::new(vec![Err(RemoteError {
        cause: ProviderError::Aborted,
        auth_failed: false,
        attempts: vec![attempt(5, true)],
    })]);
    let error = remote::compact_provider_native(
        cancelled.as_ref(),
        &active,
        &context,
        &options,
        &RemoteSettings::default(),
        "base",
        None,
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert!(remote::cancelled(&error));
    assert_eq!(cancelled.native.lock().unwrap().len(), 1);
    assert!(cancelled.generic.lock().unwrap().is_empty());
}

#[tokio::test]
async fn native_whole_history_checked_publication_reopen_and_iterative_replay_family() {
    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    let active = model();
    assert!(host::route_context(&journal, &active, None, &RemoteSettings::default(), true).unwrap().is_empty());
    let damaged_path = directory.path().join("damaged-empty.jsonl");
    let damaged_bytes = format!("{}\n{{malformed\n", journal.header());
    std::fs::write(&damaged_path, &damaged_bytes).unwrap();
    let damaged = SessionJournal::open(&damaged_path).unwrap();
    assert!(host::route_context(&damaged, &active, None, &RemoteSettings::default(), true).is_err());
    assert_eq!(std::fs::read_to_string(&damaged_path).unwrap(), damaged_bytes);
    let covered_image = ara_ai::ImageContent { data: "aQ==".into(), mime_type: "image/png".into() };
    journal
        .append_message(&Message::User(UserMessage {
            content: ara_ai::UserContent::Blocks(vec![
                ara_ai::UserBlock::text("original ALPHA-4242"),
                ara_ai::UserBlock::Image(covered_image),
            ]),
            synthetic: None,
            timestamp: 1,
        }))
        .unwrap();
    let mut original_answer = AssistantMessage::empty("openai-responses", "openai", "remote-fixture");
    original_answer.content.push(AssistantBlock::text("completed original ALPHA-4242"));
    journal.append_message(&Message::Assistant(original_answer)).unwrap();
    let tail = turn(&mut journal, "original recent tail");
    let originals = journal.entries().to_vec();
    let fixture = Fixture::new(vec![Ok(native_result("first-opaque")), Ok(native_result("second-opaque"))]);
    let snapshot = journal.native_remote_snapshot(&active, true).unwrap();
    let prepared = host::prepare_remote(
        &snapshot,
        &config(active.clone()),
        fixture.clone(),
        &RemoteConfig::default(),
        &RemoteSettings::default(),
        1,
        None,
        None,
        SummaryOptions::default(),
        deadline(),
        &CancellationToken::new(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(prepared.summary, PreparedRemoteSummary::Native(_)));
    {
        let seen = fixture.native.lock().unwrap();
        assert_eq!(seen[0].0.messages.len(), 4);
        assert_eq!(seen[0].1.instructions, "original base instructions");
        assert!(seen[0].1.previous_replacement_history.is_none());
    }
    assert_eq!(prepared.replay_through_entry_id, tail[1]);
    prepared.commit(&mut journal, &snapshot, 200).unwrap();
    assert_eq!(&journal.entries()[..originals.len()], originals);
    let mut reopened = SessionJournal::open(journal.path()).unwrap();
    assert_eq!(host::route_context(&reopened, &active, None, &RemoteSettings::default(), false).unwrap().len(), 1);
    let boundary =
        host::native_reduction_boundary(&reopened, &active, None, &RemoteSettings::default(), false).unwrap().unwrap();
    let raw = reopened.raw_reduction_snapshot().unwrap();
    assert!(
        !ara_cli::local_reduction::prepare_images(raw.clone(), &active).unwrap().edits.is_empty(),
        "negative control finds the covered original image"
    );
    assert!(
        ara_cli::local_reduction::prepare_images_after_boundary(raw, &active, Some(&boundary))
            .unwrap()
            .edits
            .is_empty(),
        "covered source must survive a later readable fallback"
    );
    turn(&mut reopened, "new prefix");
    turn(&mut reopened, "new recent tail");
    let snapshot = reopened.native_remote_snapshot(&active, true).unwrap();
    let prepared = host::prepare_remote(
        &snapshot,
        &config(active.clone()),
        fixture.clone(),
        &RemoteConfig::default(),
        &RemoteSettings::default(),
        1,
        None,
        None,
        SummaryOptions::default(),
        deadline(),
        &CancellationToken::new(),
    )
    .await
    .unwrap()
    .unwrap();
    {
        let seen = fixture.native.lock().unwrap();
        assert_eq!(seen[1].0.messages.len(), 4, "old recent tail must not be sent again");
        assert_eq!(seen[1].1.previous_replacement_history.as_ref().unwrap()[0]["encrypted_content"], "first-opaque");
    }
    prepared.commit(&mut reopened, &snapshot, 100).unwrap();
    assert_eq!(reopened.entries().iter().filter(|entry| entry.kind == "compaction").count(), 2);
}

#[tokio::test]
async fn generic_readable_sources_second_short_call_and_atomic_reopen_family() {
    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    let mut active = model();
    active.api = "openai-completions".into();
    active.provider = "cas".into();
    turn(&mut journal, "original portable ALPHA-4242");
    turn(&mut journal, "kept recent changes");
    let fixture = Fixture::new(vec![
        Ok(text_result("history ALPHA-4242")),
        Ok(text_result("split changes")),
        Ok(text_result("I added ALPHA-4242.")),
    ]);
    let snapshot = journal.native_remote_snapshot(&active, false).unwrap();
    let settings = RemoteSettings { endpoint: Some("http://fixture.invalid/summary".into()), ..Default::default() };
    let prepared = host::prepare_remote(
        &snapshot,
        &config(active.clone()),
        fixture.clone(),
        &RemoteConfig::default(),
        &settings,
        1,
        None,
        Some(1000.0),
        SummaryOptions { oneshot_retry: None, max_tokens: None },
        deadline(),
        &CancellationToken::new(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(fixture.native.lock().unwrap().is_empty());
    let requests = fixture.generic.lock().unwrap();
    let short = requests.last().unwrap();
    assert_eq!(short.max_tokens, Some(200));
    assert!(short.prompt.contains("Summarize conversation changes as a pull request description."));
    assert!(short.prompt.contains("<previous-summary>"));
    assert!(short.prompt.contains("ALPHA-4242"));
    assert_eq!(prepared.attempts.len(), requests.len());
    drop(requests);
    prepared.commit(&mut journal, &snapshot, 200).unwrap();
    let reopened = SessionJournal::open(journal.path()).unwrap();
    let raw = reopened.entries().last().unwrap();
    assert_eq!(raw.raw["method"], "remote");
    assert_eq!(raw.raw["shortSummary"], "I added ALPHA-4242.");
    assert!(raw.raw.get("preserveData").is_none());
    assert!(
        reopened
            .model_context()
            .iter()
            .any(|message| matches!(message, Message::User(user) if user.content.plain_text().contains("ALPHA-4242")))
    );
}
