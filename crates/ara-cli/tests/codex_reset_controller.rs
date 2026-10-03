//! Saved-reset production controller families over real HTTP sockets and the
//! real reference CLI/RPC. Fakes prove bounded fault/consent/settlement paths;
//! they do not establish real Codex account or complete native Host acceptance.

#![cfg(feature = "test-fixture")]

#[path = "support/codex_reset_host.rs"]
mod codex_reset_host;

use ara_ai::Model;
use ara_cli::auth_storage::{AuthRequestContext, AuthStorage, AuthStorageOptions, FetchUsageReportsOptions};
use ara_cli::codex_auto_reset::{CodexAutoRedeemMode, CodexResetAction, CodexResetSettings};
use ara_cli::codex_reset_controller::{
    CodexResetConsent, CodexResetContext, CodexResetController, CodexResetCoordinator, CodexResetHost,
};
use ara_cli::codex_reset_receipts::{
    ResetOperationReceipt, ResetReceiptBegin, ResetReceiptError, ResetReceiptState, ResetReceiptStore,
    SqliteResetReceiptStore,
};
use ara_cli::codex_usage::CodexUsageProvider;
use ara_cli::credential_store::{AuthCredential, SqliteCredentialStore};
use ara_cli::model_route::{CredentialIdentity, RequestAuthFailure, RequestAuthFailureKind};
use ara_cli::openai_codex_auth::OpenAiCodexAuth;
use async_trait::async_trait;
use base64::Engine as _;
use codex_reset_host::{Gate, Reply, ResetFixture};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

const WAIT: Duration = Duration::from_secs(15);

struct Policy {
    settings: Mutex<CodexResetSettings>,
    selection: Option<CodexResetConsent>,
    ui: bool,
    current: AtomicBool,
    selections: AtomicUsize,
    persisted: Mutex<Vec<CodexAutoRedeemMode>>,
    notices: Mutex<Vec<String>>,
}

impl Policy {
    fn new(mode: CodexAutoRedeemMode, ui: bool, selection: Option<CodexResetConsent>) -> Arc<Self> {
        Arc::new(Self {
            settings: Mutex::new(CodexResetSettings {
                auto_redeem: mode,
                salvage_horizon_hours: 0.0,
                ..Default::default()
            }),
            selection,
            ui,
            current: AtomicBool::new(true),
            selections: AtomicUsize::new(0),
            persisted: Mutex::new(Vec::new()),
            notices: Mutex::new(Vec::new()),
        })
    }

    fn mode(&self, mode: CodexAutoRedeemMode) {
        self.settings.lock().unwrap().auto_redeem = mode;
    }
}

#[async_trait]
impl CodexResetHost for Policy {
    fn settings(&self) -> Result<CodexResetSettings, String> {
        Ok(*self.settings.lock().unwrap())
    }
    fn persist_mode(&self, mode: CodexAutoRedeemMode) -> Result<(), String> {
        self.mode(mode);
        self.persisted.lock().unwrap().push(mode);
        Ok(())
    }
    fn has_ui(&self) -> bool {
        self.ui
    }
    async fn select(&self, _: &[CodexResetAction]) -> Result<Option<CodexResetConsent>, String> {
        self.selections.fetch_add(1, Ordering::AcqRel);
        Ok(self.selection)
    }
    fn notice(&self, _: &str, message: &str) {
        self.notices.lock().unwrap().push(message.into());
    }
    fn is_current(&self, _: &CodexResetContext) -> bool {
        self.current.load(Ordering::Acquire)
    }
}

#[derive(Clone, Copy)]
enum Consume {
    Reset,
    Unknown500,
    Malformed,
    NothingToReset,
    AlreadyRedeemed,
}

struct WireState {
    balances: Mutex<HashMap<String, usize>>,
    used: AtomicUsize,
    expiry: String,
    reset_at: i64,
    consume: Consume,
    post_gate: Option<Arc<Gate>>,
    detail_gate: Option<Arc<Gate>>,
    usage_gate: Option<Arc<Gate>>,
    detail_gate_after: usize,
    detail_gate_after_model: bool,
    details: AtomicUsize,
    after_post: Option<Arc<dyn Fn() + Send + Sync>>,
    model_calls: AtomicUsize,
    quota_until_reset: bool,
    model_replies: Mutex<VecDeque<Reply>>,
    exhaust_on_model: bool,
    usage_by_account: Mutex<HashMap<String, (usize, i64)>>,
}

impl WireState {
    fn new(accounts: &[&str], used: usize, consume: Consume) -> Self {
        Self {
            balances: Mutex::new(accounts.iter().map(|account| ((*account).into(), 1)).collect()),
            used: AtomicUsize::new(used),
            expiry: (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
            reset_at: chrono::Utc::now().timestamp() + 7200,
            consume,
            post_gate: None,
            detail_gate: None,
            usage_gate: None,
            detail_gate_after: 0,
            detail_gate_after_model: false,
            details: AtomicUsize::new(0),
            after_post: None,
            model_calls: AtomicUsize::new(0),
            quota_until_reset: false,
            model_replies: Mutex::new(VecDeque::new()),
            exhaust_on_model: false,
            usage_by_account: Mutex::new(HashMap::new()),
        }
    }

    async fn start(self) -> (Arc<Self>, ResetFixture) {
        let state = Arc::new(self);
        let route = state.clone();
        let fixture = ResetFixture::start(move |request| {
            if request.path.ends_with("/wham/usage") {
                let count = route.balances.lock().unwrap().get(request.account()).copied().unwrap_or(0);
                let (used, reset_at) = route.usage_by_account.lock().unwrap().get(request.account()).copied()
                    .map(|(used, reset)| (if reset <= chrono::Utc::now().timestamp() { 0 } else { used }, reset))
                    .unwrap_or((route.used.load(Ordering::Acquire), route.reset_at));
                let reply = Reply::json(200, json!({"plan_type":"pro","rate_limit":{
                    "allowed":used < 100,"limit_reached":used >= 100,
                    "primary_window":{"used_percent":used,"limit_window_seconds":18000,"reset_at":reset_at}},
                    "rate_limit_reset_credits":{"available_count":count}}));
                return if route.model_calls.load(Ordering::Acquire) > 0 && let Some(gate) = &route.usage_gate { reply.held(gate) } else { reply };
            }
            if request.path.ends_with("/wham/rate-limit-reset-credits") {
                let count = route.balances.lock().unwrap().get(request.account()).copied().unwrap_or(0);
                let credits: Vec<_> = (0..count).map(|index| json!({"id":format!("credit-{index}"),"status":"available","expires_at":route.expiry})).collect();
                let reply = Reply::json(200, json!({"available_count":count,"credits":credits}));
                let index = route.details.fetch_add(1, Ordering::AcqRel);
                return if index >= route.detail_gate_after
                    && (!route.detail_gate_after_model || route.model_calls.load(Ordering::Acquire) > 0)
                    && let Some(gate) = &route.detail_gate { reply.held(gate) } else { reply };
            }
            if request.method == "POST" && request.path.ends_with("/wham/rate-limit-reset-credits/consume") {
                assert_eq!(request.body["account_id"], request.account());
                assert!(uuid::Uuid::parse_str(request.body["redeem_request_id"].as_str().unwrap()).is_ok());
                assert_eq!(request.body["credit_id"], "credit-0");
                if let Some(callback) = &route.after_post { callback(); }
                let reply = match route.consume {
                    Consume::Reset => {
                        route.balances.lock().unwrap().insert(request.account().into(), 0);
                        route.used.store(0, Ordering::Release);
                        Reply::json(200, json!({"code":"reset"}))
                    }
                    Consume::Unknown500 => Reply::json(500, json!({"code":"reset"})),
                    Consume::Malformed => Reply::malformed(),
                    Consume::NothingToReset => Reply::json(200, json!({"code":"nothing_to_reset"})),
                    Consume::AlreadyRedeemed => Reply::json(409, json!({"code":"already_redeemed"})),
                };
                return if let Some(gate) = &route.post_gate { reply.held(gate) } else { reply };
            }
            if request.method == "POST" && request.path.ends_with("/codex/responses") {
                let index = route.model_calls.fetch_add(1, Ordering::AcqRel);
                if index == 0 && route.exhaust_on_model {
                    route.used.store(100, Ordering::Release);
                }
                if let Some(reply) = route.model_replies.lock().unwrap().pop_front() {
                    return reply;
                }
                if route.quota_until_reset && route.used.load(Ordering::Acquire) >= 100 {
                    // Fixed fetch-retry returns a quota hint beyond its delay
                    // ceiling to the Host instead of doing ordinary backoff.
                    return Reply::json(429, json!({"error":{"code":"usage_limit_reached","message":"Usage limit reached; reset in 2 hours"}})).retry_after(7200);
                }
                return Reply::text("Saved reset completed this turn.");
            }
            Reply::json(404, json!({"error":"unexpected saved-reset fixture route"}))
        }).await;
        (state, fixture)
    }
}

fn seed(path: &Path, account: &str) -> i64 {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&json!({
            "https://api.openai.com/auth":{"chatgpt_account_id":account},
            "https://api.openai.com/profile":{"email":format!("{account}@example.invalid")}
        }))
        .unwrap(),
    );
    let store = SqliteCredentialStore::open(path).unwrap();
    let rows = store.upsert_auth_credential_for_provider("openai-codex", &AuthCredential::oauth(json!({
        "access":format!("e30.{payload}.synthetic"),"refresh":"synthetic-refresh",
        "expires":chrono::Utc::now().timestamp_millis() + 3_600_000,"accountId":account,"email":format!("{account}@example.invalid")
    }).as_object().unwrap().clone())).unwrap();
    rows.iter()
        .find(|row| match &row.credential {
            AuthCredential::OAuth { fields } => fields.get("accountId").and_then(Value::as_str) == Some(account),
            _ => false,
        })
        .unwrap()
        .id
}

async fn storage(path: &Path, fixture: &ResetFixture) -> Arc<AuthStorage> {
    let auth = Arc::new(
        OpenAiCodexAuth::open_with_endpoints(path.to_owned(), reqwest::Client::new(), &fixture.url).await.unwrap(),
    );
    let origin = fixture.url.clone();
    let transport = CodexUsageProvider::with_fixture_endpoint_resolver(Arc::new(move |canonical| {
        format!("{origin}{}", reqwest::Url::parse(canonical).unwrap().path())
    }))
    .unwrap();
    let storage = Arc::new(
        AuthStorage::for_codex(
            auth,
            AuthStorageOptions {
                reset_credit_client: Some(transport.clone()),
                usage_request_timeout: Duration::from_secs(8),
                jitter: Arc::new(|| 0.5),
                ..Default::default()
            },
        )
        .unwrap(),
    );
    storage.register_usage_provider("openai-codex", Arc::new(transport)).unwrap();
    storage
}

fn context(session: &str) -> CodexResetContext {
    CodexResetContext {
        provider: "openai-codex".into(),
        model_id: "reset-model".into(),
        host_epoch: None,
        usage: FetchUsageReportsOptions {
            context: AuthRequestContext { session_id: Some(session.into()), ..Default::default() },
            ..Default::default()
        },
    }
}

fn controller(
    storage: Arc<AuthStorage>,
    receipts: Arc<dyn ResetReceiptStore>,
    policy: Arc<Policy>,
    coordinator: Arc<CodexResetCoordinator>,
) -> Arc<CodexResetController> {
    CodexResetController::new(storage, receipts, policy, coordinator)
}

async fn blocked(
    controller: &Arc<CodexResetController>,
    storage: &Arc<AuthStorage>,
    session: &str,
    account: i64,
    cancel: &CancellationToken,
) -> Vec<ResetOperationReceipt> {
    assert!(storage.pin_session_oauth_account("openai-codex", session, account, None).unwrap());
    let identity = storage.get_oauth_account_identity("openai-codex", Some(session)).unwrap().unwrap();
    tokio::time::timeout(WAIT, controller.blocked(context(session), identity, None, cancel)).await.unwrap()
}

#[tokio::test]
async fn no_dismiss_and_headless_consent_never_spend_and_only_real_selection_persists() {
    for (mode, ui, choice, expected_posts, expected_persist) in [
        (CodexAutoRedeemMode::No, false, None, 0, None),
        (CodexAutoRedeemMode::Unset, false, None, 0, None),
        (CodexAutoRedeemMode::Unset, true, None, 0, None),
        (CodexAutoRedeemMode::Unset, true, Some(CodexResetConsent::No), 0, Some(CodexAutoRedeemMode::No)),
        (CodexAutoRedeemMode::Unset, true, Some(CodexResetConsent::Yes), 1, Some(CodexAutoRedeemMode::Yes)),
    ] {
        let home = tempfile::tempdir().unwrap();
        let id = seed(&home.path().join("auth.db"), "consent");
        let (_, wire) = WireState::new(&["consent"], 100, Consume::Reset).start().await;
        let storage = storage(&home.path().join("auth.db"), &wire).await;
        let policy = Policy::new(mode, ui, choice);
        let owner = controller(
            storage.clone(),
            Arc::new(SqliteResetReceiptStore::memory().unwrap()),
            policy.clone(),
            Arc::new(Default::default()),
        );
        let result = blocked(&owner, &storage, "consent-session", id, &CancellationToken::new()).await;
        assert_eq!(result.len(), expected_posts);
        assert_eq!(wire.count("POST", "/consume"), expected_posts);
        assert_eq!(*policy.persisted.lock().unwrap(), expected_persist.into_iter().collect::<Vec<_>>());
        if mode == CodexAutoRedeemMode::No {
            assert!(wire.requests.lock().unwrap().is_empty(), "No exits before eligibility IO");
        }
        if mode == CodexAutoRedeemMode::Unset && !ui {
            let _ = blocked(&owner, &storage, "consent-session", id, &CancellationToken::new()).await;
            assert_eq!(policy.notices.lock().unwrap().len(), 1, "headless notice is keyed to the same episode");
        }
        owner.close().await;
    }
}

#[tokio::test]
async fn concurrent_sessions_adopt_one_receipt_resolve_their_own_lease_and_close_settles_after_cancel() {
    for revoke_current in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let id = seed(&home.path().join("auth.db"), "shared");
        let gate = Arc::new(Gate::default());
        let mut state = WireState::new(&["shared"], 100, Consume::Reset);
        state.post_gate = Some(gate.clone());
        let (_, wire) = state.start().await;
        let storage = storage(&home.path().join("auth.db"), &wire).await;
        let receipts = Arc::new(SqliteResetReceiptStore::open(home.path().join("reset.db")).unwrap());
        let policy = Policy::new(CodexAutoRedeemMode::Yes, false, None);
        let owner = controller(storage.clone(), receipts.clone(), policy.clone(), Arc::new(Default::default()));
        let model = Model {
            id: "reset-model".into(),
            api: "openai-codex-responses".into(),
            provider: "openai-codex".into(),
            base_url: "https://chatgpt.com/backend-api".into(),
            reasoning: false,
            max_tokens: None,
            context_window: None,
            tokenizer: None,
        };
        let one = owner.decorate(storage.session_resolver(Some("one".into()), None), Some("one".into()), None);
        let two = one.for_session("two").unwrap();
        let cancel_one = CancellationToken::new();
        let cancel_two = CancellationToken::new();
        let failed_one = one.resolve(&model, &cancel_one).await.unwrap();
        let failed_two = two.resolve(&model, &cancel_two).await.unwrap();
        let failure = RequestAuthFailure { kind: RequestAuthFailureKind::Quota, retry_after_ms: Some(7_200_000.0) };
        let first = tokio::spawn({
            let (one, model, failure, cancel) = (one.clone(), model.clone(), failure.clone(), cancel_one.clone());
            async move { one.quota_reset(&model, &failed_one, &failure, &cancel).await.unwrap() }
        });
        wire.wait_count("POST", "/consume", 1).await;
        assert_eq!(
            receipts.receipts().unwrap()[0].state,
            ResetReceiptState::Pending,
            "Pending precedes the external POST"
        );
        let second = two.quota_reset(&model, &failed_two, &failure, &cancel_two);
        tokio::pin!(second);
        tokio::select! { biased; _ = &mut second => panic!("adopted pass must await the held consume"), _ = tokio::task::yield_now() => {} }
        cancel_one.cancel();
        assert!(first.await.unwrap().is_none(), "cancelled Run stops waiting but does not cancel consume");
        let close = tokio::spawn({
            let owner = owner.clone();
            async move { owner.close().await }
        });
        tokio::task::yield_now().await;
        assert!(!close.is_finished(), "Host shutdown waits for dispatch settlement");
        if revoke_current {
            policy.current.store(false, Ordering::Release);
        }
        gate.release();
        let replay = tokio::time::timeout(WAIT, second).await.unwrap().unwrap();
        assert_eq!(replay.is_none(), revoke_current, "stale callers cannot adopt a settled reset for replay");
        // The replay object intentionally hides bearer bytes. Resolve the second
        // Session independently and compare the public actual-row identity.
        assert!(
            matches!(two.resolve(&model, &CancellationToken::new()).await.unwrap().identity(), CredentialIdentity::Stored { id: actual, .. } if *actual == id)
        );
        drop(replay);
        tokio::time::timeout(WAIT, close).await.unwrap().unwrap();
        assert_eq!(wire.count("POST", "/consume"), 1);
        assert!(receipts.receipts().unwrap()[0].is_confirmed_reset());
    }
}

#[tokio::test]
async fn unknown_500_and_malformed_success_survive_restart_and_fresh_zero_credit_get() {
    for consume in [Consume::Unknown500, Consume::Malformed] {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("auth.db");
        seed(&path, "unknown");
        let (state, wire) = WireState::new(&["unknown"], 100, consume).start().await;
        let receipt_path = home.path().join("reset.db");
        let receipts = Arc::new(SqliteResetReceiptStore::open(&receipt_path).unwrap());
        let auth = storage(&path, &wire).await;
        let owner = controller(
            auth.clone(),
            receipts.clone(),
            Policy::new(CodexAutoRedeemMode::No, false, None),
            Arc::new(Default::default()),
        );
        let first = owner
            .manual(context("first"), "UNKNOWN@example.invalid", &CancellationToken::new())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.outcome.code, "outcome_unknown");
        let original = first.operation.unwrap();
        assert_eq!(original.state, ResetReceiptState::Unknown);
        owner.close().await;
        drop(owner);
        drop(auth);
        drop(receipts);
        // A provider may enrich row metadata while the original operation is
        // unresolved. The same authority and exact row must retain its fence.
        let store = SqliteCredentialStore::open(&path).unwrap();
        let row = store.list_auth_credentials(Some("openai-codex")).unwrap().into_iter().next().unwrap();
        let AuthCredential::OAuth { mut fields } = row.credential else { panic!("synthetic OAuth row") };
        fields.insert("accountId".into(), json!("enriched-unknown"));
        assert!(
            store
                .try_update_auth_credential_if_matches(
                    row.id,
                    &row.serialized_data,
                    &AuthCredential::oauth(fields),
                    None
                )
                .unwrap()
        );
        drop(store);
        state.balances.lock().unwrap().insert("unknown".into(), 0);
        let receipts = Arc::new(SqliteResetReceiptStore::open(&receipt_path).unwrap());
        let restarted_auth = storage(&path, &wire).await;
        assert_eq!(
            restarted_auth.list_oauth_accounts("openai-codex", Some("restart")).unwrap()[0].account_id.as_deref(),
            Some("enriched-unknown")
        );
        let restarted = controller(
            restarted_auth,
            receipts.clone(),
            Policy::new(CodexAutoRedeemMode::No, false, None),
            Arc::new(Default::default()),
        );
        let statuses = restarted.list(&context("restart"), &CancellationToken::new()).await.unwrap();
        assert_eq!(statuses[0].available_count, 0.0);
        let retry = restarted
            .manual(context("restart"), "UNKNOWN@example.invalid", &CancellationToken::new())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retry.outcome.code, "outcome_unknown", "fresh zero is not authority to clear the durable fence");
        assert_eq!(retry.operation.unwrap().request_id, original.request_id);
        assert_eq!(receipts.receipts().unwrap().len(), 1);
        assert_eq!(wire.count("POST", "/consume"), 1);
        restarted.close().await;
    }
}

#[tokio::test]
async fn authority_scope_isolates_same_account_and_row_ids_in_one_process_and_one_journal() {
    let home = tempfile::tempdir().unwrap();
    let first_path = home.path().join("first/auth.db");
    let second_path = home.path().join("second/auth.db");
    let first_id = seed(&first_path, "same-account");
    let second_id = seed(&second_path, "same-account");
    assert_eq!(first_id, second_id, "fixture deliberately reuses numeric row IDs");
    let (_, wire) = WireState::new(&["same-account"], 100, Consume::Unknown500).start().await;
    let shared = Arc::new(CodexResetCoordinator::default());
    let receipts = Arc::new(SqliteResetReceiptStore::open(home.path().join("reset.db")).unwrap());
    let first_auth = storage(&first_path, &wire).await;
    let second_auth = storage(&second_path, &wire).await;
    let first = controller(
        first_auth.clone(),
        receipts.clone(),
        Policy::new(CodexAutoRedeemMode::Yes, false, None),
        shared.clone(),
    );
    let second = controller(
        second_auth.clone(),
        receipts.clone(),
        Policy::new(CodexAutoRedeemMode::Yes, false, None),
        shared.clone(),
    );
    assert!(blocked(&first, &first_auth, "same-session", first_id, &CancellationToken::new()).await.is_empty());
    assert!(blocked(&second, &second_auth, "same-session", second_id, &CancellationToken::new()).await.is_empty());
    assert_eq!(wire.count("POST", "/consume"), 2, "attempt and unknown fence belong to each actual authority");
    let durable = receipts.receipts().unwrap();
    assert_eq!(durable.len(), 2);
    assert_ne!(durable[0].account_key, durable[1].account_key);
    first.close().await;
}

#[tokio::test]
async fn known_business_outcomes_defer_or_finish_without_retrying_the_same_episode() {
    for consume in [Consume::NothingToReset, Consume::AlreadyRedeemed] {
        let home = tempfile::tempdir().unwrap();
        let id = seed(&home.path().join("auth.db"), "known");
        let (_, wire) = WireState::new(&["known"], 100, consume).start().await;
        let auth = storage(&home.path().join("auth.db"), &wire).await;
        let receipts = Arc::new(SqliteResetReceiptStore::memory().unwrap());
        let owner = controller(
            auth.clone(),
            receipts.clone(),
            Policy::new(CodexAutoRedeemMode::Yes, false, None),
            Arc::new(Default::default()),
        );
        assert!(blocked(&owner, &auth, "known", id, &CancellationToken::new()).await.is_empty());
        assert_eq!(receipts.receipts().unwrap()[0].state, ResetReceiptState::Known);
        assert!(blocked(&owner, &auth, "known", id, &CancellationToken::new()).await.is_empty());
        assert_eq!(wire.count("POST", "/consume"), 1);
        owner.close().await;
    }
}

struct FailingFinish {
    inner: SqliteResetReceiptStore,
}
impl ResetReceiptStore for FailingFinish {
    fn begin(&self, receipt: &ResetOperationReceipt) -> Result<ResetReceiptBegin, ResetReceiptError> {
        self.inner.begin(receipt)
    }
    fn finish(&self, _: &ResetOperationReceipt) -> Result<(), ResetReceiptError> {
        Err(ResetReceiptError)
    }
    fn receipts(&self) -> Result<Vec<ResetOperationReceipt>, ResetReceiptError> {
        self.inner.receipts()
    }
}

#[tokio::test]
async fn confirmed_response_with_failed_durable_finish_keeps_success_feedback_and_pending_fence() {
    let home = tempfile::tempdir().unwrap();
    let id = seed(&home.path().join("auth.db"), "settlement");
    let (_, wire) = WireState::new(&["settlement"], 100, Consume::Reset).start().await;
    let auth = storage(&home.path().join("auth.db"), &wire).await;
    let receipts = Arc::new(FailingFinish { inner: SqliteResetReceiptStore::memory().unwrap() });
    let policy = Policy::new(CodexAutoRedeemMode::Yes, false, None);
    let owner = controller(auth.clone(), receipts.clone(), policy.clone(), Arc::new(Default::default()));
    assert!(
        blocked(&owner, &auth, "settlement", id, &CancellationToken::new()).await.is_empty(),
        "failed durable settlement cannot authorize immediate model replay"
    );
    assert_eq!(receipts.receipts().unwrap()[0].state, ResetReceiptState::Pending);
    assert!(
        policy
            .notices
            .lock()
            .unwrap()
            .iter()
            .any(|notice| notice.contains("Auto-redeemed") && notice.contains("could not be durably settled"))
    );
    let manual = owner.manual(context("settlement"), "active", &CancellationToken::new()).await.unwrap().unwrap();
    assert_eq!(manual.outcome.code, "outcome_unknown");
    assert_eq!(wire.count("POST", "/consume"), 1);
    owner.close().await;
}

#[tokio::test]
async fn cancelled_and_stale_admission_never_spends_and_policy_revocation_stops_remaining_sweep_actions() {
    let home = tempfile::tempdir().unwrap();
    seed(&home.path().join("auth.db"), "one");
    seed(&home.path().join("auth.db"), "two");
    let policy = Policy::new(CodexAutoRedeemMode::Yes, false, None);
    policy.settings.lock().unwrap().salvage_horizon_hours = 12.0;
    let mut state = WireState::new(&["one", "two"], 40, Consume::Reset);
    state.after_post = Some(Arc::new({
        let policy = policy.clone();
        move || policy.mode(CodexAutoRedeemMode::No)
    }));
    let (_, wire) = state.start().await;
    let auth = storage(&home.path().join("auth.db"), &wire).await;
    let owner = controller(
        auth.clone(),
        Arc::new(SqliteResetReceiptStore::memory().unwrap()),
        policy.clone(),
        Arc::new(Default::default()),
    );
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(owner.manual(context("cancelled"), "one", &cancelled).await.is_err());
    assert!(wire.requests.lock().unwrap().is_empty(), "already cancelled manual has no live GET or consume");
    policy.current.store(false, Ordering::Release);
    owner.refresh_if_stale(context("stale"));
    owner.coordinator().wait_for_settlement().await;
    assert!(wire.requests.lock().unwrap().is_empty(), "stale Host render never applies usage or reset planning");
    policy.current.store(true, Ordering::Release);
    let mut other_model = context("non-codex-active");
    other_model.provider = "custom".into();
    owner.fetch_usage_reports(&other_model, &CancellationToken::new()).await.unwrap();
    owner.coordinator().wait_for_settlement().await;
    assert_eq!(wire.count("POST", "/consume"), 1, "mode is re-read before each automatic action");
    owner.close().await;
}

#[tokio::test]
async fn auto_authorization_is_rechecked_after_live_credit_preparation() {
    let home = tempfile::tempdir().unwrap();
    seed(&home.path().join("auth.db"), "admission");
    let detail = Arc::new(Gate::default());
    let mut state = WireState::new(&["admission"], 40, Consume::Reset);
    state.detail_gate = Some(detail.clone());
    state.detail_gate_after = 1;
    let (_, wire) = state.start().await;
    let auth = storage(&home.path().join("auth.db"), &wire).await;
    let policy = Policy::new(CodexAutoRedeemMode::Yes, false, None);
    policy.settings.lock().unwrap().salvage_horizon_hours = 12.0;
    let receipts = Arc::new(SqliteResetReceiptStore::memory().unwrap());
    let owner = controller(auth.clone(), receipts.clone(), policy.clone(), Arc::new(Default::default()));
    // Seed a successful planner snapshot independently of the later live GET;
    // this reproduces settings revocation during the consume preparation IO.
    let reports = auth
        .fetch_usage_reports_with_options(&context("snapshot").usage, &CancellationToken::new())
        .await
        .unwrap()
        .unwrap();
    owner.schedule_sweep(context("admission"), reports);
    wire.wait_count("GET", "/rate-limit-reset-credits", 2).await;
    policy.mode(CodexAutoRedeemMode::No);
    detail.release();
    owner.coordinator().wait_for_settlement().await;
    assert_eq!(wire.count("POST", "/consume"), 0, "revoked policy during preparation cannot reach Pending or POST");
    assert!(receipts.receipts().unwrap().is_empty());
    owner.close().await;
}

#[tokio::test]
async fn sweep_and_blocked_pass_serialize_in_both_directions_and_freshness_coalesces() {
    for sweep_first in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let id = seed(&home.path().join("auth.db"), "serialized");
        let gate = Arc::new(Gate::default());
        let mut state = WireState::new(&["serialized"], 100, Consume::Reset);
        state.post_gate = Some(gate.clone());
        let (_, wire) = state.start().await;
        let auth = storage(&home.path().join("auth.db"), &wire).await;
        let policy = Policy::new(CodexAutoRedeemMode::Yes, false, None);
        policy.settings.lock().unwrap().salvage_horizon_hours = 12.0;
        let owner = controller(
            auth.clone(),
            Arc::new(SqliteResetReceiptStore::memory().unwrap()),
            policy,
            Arc::new(Default::default()),
        );
        assert!(auth.pin_session_oauth_account("openai-codex", "serialized", id, None).unwrap());
        let reports = auth
            .fetch_usage_reports_with_options(&context("serialized").usage, &CancellationToken::new())
            .await
            .unwrap()
            .unwrap();
        if sweep_first {
            owner.schedule_sweep(context("serialized"), reports.clone());
        }
        let blocked = tokio::spawn({
            let (owner, auth) = (owner.clone(), auth.clone());
            async move { blocked(&owner, &auth, "serialized", id, &CancellationToken::new()).await }
        });
        wire.wait_count("POST", "/consume", 1).await;
        if !sweep_first {
            owner.schedule_sweep(context("serialized"), reports);
        }
        tokio::task::yield_now().await;
        assert!(!blocked.is_finished());
        gate.release();
        assert_eq!(tokio::time::timeout(WAIT, blocked).await.unwrap().unwrap().len(), 1);
        owner.coordinator().wait_for_settlement().await;
        assert_eq!(wire.count("POST", "/consume"), 1, "the concurrent trigger adopts or waits for the same spend");
        auth.invalidate_usage_cache_and_notify(Some("openai-codex"), &CancellationToken::new()).await.unwrap();
        let before = wire.count("GET", "/wham/usage");
        for _ in 0..10 {
            owner.refresh_if_stale(context("render-session"));
        }
        owner.coordinator().wait_for_settlement().await;
        assert_eq!(wire.count("GET", "/wham/usage"), before + 1, "repeated renders share one freshness flight");
        for _ in 0..10 {
            owner.refresh_if_stale(context("render-session"));
        }
        owner.coordinator().wait_for_settlement().await;
        assert_eq!(
            wire.count("GET", "/wham/usage"),
            before + 1,
            "successful freshness suppresses another immediate render fetch"
        );
        owner.close().await;
    }
}

#[tokio::test]
async fn manual_cancellation_during_live_listing_does_not_start_owned_consume() {
    let home = tempfile::tempdir().unwrap();
    seed(&home.path().join("auth.db"), "manual-cancel");
    let gate = Arc::new(Gate::default());
    let mut state = WireState::new(&["manual-cancel"], 100, Consume::Reset);
    state.detail_gate = Some(gate.clone());
    let (_, wire) = state.start().await;
    let auth = storage(&home.path().join("auth.db"), &wire).await;
    let receipts = Arc::new(SqliteResetReceiptStore::memory().unwrap());
    let owner = controller(
        auth,
        receipts.clone(),
        Policy::new(CodexAutoRedeemMode::No, false, None),
        Arc::new(Default::default()),
    );
    let cancel = CancellationToken::new();
    let manual = tokio::spawn({
        let (owner, cancel) = (owner.clone(), cancel.clone());
        async move { owner.manual(context("cancel"), "active", &cancel).await }
    });
    wire.wait_count("GET", "/rate-limit-reset-credits", 1).await;
    cancel.cancel();
    gate.release();
    assert!(tokio::time::timeout(WAIT, manual).await.unwrap().unwrap().is_err());
    owner.close().await;
    assert_eq!(wire.count("POST", "/consume"), 0);
    assert!(receipts.receipts().unwrap().is_empty());
}

struct ProcessHost {
    home: tempfile::TempDir,
    work: tempfile::TempDir,
    sessions: PathBuf,
    auth: PathBuf,
    receipts: PathBuf,
    credential_id: i64,
}

impl ProcessHost {
    fn new(fixture: &ResetFixture, account: &str, mode: &str) -> Self {
        let home = tempfile::Builder::new().prefix("ara-reset-host-home-").tempdir().unwrap();
        let work = tempfile::Builder::new().prefix("ara-reset-host-work-").tempdir().unwrap();
        std::fs::create_dir(work.path().join(".git")).unwrap();
        let agent = home.path().join("agent");
        std::fs::create_dir(&agent).unwrap();
        let auth = agent.join("auth.db");
        let credential_id = seed(&auth, account);
        std::fs::write(
            agent.join("models.yml"),
            json!({"providers":{"openai-codex":{
                "api":"openai-codex-responses","baseUrl":fixture.url,"auth":"oauth",
                "models":[{"id":"reset-model","input":["text"]}]
            }}})
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            agent.join("config.yml"),
            format!(
                "codexResets:\n  autoRedeem: '{mode}'\n  salvageHorizonHours: 0\ncompaction:\n  methodOrder: [soft]\n"
            ),
        )
        .unwrap();
        // Synthetic grants must never be sent to the public model catalogue.
        // The existing real-device Host family uses this same authoritative
        // bundled fingerprint cache before exercising private Responses.
        let bundled = ara_cli::model_identity_wire::bundled_provider_models(&"openai-codex".into());
        let fingerprint = ara_cli::model_manager::fingerprint_static_models(
            &ara_cli::model_manager::ModelArray::new(bundled.clone()),
            true,
        );
        ara_cli::model_cache::SqliteModelCache::for_path(agent.join("model-cache.db"))
            .write_model_cache_wire(
                &"openai-codex".into(),
                chrono::Utc::now().timestamp_millis() as f64,
                &[],
                ara_cli::model_cache::WireModelCacheWriteOptions {
                    authoritative: true,
                    static_fingerprint: &fingerprint,
                    static_header_sources: &bundled,
                    restorable_header_fallback: None,
                },
            )
            .unwrap();
        let sessions = home.path().join("sessions");
        let receipts = agent.join("codex-reset-operations.db");
        Self { home, work, sessions, auth, receipts, credential_id }
    }

    fn command(&self, fixture: &ResetFixture, args: &[&str]) -> Command {
        self.command_tools(fixture, args, "")
    }

    fn command_tools(&self, fixture: &ResetFixture, args: &[&str], tools: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ara"));
        command.env_clear();
        for name in ["PATH", "SystemRoot", "WINDIR", "SystemDrive", "ComSpec", "PATHEXT"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command
            .env("HOME", self.home.path())
            .env("USERPROFILE", self.home.path())
            .env("ARA_HOME", self.home.path())
            .env("TEMP", self.home.path())
            .env("TMP", self.home.path())
            .env("ARA_TEST_CODEX_AUTH_BASE_URL", &fixture.url)
            .current_dir(self.work.path())
            .args([
                "--provider",
                "openai-codex",
                "--model",
                "reset-model",
                "--no-skills",
                "--tools",
                tools,
                "--max-model-calls",
                "4",
                "--compact-threshold",
                "0",
                "--session-dir",
            ])
            .arg(&self.sessions)
            .args(args)
            .stdin(Stdio::null());
        command
    }

    fn journal(&self) -> String {
        let path = std::fs::read_dir(&self.sessions)
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|extension| extension == "jsonl"))
            .unwrap();
        std::fs::read_to_string(path).unwrap()
    }
}

async fn output(command: Command) -> Output {
    tokio::time::timeout(WAIT, tokio::process::Command::from(command).kill_on_drop(true).output())
        .await
        .expect("reference Host process deadline")
        .unwrap()
}

fn success(output: &Output) -> String {
    assert_eq!(output.status.code(), Some(0), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout.clone()).unwrap()
}

async fn repl(host: &ProcessHost, fixture: &ResetFixture, input: &str) -> Output {
    let mut child = tokio::process::Command::from(host.command(fixture, &["--repl"]))
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).await.unwrap();
    tokio::time::timeout(WAIT, child.wait_with_output()).await.expect("saved-reset REPL deadline").unwrap()
}

#[tokio::test]
async fn real_cli_quota_reset_replays_once_and_persists_the_confirmed_operation_with_the_session() {
    let mut state = WireState::new(&["cli"], 100, Consume::Reset);
    state.quota_until_reset = true;
    let (_, wire) = state.start().await;
    let host = ProcessHost::new(&wire, "cli", "yes");
    let result = output(host.command(&wire, &["--mode", "json", "complete a bounded saved-reset turn"])).await;
    assert!(success(&result).contains("Saved reset completed this turn."));
    let diagnostics = String::from_utf8_lossy(&result.stderr);
    assert!(diagnostics.contains("Auto-redeemed"), "{diagnostics}");
    assert_eq!(wire.count("POST", "/codex/responses"), 2, "same bearer replay needs the confirmed reset receipt");
    assert_eq!(wire.count("POST", "/consume"), 1);
    let receipts = SqliteResetReceiptStore::open(&host.receipts).unwrap().receipts().unwrap();
    assert_eq!(receipts.len(), 1);
    assert!(receipts[0].is_confirmed_reset());
    assert!(host.auth.exists());
    let journal = host.journal();
    assert!(journal.contains("Saved reset completed this turn."));
    assert!(!journal.contains("synthetic-refresh"));
    let requests = wire.requests.lock().unwrap();
    let calls: Vec<_> = requests.iter().filter(|request| request.path.ends_with("/codex/responses")).collect();
    assert_eq!(calls[0].headers["session_id"], calls[1].headers["session_id"]);
    assert_eq!(calls[0].headers["chatgpt-account-id"], "cli");
    assert_eq!(calls[0].headers["authorization"], calls[1].headers["authorization"]);
}

#[tokio::test]
async fn real_repl_manual_list_and_selectors_work_with_no_auto_mode_and_unknown_restart_stays_fenced() {
    let (_, wire) = WireState::new(&["manual"], 100, Consume::Reset).start().await;
    let host = ProcessHost::new(&wire, "manual", "no");
    let result = repl(
        &host,
        &wire,
        "/usage reset\n/usage reset active\n/usage reset MANUAL@example.invalid\n/usage reset manual\n/exit\n",
    )
    .await;
    success(&result);
    let diagnostics = String::from_utf8_lossy(&result.stderr);
    assert!(diagnostics.contains("manual@example.invalid (active): 1 saved resets"), "{diagnostics}");
    assert!(diagnostics.contains("saved reset redeemed"));
    assert_eq!(wire.count("POST", "/consume"), 1, "manual is explicitly authorized even when auto mode is No");
    assert_eq!(wire.count("POST", "/codex/responses"), 0);
    assert!(SqliteResetReceiptStore::open(&host.receipts).unwrap().receipts().unwrap()[0].is_confirmed_reset());

    let (state, unknown_wire) = WireState::new(&["restart"], 100, Consume::Unknown500).start().await;
    let restart_host = ProcessHost::new(&unknown_wire, "restart", "no");
    let first = repl(&restart_host, &unknown_wire, "/usage reset active\n/exit\n").await;
    success(&first);
    assert!(String::from_utf8_lossy(&first.stderr).contains("reset outcome unknown"));
    let original = SqliteResetReceiptStore::open(&restart_host.receipts).unwrap().receipts().unwrap()[0].clone();
    state.balances.lock().unwrap().insert("restart".into(), 0);
    let second =
        repl(&restart_host, &unknown_wire, "/usage reset\n/usage reset RESTART@example.invalid\n/exit\n").await;
    success(&second);
    let diagnostics = String::from_utf8_lossy(&second.stderr);
    assert!(diagnostics.contains("0 saved resets") && diagnostics.contains("reset outcome unknown"), "{diagnostics}");
    assert_eq!(unknown_wire.count("POST", "/consume"), 1, "fresh zero and a new process do not clear Unknown");
    let restarted = SqliteResetReceiptStore::open(&restart_host.receipts).unwrap().receipts().unwrap();
    assert_eq!(restarted.len(), 1);
    assert_eq!(restarted[0].request_id, original.request_id);
}

#[tokio::test]
async fn real_codex_rpc_route_emits_reset_notice_and_retries_in_the_same_session() {
    let mut state = WireState::new(&["rpc"], 100, Consume::Reset);
    state.quota_until_reset = true;
    let (_, wire) = state.start().await;
    let host = ProcessHost::new(&wire, "rpc", "yes");
    let mut child = tokio::process::Command::from(host.command(&wire, &["--mode", "rpc"]))
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut stderr = child.stderr.take().unwrap();
    let errors = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).await.unwrap();
        String::from_utf8_lossy(&bytes).into_owned()
    });
    let mut seen = Vec::<Value>::new();
    tokio::time::timeout(WAIT, async {
        loop {
            let frame: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().expect("RPC ready frame")).unwrap();
            let ready = frame["type"] == "available_commands_update";
            seen.push(frame);
            if ready {
                break;
            }
        }
    })
    .await
    .expect("RPC startup deadline");
    stdin
        .write_all(
            b"{\"id\":\"reset-turn\",\"type\":\"prompt\",\"message\":\"complete a bounded saved-reset RPC turn\"}\n",
        )
        .await
        .unwrap();
    tokio::time::timeout(WAIT, async {
        loop {
            let frame: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().expect("RPC turn frame")).unwrap();
            let end = frame["type"] == "agent_end";
            seen.push(frame);
            if end {
                break;
            }
        }
    })
    .await
    .expect("RPC reset turn deadline");
    stdin
        .write_all(b"{\"id\":\"new\",\"type\":\"new_session\"}\n{\"id\":\"new-state\",\"type\":\"get_state\"}\n")
        .await
        .unwrap();
    let new_session = tokio::time::timeout(WAIT, async {
        loop {
            let frame: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().expect("RPC new Session frame")).unwrap();
            let state =
                (frame["type"] == "response" && frame["id"] == "new-state").then(|| frame["data"]["sessionId"].clone());
            seen.push(frame);
            if let Some(session) = state {
                break session;
            }
        }
    })
    .await
    .expect("RPC new Session admission deadline");
    stdin
        .write_all(b"{\"id\":\"new-turn\",\"type\":\"prompt\",\"message\":\"complete in the admitted new Session\"}\n")
        .await
        .unwrap();
    tokio::time::timeout(WAIT, async {
        loop {
            let frame: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().expect("RPC new turn frame")).unwrap();
            let end = frame["type"] == "agent_end";
            seen.push(frame);
            if end {
                break;
            }
        }
    })
    .await
    .expect("RPC rebound Codex Session deadline");
    drop(stdin);
    tokio::time::timeout(WAIT, async {
        while let Some(line) = lines.next_line().await.unwrap() {
            seen.push(serde_json::from_str(&line).unwrap());
        }
    })
    .await
    .expect("RPC shutdown drain deadline");
    let status = tokio::time::timeout(WAIT, child.wait()).await.unwrap().unwrap();
    let diagnostics = errors.await.unwrap();
    assert_eq!(status.code(), Some(0), "{diagnostics}");
    assert!(
        seen.iter().any(|frame| frame["type"] == "response" && frame["id"] == "reset-turn" && frame["success"] == true)
    );
    assert!(seen.iter().any(|frame| frame["type"] == "notice"
        && frame["source"] == "codex-auto-reset"
        && frame["message"].as_str().is_some_and(|message| message.contains("Auto-redeemed"))));
    assert!(
        seen.iter()
            .any(|frame| frame["type"] == "message_end"
                && frame.to_string().contains("Saved reset completed this turn."))
    );
    assert_eq!(seen.iter().filter(|frame| frame["type"] == "session_shutdown").count(), 1);
    assert_eq!(wire.count("POST", "/consume"), 1);
    assert_eq!(wire.count("POST", "/codex/responses"), 3);
    let calls = wire
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|request| request.path.ends_with("/codex/responses"))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(calls[0].headers["session_id"], calls[1].headers["session_id"]);
    assert_eq!(calls[2].headers["session_id"], new_session.as_str().unwrap());
    assert_ne!(
        calls[0].headers["session_id"], calls[2].headers["session_id"],
        "the new Host Session has a new Codex binding and epoch"
    );
    assert!(SqliteResetReceiptStore::open(&host.receipts).unwrap().receipts().unwrap()[0].is_confirmed_reset());
    assert!(host.journal().contains("Saved reset completed this turn."));
}

// These five coarse families exercise post-emitted Session ownership. The
// pre-output AuthRetryState families above remain distinct: a provider Start
// followed by a quota terminal must be attributed even when no text was shown.
#[derive(Clone, Copy)]
enum QuotaContent {
    Thinking,
    Whitespace,
    UnfinishedTool,
    CompletedTool,
    Visible,
    MixedVisibleTools,
}

fn quota_stream(content: QuotaContent, hint: bool) -> Reply {
    let mut events = vec![json!({"type":"response.created","response":{"id":"resp_quota_actual"}})];
    match content {
        QuotaContent::Thinking => events.push(json!({"type":"response.output_item.done","output_index":0,
            "item":{"type":"reasoning","id":"reasoning_quota","summary":[{"type":"summary_text","text":"bounded reasoning"}]}})),
        QuotaContent::Whitespace | QuotaContent::Visible | QuotaContent::MixedVisibleTools => {
            let text = if matches!(content, QuotaContent::Whitespace) { " \t\n" } else { "already delivered visible quota output" };
            events.push(json!({"type":"response.output_item.done","output_index":0,
                "item":{"type":"message","id":"msg_quota_actual","role":"assistant","content":[{"type":"output_text","text":text}]}}));
        }
        _ => {}
    }
    if matches!(content, QuotaContent::UnfinishedTool | QuotaContent::MixedVisibleTools) {
        events.push(json!({"type":"response.output_item.added","output_index":1,
            "item":{"type":"function_call","id":"fc_unfinished_quota","call_id":"unfinished-quota","name":"write"}}));
        events.push(json!({"type":"response.function_call_arguments.delta","output_index":1,"item_id":"fc_unfinished_quota","delta":"{\"path\":"}));
    }
    if matches!(content, QuotaContent::CompletedTool | QuotaContent::MixedVisibleTools) {
        events.push(json!({"type":"response.output_item.done","output_index":2,
            "item":{"type":"function_call","id":"fc_unexecuted_quota","call_id":"unexecuted-quota","name":"write",
                "arguments":"{\"path\":\"must-not-execute.txt\",\"content\":\"quota tool was never admitted\"}"}}));
    }
    events.push(json!({"type":"response.failed","response":{"id":"resp_quota_actual","status":"failed",
        "error":{"code":"usage_limit_reached","message":if hint { "Usage limit reached; reset in 2 hours" } else { "Usage limit reached" }}}}));
    Reply::events(events)
}

fn actual_write_stream() -> Reply {
    Reply::events([
        json!({"type":"response.created","response":{"id":"resp_actual_effect"}}),
        json!({"type":"response.output_item.done","output_index":0,
            "item":{"type":"function_call","id":"fc_actual_effect","call_id":"actual-effect","name":"write",
                "arguments":"{\"path\":\"quota-effect.txt\",\"content\":\"one retained actual effect\"}"}}),
        json!({"type":"response.completed","response":{"id":"resp_actual_effect","status":"completed"}}),
    ])
}

fn session_retry(host: &ProcessHost, enabled: bool, max_retries: usize, max_delay: usize) {
    let path = host.home.path().join("agent/config.yml");
    let mut config = std::fs::read_to_string(&path).unwrap();
    config.push_str(&format!("retry:\n  enabled: {enabled}\n  maxRetries: {max_retries}\n  baseDelayMs: 0\n  maxDelayMs: {max_delay}\n  modelFallback: false\n"));
    std::fs::write(path, config).unwrap();
}

struct QuotaRpc {
    child: tokio::process::Child,
    stdin: Option<tokio::process::ChildStdin>,
    lines: tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    errors: tokio::task::JoinHandle<String>,
    seen: Vec<Value>,
    deadline: tokio::time::Instant,
}

impl QuotaRpc {
    async fn start(host: &ProcessHost, wire: &ResetFixture, tools: &str) -> Self {
        let mut child = tokio::process::Command::from(host.command_tools(wire, &["--mode", "rpc"], tools))
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut stderr = child.stderr.take().unwrap();
        let errors = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).await.unwrap();
            String::from_utf8_lossy(&bytes).into_owned()
        });
        let mut host = Self { child, stdin, lines, errors, seen: vec![], deadline: tokio::time::Instant::now() + WAIT };
        host.until(|frame| frame["type"] == "available_commands_update").await;
        host
    }

    async fn send(&mut self, frame: Value) {
        tokio::time::timeout_at(self.deadline, self.stdin.as_mut().unwrap().write_all(format!("{frame}\n").as_bytes()))
            .await
            .expect("quota RPC input deadline")
            .unwrap();
    }

    async fn until(&mut self, predicate: impl Fn(&Value) -> bool) -> Value {
        loop {
            let line = tokio::time::timeout_at(self.deadline, self.lines.next_line())
                .await
                .unwrap_or_else(|error| {
                    panic!("quota RPC bounded child observation: {error}; seen={}", json!(self.seen))
                })
                .unwrap()
                .expect("quota RPC frame before EOF");
            let frame: Value = serde_json::from_str(&line).unwrap();
            self.seen.push(frame.clone());
            if predicate(&frame) {
                return frame;
            }
        }
    }

    async fn response(&mut self, id: &str) -> Value {
        if let Some(frame) = self.seen.iter().find(|frame| frame["type"] == "response" && frame["id"] == id) {
            return frame.clone();
        }
        self.until(|frame| frame["type"] == "response" && frame["id"] == id).await
    }

    async fn recovered_end(&mut self) -> Value {
        let end = self.until(|frame| frame["type"] == "auto_retry_end").await;
        assert_eq!(end["success"], true, "{end}");
        // The Host emits agent_end before the retry receipt. until() has
        // already consumed it; verify the completed replay instead of waiting
        // for a second event that the protocol does not promise.
        let completed = self.seen.iter().rev().find(|frame| frame["type"] == "agent_end").unwrap();
        assert_eq!(completed["messages"].as_array().unwrap().last().unwrap()["stopReason"], "stop", "{completed}");
        end
    }

    async fn state(&mut self, id: &str) -> Value {
        self.send(json!({"id":id,"type":"get_state"})).await;
        let response = self.response(id).await;
        assert_eq!(response["success"], true, "{response}");
        response["data"].clone()
    }

    async fn prompt(&mut self) {
        self.prompt_as("quota-turn", "exercise the bounded quota Session family").await;
    }

    async fn prompt_as(&mut self, id: &str, message: &str) {
        self.send(json!({"id":id,"type":"prompt","message":message})).await;
        assert_eq!(self.response(id).await["success"], true);
    }

    async fn finish(mut self) -> Vec<Value> {
        self.stdin.take();
        while let Some(line) = tokio::time::timeout_at(self.deadline, self.lines.next_line())
            .await
            .expect("quota RPC drain deadline")
            .unwrap()
        {
            self.seen.push(serde_json::from_str(&line).unwrap());
        }
        let status =
            tokio::time::timeout_at(self.deadline, self.child.wait()).await.expect("quota RPC exit deadline").unwrap();
        let diagnostics =
            tokio::time::timeout_at(self.deadline, self.errors).await.expect("quota RPC stderr deadline").unwrap();
        assert_eq!(status.code(), Some(0), "{diagnostics}");
        assert_eq!(self.seen.iter().filter(|frame| frame["type"] == "session_shutdown").count(), 1);
        assert!(
            !self.seen.iter().any(|frame| frame.to_string().contains("synthetic-refresh")),
            "private credential leaked"
        );
        self.seen
    }
}

fn quota_journal(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect()
}

fn quota_error(entries: &[Value]) -> &Value {
    entries
        .iter()
        .find(|entry| {
            entry["message"]["role"] == "assistant"
                && entry["message"]["errorMessage"].as_str().is_some_and(|error| error.contains("usage_limit_reached"))
        })
        .unwrap()
}

fn assert_quota_recorded(host: &ProcessHost) {
    let store = SqliteCredentialStore::open(&host.auth).unwrap();
    assert!(
        store.get_credential_block(host.credential_id, "openai-codex:oauth", "chat").unwrap().is_some(),
        "the actual settled quota must leave a durable block even when replay is vetoed; journal={}",
        host.journal()
    );
}

fn assert_quota_recovered(path: &Path, original: &Value, end: &Value, kind: &str) {
    assert_quota_recovered_with_users(path, original, end, kind, 1);
}

fn assert_quota_recovered_with_users(path: &Path, original: &Value, end: &Value, kind: &str, users: usize) {
    let entries = quota_journal(path);
    let updated = entries.iter().find(|entry| entry["id"] == original["id"]).unwrap();
    let receipt =
        end["retryErrors"].as_array().unwrap().iter().find(|receipt| receipt["entryId"] == original["id"]).unwrap();
    assert_eq!(receipt["retryRecovery"], updated["message"]["retryRecovery"]);
    assert_eq!(receipt["retryRecovery"]["status"], "recovered");
    assert_eq!(receipt["retryRecovery"]["recovery"], kind);
    assert_eq!(receipt["retryRecovery"]["attempt"], 1);
    let mut expected = original.clone();
    expected["message"]["retryRecovery"] = receipt["retryRecovery"].clone();
    assert_eq!(&expected, updated, "recovery may only annotate the original failed raw entry");
    assert_eq!(
        entries.iter().filter(|entry| entry["message"]["role"] == "user").count(),
        users,
        "same Session continuation must not append the original prompt twice"
    );
}

#[tokio::test]
async fn session_quota_discardable_or_unexecuted_output_switches_sibling_before_reset_and_keeps_raw_identity() {
    for content in
        [QuotaContent::Thinking, QuotaContent::Whitespace, QuotaContent::UnfinishedTool, QuotaContent::CompletedTool]
    {
        let replay = Arc::new(Gate::default());
        let state = WireState::new(&["first", "sibling"], 0, Consume::Reset);
        *state.model_replies.lock().unwrap() = VecDeque::from([
            quota_stream(content, true),
            Reply::text("sibling completed quota recovery").held(&replay),
        ]);
        let (_, wire) = state.start().await;
        let host = ProcessHost::new(&wire, "first", "yes");
        let sibling_id = seed(&host.auth, "sibling");
        session_retry(&host, true, 2, 300_000);
        let mut rpc = QuotaRpc::start(&host, &wire, "write").await;
        if matches!(content, QuotaContent::Thinking) {
            let initial = rpc.state("before-adoption").await;
            rpc.send(json!({"id":"adopt","type":"new_session"})).await;
            assert_eq!(rpc.response("adopt").await["success"], true);
            let adopted = rpc.state("after-adoption").await;
            assert_ne!(
                adopted["sessionId"], initial["sessionId"],
                "the quota observer must belong to the adopted Session"
            );
        }
        let origin = rpc.state("origin").await;
        let path = PathBuf::from(origin["sessionFile"].as_str().unwrap());
        rpc.prompt().await;
        let start = rpc.until(|frame| frame["type"] == "auto_retry_start").await;
        assert_eq!(start["delayMs"].as_f64(), Some(0.0));
        wire.wait_count("POST", "/codex/responses", 2).await;
        assert_eq!(wire.count("POST", "/consume"), 0, "a healthy sibling outranks saved-reset spending");
        let original = quota_error(&quota_journal(&path)).clone();
        assert!(original["message"].get("retryRecovery").is_none());
        assert!(!host.work.path().join("must-not-execute.txt").exists());
        replay.release();
        let end = rpc.recovered_end().await;
        let settled = rpc.state("settled").await;
        assert_eq!(settled["sessionId"], origin["sessionId"]);
        assert_eq!(settled["sessionFile"], origin["sessionFile"]);
        assert_quota_recovered(&path, &original, &end, "credential");
        rpc.finish().await;
        let calls = wire
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.path.ends_with("/codex/responses"))
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(calls.len(), 2);
        assert_ne!(calls[0].account(), calls[1].account());
        assert_eq!(calls[0].headers["session_id"], calls[1].headers["session_id"]);
        let failed_id = if calls[0].account() == "first" { host.credential_id } else { sibling_id };
        assert!(
            SqliteCredentialStore::open(&host.auth)
                .unwrap()
                .get_credential_block(failed_id, "openai-codex:oauth", "chat")
                .unwrap()
                .is_some(),
            "rotation must record the actual first request's rejected row"
        );
    }
}

#[tokio::test]
async fn session_quota_confirmed_reset_authorizes_same_account_but_unknown_and_malformed_do_not() {
    for consume in [Consume::Reset, Consume::Unknown500, Consume::Malformed] {
        let replay = Arc::new(Gate::default());
        let mut state = WireState::new(&["single"], 0, consume);
        state.exhaust_on_model = true;
        *state.model_replies.lock().unwrap() = VecDeque::from([
            quota_stream(QuotaContent::Thinking, true),
            Reply::text("confirmed Session reset replay").held(&replay),
        ]);
        let (_, wire) = state.start().await;
        let host = ProcessHost::new(&wire, "single", "yes");
        session_retry(&host, true, 2, 300_000);
        let mut rpc = QuotaRpc::start(&host, &wire, "").await;
        let origin = rpc.state("origin").await;
        let path = PathBuf::from(origin["sessionFile"].as_str().unwrap());
        rpc.prompt().await;
        if matches!(consume, Consume::Reset) {
            rpc.until(|frame| frame["type"] == "auto_retry_start").await;
            wire.wait_count("POST", "/codex/responses", 2).await;
            let original = quota_error(&quota_journal(&path)).clone();
            replay.release();
            let end = rpc.recovered_end().await;
            assert_quota_recovered(&path, &original, &end, "credential");
            assert_eq!(rpc.state("same-session").await["sessionId"], origin["sessionId"]);
        } else {
            // agent_end precedes Host quota preparation. Closing the input
            // before this admission would cancel the scenario being checked.
            wire.wait_count("POST", "/consume", 1).await;
            rpc.until(|frame| frame["type"] == "agent_end").await;
            assert!(!rpc.seen.iter().any(|frame| frame["type"] == "auto_retry_start"));
            assert!(quota_error(&quota_journal(&path))["message"].get("retryRecovery").is_none());
        }
        rpc.finish().await;
        assert_eq!(wire.count("POST", "/consume"), 1);
        let receipts = SqliteResetReceiptStore::open(&host.receipts).unwrap().receipts().unwrap();
        assert_eq!(receipts.len(), 1);
        if matches!(consume, Consume::Reset) {
            assert!(receipts[0].is_confirmed_reset());
            let calls = wire
                .requests
                .lock()
                .unwrap()
                .iter()
                .filter(|request| request.path.ends_with("/codex/responses"))
                .cloned()
                .collect::<Vec<_>>();
            assert_eq!(calls.len(), 2);
            assert_eq!(calls[0].headers["authorization"], calls[1].headers["authorization"]);
            assert_eq!(calls[0].headers["session_id"], calls[1].headers["session_id"]);
        } else {
            assert_eq!(receipts[0].state, ResetReceiptState::Unknown);
            assert_eq!(wire.count("POST", "/codex/responses"), 1);
            assert_quota_recorded(&host);
        }
    }
}

#[tokio::test]
async fn session_quota_visible_output_and_prior_actual_effect_remain_recorded_without_reset_or_replay() {
    for with_tools in [false, true] {
        let mut state = WireState::new(&["visible"], 0, Consume::Reset);
        state.exhaust_on_model = true;
        let mut replies = VecDeque::new();
        if with_tools {
            replies.push_back(actual_write_stream());
        }
        replies.push_back(quota_stream(
            if with_tools { QuotaContent::MixedVisibleTools } else { QuotaContent::Visible },
            true,
        ));
        *state.model_replies.lock().unwrap() = replies;
        let (_, wire) = state.start().await;
        let host = ProcessHost::new(&wire, "visible", "yes");
        session_retry(&host, true, 2, 300_000);
        let mut rpc = QuotaRpc::start(&host, &wire, "write").await;
        let path = PathBuf::from(rpc.state("origin").await["sessionFile"].as_str().unwrap());
        rpc.prompt().await;
        rpc.until(|frame| frame["type"] == "agent_end").await;
        assert!(
            !rpc.seen.iter().any(|frame| matches!(frame["type"].as_str(), Some("auto_retry_start" | "auto_retry_end")))
        );
        let frames = rpc.finish().await;
        assert_eq!(wire.count("POST", "/consume"), 0);
        assert_eq!(wire.count("POST", "/codex/responses"), if with_tools { 2 } else { 1 });
        assert_quota_recorded(&host);
        let entries = quota_journal(&path);
        let failed = quota_error(&entries);
        assert!(failed["message"].to_string().contains("already delivered visible quota output"));
        assert!(failed["message"].get("retryRecovery").is_none());
        assert!(frames.iter().any(|frame| frame["type"] == "message_end"
            && frame.to_string().contains("already delivered visible quota output")));
        assert!(!host.work.path().join("must-not-execute.txt").exists());
        if with_tools {
            assert_eq!(
                std::fs::read_to_string(host.work.path().join("quota-effect.txt")).unwrap(),
                "one retained actual effect"
            );
            assert_eq!(
                entries
                    .iter()
                    .filter(|entry| entry["message"]["role"] == "toolResult"
                        && entry["message"]["toolCallId"].as_str().is_some_and(|id| id.starts_with("actual-effect")))
                    .count(),
                1
            );
        }
    }
}

#[tokio::test]
async fn session_quota_disabled_budget_and_wait_windows_preserve_feedback_without_unbounded_calls() {
    for (enabled, budget, hint) in [(false, 2, true), (true, 0, true), (true, 2, false)] {
        let mut state = WireState::new(&["bounded"], 0, Consume::Reset);
        state.exhaust_on_model = hint;
        *state.model_replies.lock().unwrap() = VecDeque::from([quota_stream(QuotaContent::Thinking, hint)]);
        let (_, wire) = state.start().await;
        let host = ProcessHost::new(&wire, "bounded", if hint { "yes" } else { "no" });
        session_retry(&host, enabled, budget, 300_000);
        let mut rpc = QuotaRpc::start(&host, &wire, "").await;
        let observed_at = chrono::Utc::now().timestamp_millis();
        rpc.prompt().await;
        rpc.until(|frame| frame["type"] == "agent_end").await;
        if !hint {
            rpc.until(|frame| frame["type"] == "auto_retry_end").await;
        }
        let frames = rpc.finish().await;
        assert!(!frames.iter().any(|frame| frame["type"] == "auto_retry_start"));
        if !hint {
            assert!(frames.iter().any(|frame| frame["type"] == "auto_retry_end"
                && frame["success"] == false
                && frame["finalError"].as_str().is_some_and(|message| message.contains("exceeds retry.maxDelayMs"))));
        }
        assert_eq!(wire.count("POST", "/consume"), 0);
        assert_eq!(wire.count("POST", "/codex/responses"), 1);
        assert_quota_recorded(&host);
        if !hint {
            let until = SqliteCredentialStore::open(&host.auth)
                .unwrap()
                .get_credential_block(host.credential_id, "openai-codex:oauth", "chat")
                .unwrap()
                .unwrap();
            assert!(
                (1_795_000..=1_805_000).contains(&(until - observed_at)),
                "native no-hint quota backoff must be thirty minutes: {until} vs {observed_at}"
            );
        }
    }

    let state = WireState::new(&["current", "soon"], 0, Consume::Reset);
    let sibling_reset = chrono::Utc::now().timestamp() + 6;
    state.usage_by_account.lock().unwrap().insert("soon".into(), (100, sibling_reset));
    *state.model_replies.lock().unwrap() =
        VecDeque::from([quota_stream(QuotaContent::Thinking, true), Reply::text("earliest sibling became available")]);
    let (_, wire) = state.start().await;
    let host = ProcessHost::new(&wire, "current", "no");
    seed(&host.auth, "soon");
    session_retry(&host, true, 2, 300_000);
    let mut rpc = QuotaRpc::start(&host, &wire, "").await;
    rpc.prompt().await;
    let start = rpc.until(|frame| frame["type"] == "auto_retry_start").await;
    let delay = start["delayMs"].as_f64().unwrap();
    assert!(
        (1_000.0..=7_100.0).contains(&delay),
        "earliest sibling plus one second bounds the two-hour current-account hint: {start}"
    );
    rpc.recovered_end().await;
    rpc.finish().await;
    assert_eq!(wire.count("POST", "/consume"), 0);
    assert_eq!(wire.count("POST", "/codex/responses"), 2);
    let calls = wire
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|request| request.path.ends_with("/codex/responses"))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(calls[0].account(), "current");
    assert_eq!(calls[1].account(), "soon");
    assert_eq!(calls[0].headers["session_id"], calls[1].headers["session_id"]);
}

#[tokio::test]
async fn session_quota_preparation_controls_revoke_unadmitted_spend_and_admitted_consume_settles_without_replay() {
    for stop in ["abort", "abort_retry", "set_auto_retry", "new_session", "eof"] {
        let preparation = Arc::new(Gate::default());
        let fresh_replay = Arc::new(Gate::default());
        let mut state = WireState::new(&["preparation"], 0, Consume::Reset);
        state.exhaust_on_model = true;
        if matches!(stop, "abort" | "abort_retry" | "set_auto_retry") {
            state.usage_gate = Some(preparation.clone());
        } else {
            state.detail_gate = Some(preparation.clone());
            // Initial account preparation also lists credits. This family
            // holds quota recovery after the actual rejected model request.
            state.detail_gate_after_model = true;
        }
        let mut replies = VecDeque::from([quota_stream(QuotaContent::Thinking, true)]);
        if stop == "abort" {
            replies.push_back(quota_stream(QuotaContent::Thinking, true));
            replies.push_back(Reply::text("fresh prompt recovered in the same Session").held(&fresh_replay));
        }
        *state.model_replies.lock().unwrap() = replies;
        let (_, wire) = state.start().await;
        let host = ProcessHost::new(&wire, "preparation", "yes");
        session_retry(&host, true, 2, 300_000);
        let mut rpc = QuotaRpc::start(&host, &wire, "").await;
        let origin = rpc.state("before-preparation").await;
        rpc.prompt().await;
        preparation.wait_entered().await;
        assert_eq!(wire.count("POST", "/codex/responses"), 1, "stop={stop}: Gate must follow the actual quota");
        assert!(rpc.child.try_wait().unwrap().is_none(), "preparation Gate is observed in a live child");
        if stop == "eof" {
            rpc.stdin.take();
            rpc.until(|frame| frame["type"] == "agent_end").await;
        } else {
            rpc.send(if stop == "set_auto_retry" {
                json!({"id":"stop","type":stop,"enabled":false})
            } else {
                json!({"id":"stop","type":stop})
            })
            .await;
            let response = rpc.response("stop").await;
            assert_eq!(
                response["success"], true,
                "reader cancellation must be reachable while preparation is held: {response}"
            );
            assert_eq!(wire.count("POST", "/consume"), 0, "no new spend was admitted after the stop acknowledgement");
        }
        preparation.release();
        if stop == "abort" {
            let after_abort = rpc.state("after-abort-before-fresh-prompt").await;
            assert_eq!(after_abort["sessionId"], origin["sessionId"]);
            assert_eq!(
                wire.count("POST", "/consume"),
                0,
                "the revoked old job must not spend after its preparation response is released"
            );
            rpc.prompt_as("fresh-quota-turn", "continue with a new plain prompt after the prior abort").await;
            rpc.until(|frame| frame["type"] == "auto_retry_start").await;
            wire.wait_count("POST", "/codex/responses", 3).await;
            fresh_replay.wait_entered().await;
            assert_eq!(
                wire.count("POST", "/consume"),
                1,
                "the newly admitted prompt epoch may spend its own confirmed reset"
            );
            let path = PathBuf::from(origin["sessionFile"].as_str().unwrap());
            let before_recovery = quota_journal(&path);
            let failed = before_recovery
                .iter()
                .rev()
                .find(|entry| {
                    entry["message"]["role"] == "assistant"
                        && entry["message"]["errorMessage"]
                            .as_str()
                            .is_some_and(|error| error.contains("usage_limit_reached"))
                })
                .unwrap()
                .clone();
            fresh_replay.release();
            let end = rpc.recovered_end().await;
            let after_recovery = rpc.state("fresh-prompt-settled").await;
            assert_eq!(after_recovery["sessionId"], origin["sessionId"]);
            assert_eq!(after_recovery["sessionFile"], origin["sessionFile"]);
            assert_quota_recovered_with_users(&path, &failed, &end, "credential", 2);
        }
        rpc.finish().await;
        if stop == "abort" {
            assert_eq!(wire.count("POST", "/consume"), 1);
            assert_eq!(wire.count("POST", "/codex/responses"), 3);
            assert!(SqliteResetReceiptStore::open(&host.receipts).unwrap().receipts().unwrap()[0].is_confirmed_reset());
            let requests = wire.requests.lock().unwrap();
            let calls = requests
                .iter()
                .filter(|request| request.path.ends_with("/codex/responses"))
                .cloned()
                .collect::<Vec<_>>();
            assert!(calls.iter().all(|call| call.headers["session_id"] == origin["sessionId"].as_str().unwrap()));
            let fresh_quota = requests
                .iter()
                .enumerate()
                .filter(|(_, request)| request.path.ends_with("/codex/responses"))
                .nth(1)
                .unwrap()
                .0;
            let consume = requests
                .iter()
                .position(|request| request.method == "POST" && request.path.ends_with("/consume"))
                .unwrap();
            assert!(consume > fresh_quota, "the one consume must follow the fresh prompt's actual provider rejection");
        } else {
            assert_eq!(wire.count("POST", "/consume"), 0);
            assert_eq!(
                wire.count("POST", "/codex/responses"),
                1,
                "stop={stop}; requests={:?}",
                wire.requests
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|request| (&request.method, &request.path))
                    .collect::<Vec<_>>()
            );
            assert_quota_recorded(&host);
        }
    }

    for stop in ["abort", "new_session", "eof"] {
        let admitted = Arc::new(Gate::default());
        let mut state = WireState::new(&["admitted"], 0, Consume::Reset);
        state.exhaust_on_model = true;
        state.post_gate = Some(admitted.clone());
        *state.model_replies.lock().unwrap() = VecDeque::from([quota_stream(QuotaContent::Thinking, true)]);
        let (_, wire) = state.start().await;
        let host = ProcessHost::new(&wire, "admitted", "yes");
        session_retry(&host, true, 2, 300_000);
        let mut rpc = QuotaRpc::start(&host, &wire, "").await;
        rpc.prompt().await;
        wire.wait_count("POST", "/consume", 1).await;
        admitted.wait_entered().await;
        assert!(rpc.child.try_wait().unwrap().is_none(), "admitted POST Gate is observed in a live child");
        if stop == "eof" {
            rpc.stdin.take();
            rpc.until(|frame| frame["type"] == "agent_end").await;
        } else {
            rpc.send(json!({"id":"stop","type":stop})).await;
            assert_eq!(
                rpc.response("stop").await["success"],
                true,
                "stop acknowledgement must arrive before the admitted response is released"
            );
        }
        admitted.release();
        rpc.finish().await;
        assert_eq!(wire.count("POST", "/consume"), 1);
        assert_eq!(wire.count("POST", "/codex/responses"), 1, "a settled old-Session reset cannot replay after stop");
        let receipts = SqliteResetReceiptStore::open(&host.receipts).unwrap().receipts().unwrap();
        assert_eq!(receipts.len(), 1);
        assert!(receipts[0].is_confirmed_reset(), "already admitted consume must settle its owned durable receipt");
    }
}
