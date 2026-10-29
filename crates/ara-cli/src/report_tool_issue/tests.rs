//! Two grouped consumers preserve the fixed original29 identities. Controlled
//! ports prove deterministic faults; Native wrapper observations are explicit.
use super::*;
use std::{
    collections::VecDeque,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
};
use tokio::sync::{Notify, Semaphore};
#[derive(Default)]
struct Settings(Mutex<AutoQaConfig>);
impl AutoQaSettings for Settings {
    fn snapshot(&self) -> AutoQaConfig {
        self.0.lock().unwrap().clone()
    }
    fn set_consent(&self, c: Consent) -> Result<(), String> {
        self.0.lock().unwrap().consent = c;
        Ok(())
    }
}
fn settings(c: Consent) -> Arc<dyn AutoQaSettings> {
    Arc::new(Settings(Mutex::new(AutoQaConfig {
        consent: c,
        endpoint: Some("http://127.0.0.1/fixture".into()),
        ..Default::default()
    })))
}
fn mark(id: &str) {
    println!("original-report-issue:pass:{id}");
}
struct Clock(AtomicU64);
impl AutoQaClock for Clock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}
#[derive(Default)]
struct Http {
    requests: Mutex<Vec<(String, Option<String>, serde_json::Value)>>,
    status: Mutex<VecDeque<Result<u16, HttpFailure>>>,
    hold: Mutex<Option<Arc<Semaphore>>>,
    entered: Notify,
}
#[async_trait::async_trait]
impl AutoQaHttp for Http {
    async fn post(
        &self,
        url: &str,
        token: Option<&str>,
        body: serde_json::Value,
    ) -> Result<u16, HttpFailure> {
        self.requests
            .lock()
            .unwrap()
            .push((url.into(), token.map(str::to_owned), body));
        self.entered.notify_one();
        let hold = self.hold.lock().unwrap().take();
        if let Some(hold) = hold {
            hold.acquire().await.unwrap().forget();
        }
        self.status.lock().unwrap().pop_front().unwrap_or(Ok(200))
    }
}
struct Handler {
    calls: AtomicUsize,
    answer: Mutex<Result<Option<bool>, String>>,
    hold: Option<Arc<Semaphore>>,
    entered: Notify,
}
impl Handler {
    fn new(answer: Result<Option<bool>, String>, held: bool) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            answer: Mutex::new(answer),
            hold: held.then(|| Arc::new(Semaphore::new(0))),
            entered: Notify::new(),
        })
    }
}
#[async_trait::async_trait]
impl AutoQaConsentHandler for Handler {
    async fn decide(&self) -> Result<Option<bool>, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        if let Some(hold) = &self.hold {
            hold.acquire().await.unwrap().forget();
        }
        self.answer.lock().unwrap().clone()
    }
}
struct Fixture {
    _dir: tempfile::TempDir,
    owner: Arc<AutoQaProcessOwner>,
    http: Arc<Http>,
    clock: Arc<Clock>,
    db: PathBuf,
    installs: Arc<AtomicUsize>,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("autoqa.db");
        let http = Arc::new(Http::default());
        let clock = Arc::new(Clock(AtomicU64::new(100000)));
        let installs = Arc::new(AtomicUsize::new(0));
        let count = installs.clone();
        let owner = AutoQaProcessOwner::new(AutoQaHostPorts {
            database_path: Some(db.clone()),
            version: "test-version".into(),
            platform: "win32".into(),
            arch: "x64".into(),
            install_id: Arc::new(move || {
                count.fetch_add(1, Ordering::SeqCst);
                "11111111-2222-3333-4444-555555555555".into()
            }),
            clock: clock.clone(),
            http: http.clone(),
        });
        Self {
            _dir: dir,
            owner,
            http,
            clock,
            db,
            installs,
        }
    }
    fn seed(&self, n: usize) {
        for i in 0..n {
            self.owner
                .insert("test-model", "glob", &format!("r-{i}"))
                .unwrap();
        }
    }
    fn counts(&self) -> (usize, usize) {
        self.owner
            .with_db(|db| {
                db.query_row(
                    "SELECT COUNT(*),COALESCE(SUM(pushed),0) FROM grievances",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
            })
            .unwrap()
    }
    async fn settle(&self) {
        let jobs = std::mem::take(&mut *self.owner.jobs.lock().unwrap());
        for job in jobs {
            job.task.await.unwrap();
        }
    }
}

#[tokio::test]
async fn fixed_report_issue_consent8_grouped() {
    let f = Fixture::new();
    let local = settings(Consent::Unset);
    assert!(!f.owner.resolve_consent(Some(local.clone())).await);
    assert_eq!(local.snapshot().consent, Consent::Unset);
    assert!(!f.db.exists());
    mark("B-d725e0bb4b");
    for (consent, expected, id) in [
        (Consent::Granted, true, "B-d766867896"),
        (Consent::Denied, false, "B-fa2d680621"),
    ] {
        let f = Fixture::new();
        let handler = Handler::new(Ok(Some(!expected)), false);
        f.owner.register_consent(Some(handler.clone()), None);
        assert_eq!(
            f.owner.resolve_consent(Some(settings(consent))).await,
            expected
        );
        assert_eq!(handler.calls.load(Ordering::SeqCst), 0);
        mark(id);
    }
    let f = Fixture::new();
    let local = settings(Consent::Unset);
    let persistent = settings(Consent::Unset);
    let handler = Handler::new(Ok(Some(true)), true);
    f.owner
        .register_consent(Some(handler.clone()), Some(persistent.clone()));
    let tasks = (0..3)
        .map(|_| {
            let owner = f.owner.clone();
            let settings = local.clone();
            tokio::spawn(async move { owner.resolve_consent(Some(settings)).await })
        })
        .collect::<Vec<_>>();
    handler.entered.notified().await;
    handler.hold.as_ref().unwrap().add_permits(1);
    for task in tasks {
        assert!(task.await.unwrap())
    }
    assert_eq!(handler.calls.load(Ordering::SeqCst), 1);
    assert_eq!(local.snapshot().consent, Consent::Granted);
    assert_eq!(persistent.snapshot().consent, Consent::Granted);
    mark("B-6cde50738d");
    let f = Fixture::new();
    let local = settings(Consent::Unset);
    let persistent = settings(Consent::Unset);
    let handler = Handler::new(Ok(Some(false)), false);
    f.owner
        .register_consent(Some(handler.clone()), Some(persistent.clone()));
    for _ in 0..2 {
        assert!(!f.owner.resolve_consent(Some(local.clone())).await)
    }
    assert_eq!(handler.calls.load(Ordering::SeqCst), 1);
    assert_eq!(local.snapshot().consent, Consent::Denied);
    assert_eq!(persistent.snapshot().consent, Consent::Denied);
    mark("B-b1f6334e7d");
    for (answer, id) in [
        (Err("dialog crashed".into()), "B-4fb2b452ce"),
        (Ok(None), "B-250e64fd91"),
    ] {
        let f = Fixture::new();
        let local = settings(Consent::Unset);
        let persistent = settings(Consent::Unset);
        let handler = Handler::new(answer, false);
        f.owner
            .register_consent(Some(handler.clone()), Some(persistent.clone()));
        for _ in 0..2 {
            assert!(!f.owner.resolve_consent(Some(local.clone())).await)
        }
        assert_eq!(handler.calls.load(Ordering::SeqCst), 2);
        assert_eq!(local.snapshot().consent, Consent::Unset);
        assert_eq!(persistent.snapshot().consent, Consent::Unset);
        mark(id);
    }
    let f = Fixture::new();
    let handler = Handler::new(Ok(Some(false)), false);
    let local = settings(Consent::Unset);
    f.owner
        .register_consent(Some(handler.clone()), Some(settings(Consent::Granted)));
    assert!(f.owner.resolve_consent(Some(local.clone())).await);
    assert_eq!(handler.calls.load(Ordering::SeqCst), 0);
    assert_eq!(local.snapshot().consent, Consent::Unset);
    mark("B-3623014fa8");
    // Supplemental: re-registration retains cache and directs pending decision
    // to the current persistent owner, never the retired registration.
    f.owner
        .register_consent(None, Some(settings(Consent::Denied)));
    assert!(f.owner.resolve_consent(None).await);
    let f = Fixture::new();
    let a = settings(Consent::Unset);
    let b = settings(Consent::Unset);
    let local = settings(Consent::Unset);
    let handler = Handler::new(Ok(Some(true)), true);
    f.owner
        .register_consent(Some(handler.clone()), Some(a.clone()));
    let owner = f.owner.clone();
    let caller = local.clone();
    let task = tokio::spawn(async move { owner.resolve_consent(Some(caller)).await });
    handler.entered.notified().await;
    f.owner.register_consent(None, Some(b.clone()));
    handler.hold.as_ref().unwrap().add_permits(1);
    assert!(task.await.unwrap());
    assert_eq!(a.snapshot().consent, Consent::Unset);
    assert_eq!(b.snapshot().consent, Consent::Granted);
    println!("report-issue-source:pass:consent-rereg-current-persistent");
}

#[tokio::test]
async fn fixed_report_issue_main21_native_grouped() {
    let default = settings(Consent::Unset);
    assert!(
        !AutoQaEnvironment {
            enabled: Some(false),
            ..Default::default()
        }
        .enabled(Some(&default))
    );
    mark("B-61eb807edf");
    let disabled: Arc<dyn AutoQaSettings> = Arc::new(Settings(Mutex::new(AutoQaConfig {
        enabled: false,
        explicitly_configured: true,
        ..Default::default()
    })));
    assert!(
        AutoQaEnvironment {
            enabled: Some(true),
            ..Default::default()
        }
        .enabled(Some(&disabled))
    );
    mark("B-115d80317f");
    assert!(AutoQaEnvironment::default().enabled(Some(&default)));
    mark("B-1c8bf5fa15");
    let denied: Arc<dyn AutoQaSettings> = Arc::new(Settings(Mutex::new(AutoQaConfig {
        consent: Consent::Denied,
        ..Default::default()
    })));
    assert!(!AutoQaEnvironment::default().enabled(Some(&denied)));
    mark("B-343f6f42df");
    let explicit: Arc<dyn AutoQaSettings> = Arc::new(Settings(Mutex::new(AutoQaConfig {
        consent: Consent::Denied,
        explicitly_configured: true,
        ..Default::default()
    })));
    assert!(AutoQaEnvironment::default().enabled(Some(&explicit)));
    mark("B-ec3ce709e4");
    assert!(!AutoQaEnvironment::default().enabled(Some(&disabled)));
    mark("B-9f20ac174f");
    let f = Fixture::new();
    f.seed(1);
    let r = f
        .owner
        .flush(
            Some(default),
            AutoQaEnvironment::default(),
            FlushOptions::default(),
        )
        .await;
    assert_eq!(r.skipped, Some(true));
    assert_eq!(f.counts(), (1, 0));
    assert!(f.http.requests.lock().unwrap().is_empty());
    mark("B-41bf4d7e2b");
    let noendpoint: Arc<dyn AutoQaSettings> = Arc::new(Settings(Mutex::new(AutoQaConfig {
        consent: Consent::Granted,
        endpoint: Some("".into()),
        ..Default::default()
    })));
    let r = f
        .owner
        .flush(
            Some(noendpoint),
            AutoQaEnvironment::default(),
            FlushOptions::default(),
        )
        .await;
    assert_eq!(r.skipped, Some(true));
    assert!(f.http.requests.lock().unwrap().is_empty());
    mark("B-395f437676");
    let f = Fixture::new();
    let r = f
        .owner
        .flush(
            Some(settings(Consent::Granted)),
            AutoQaEnvironment::default(),
            FlushOptions::default(),
        )
        .await;
    assert!(r.ok);
    assert_eq!(r.pushed, 0);
    assert!(f.http.requests.lock().unwrap().is_empty());
    assert_eq!(f.installs.load(Ordering::SeqCst), 0);
    mark("B-b5aae28174");
    let f = Fixture::new();
    f.owner
        .insert("test-model", "glob", "weird ordering")
        .unwrap();
    f.owner
        .insert("test-model", "read", "selector ignored")
        .unwrap();
    let s: Arc<dyn AutoQaSettings> = Arc::new(Settings(Mutex::new(AutoQaConfig {
        consent: Consent::Granted,
        endpoint: Some("http://127.0.0.1/grievances".into()),
        token: Some("fixture-token".into()),
        ..Default::default()
    })));
    let r = f
        .owner
        .flush(
            Some(s),
            AutoQaEnvironment::default(),
            FlushOptions::default(),
        )
        .await;
    assert!(r.ok);
    assert_eq!(r.pushed, 2);
    assert_eq!(f.counts(), (2, 2));
    let requests = f.http.requests.lock().unwrap();
    let (url, token, body) = &requests[0];
    assert_eq!(url, "http://127.0.0.1/grievances");
    assert_eq!(token.as_deref(), Some("fixture-token"));
    assert_eq!(
        body["agent"],
        serde_json::json!({"name":"omp","version":"test-version"})
    );
    assert_eq!(body["installId"], "11111111-2222-3333-4444-555555555555");
    assert!(body.get("host").is_none());
    assert!(body.get("hostname").is_none());
    assert_eq!(body["platform"], "win32");
    assert_eq!(body["arch"], "x64");
    assert_eq!(
        body["entries"],
        serde_json::json!([{"id":1,"model":"test-model","version":"test-version","tool":"glob","report":"weird ordering"},{"id":2,"model":"test-model","version":"test-version","tool":"read","report":"selector ignored"}])
    );
    drop(requests);
    mark("B-ac176db69a");
    let f = Fixture::new();
    f.seed(1);
    f.http.status.lock().unwrap().push_back(Ok(204));
    let r = f
        .owner
        .flush(
            Some(settings(Consent::Granted)),
            AutoQaEnvironment::default(),
            FlushOptions::default(),
        )
        .await;
    assert!(r.ok);
    assert_eq!(r.pushed, 1);
    assert_eq!(f.counts(), (1, 1));
    assert!(f.http.requests.lock().unwrap()[0].1.is_none());
    mark("B-36729dc5f9");
    let f = Fixture::new();
    f.seed(1);
    f.http.status.lock().unwrap().push_back(Ok(500));
    let r = f
        .owner
        .flush(
            Some(settings(Consent::Granted)),
            AutoQaEnvironment::default(),
            FlushOptions::default(),
        )
        .await;
    assert!(!r.ok && !r.outcome_unknown);
    assert_eq!(r.pushed, 0);
    assert_eq!(f.counts(), (1, 0));
    mark("B-d0b6e0a7df");
    let r = f
        .owner
        .flush(
            Some(settings(Consent::Granted)),
            AutoQaEnvironment::default(),
            FlushOptions::default(),
        )
        .await;
    assert_eq!(r.skipped, Some(true));
    assert_eq!(f.http.requests.lock().unwrap().len(), 1);
    mark("B-3baf1539da");
    f.clock.0.fetch_add(30000, Ordering::SeqCst);
    assert!(
        f.owner
            .flush(
                Some(settings(Consent::Granted)),
                AutoQaEnvironment::default(),
                FlushOptions::default()
            )
            .await
            .ok
    );
    let f = Fixture::new();
    f.seed(1);
    let hold = Arc::new(Semaphore::new(0));
    *f.http.hold.lock().unwrap() = Some(hold.clone());
    let owner = f.owner.clone();
    let s = settings(Consent::Granted);
    let a = tokio::spawn(async move {
        owner
            .flush(
                Some(s),
                AutoQaEnvironment::default(),
                FlushOptions::default(),
            )
            .await
    });
    f.http.entered.notified().await;
    f.owner.insert("test-model", "read", "second").unwrap();
    let owner = f.owner.clone();
    let s = settings(Consent::Granted);
    let b = tokio::spawn(async move {
        owner
            .flush(
                Some(s),
                AutoQaEnvironment::default(),
                FlushOptions::default(),
            )
            .await
    });
    tokio::task::yield_now().await;
    hold.add_permits(1);
    let (a, b) = (a.await.unwrap(), b.await.unwrap());
    assert!(Arc::ptr_eq(&a, &b));
    assert!(a.ok);
    assert_eq!(a.pushed, 2);
    assert_eq!(f.http.requests.lock().unwrap().len(), 2);
    assert_eq!(f.counts(), (2, 2));
    mark("B-ac72a5255a");
    mark("B-89f983f0ab");
    let f = Fixture::new();
    f.seed(127);
    let starts = Arc::new(Mutex::new(Vec::new()));
    let progress = Arc::new(Mutex::new(Vec::new()));
    let a = starts.clone();
    let b = progress.clone();
    let r = f
        .owner
        .flush(
            Some(settings(Consent::Granted)),
            AutoQaEnvironment::default(),
            FlushOptions {
                on_start: Some(Arc::new(move |n| a.lock().unwrap().push(n))),
                on_progress: Some(Arc::new(move |n| b.lock().unwrap().push(n))),
                ..Default::default()
            },
        )
        .await;
    assert!(r.ok);
    assert_eq!(r.pushed, 127);
    assert_eq!(
        f.http
            .requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.2["entries"].as_array().unwrap().len())
            .collect::<Vec<_>>(),
        vec![50, 50, 27]
    );
    assert_eq!(*starts.lock().unwrap(), vec![127]);
    assert_eq!(*progress.lock().unwrap(), vec![50, 100, 127]);
    assert_eq!(f.counts(), (127, 127));
    mark("B-086b3f0afb");
    let f = Fixture::new();
    f.seed(60);
    f.http.status.lock().unwrap().extend([Ok(200), Ok(500)]);
    let r = f
        .owner
        .flush(
            Some(settings(Consent::Granted)),
            AutoQaEnvironment::default(),
            FlushOptions::default(),
        )
        .await;
    assert!(!r.ok);
    assert_eq!(r.pushed, 50);
    assert_eq!(f.counts(), (60, 50));
    mark("B-29dbb98ebd");
    for (body, expected, id) in [
        (
            "proxy_read: selector ignored",
            ("read", "selector ignored"),
            "B-67b8af5466",
        ),
        (
            "read\nselector ignored",
            ("read", "selector ignored"),
            "B-f7849ef1a6",
        ),
    ] {
        let f = Fixture::new();
        let s: Arc<dyn AutoQaSettings> = Arc::new(Settings(Mutex::new(AutoQaConfig {
            consent: Consent::Granted,
            endpoint: Some("".into()),
            ..Default::default()
        })));
        f.owner
            .dispatch(
                Some(s),
                AutoQaEnvironment::default(),
                "chosen-model".into(),
                body,
            )
            .unwrap();
        f.settle().await;
        let row = f
            .owner
            .with_db(|db| {
                db.query_row("SELECT tool,report,model FROM grievances", [], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })
            })
            .unwrap();
        assert_eq!((row.0.as_str(), row.1.as_str()), expected);
        assert_eq!(row.2, "chosen-model");
        assert!(f.http.requests.lock().unwrap().is_empty());
        mark(id);
    }
    assert!(
        parse_body("missing separator")
            .unwrap_err()
            .contains(&usage())
    );
    assert!(
        parse_body(" \u{feff} ")
            .unwrap_err()
            .starts_with("Empty report.")
    );
    mark("B-db45fcd4bf");
    // The unresolved-consent original is exercised through real lib Native
    // dispatch below, so Noted is observed independently from background state.
    exercise_native_plain_text_owner().await;
    mark("B-c571dbb2c0");
    // Supplemental original source branches: stale cache records but cannot
    // push; legacy schema backfills before index; DB unavailable stays best effort.
    let f = Fixture::new();
    assert!(
        f.owner
            .resolve_consent(Some(settings(Consent::Granted)))
            .await
    );
    f.owner
        .dispatch(
            Some(settings(Consent::Unset)),
            AutoQaEnvironment::default(),
            "stale-child".into(),
            "read: stale report",
        )
        .unwrap();
    f.settle().await;
    assert_eq!(f.counts(), (1, 0));
    assert!(f.http.requests.lock().unwrap().is_empty());
    let f = Fixture::new();
    {
        let db = Connection::open(&f.db).unwrap();
        db.execute_batch("CREATE TABLE grievances(id INTEGER PRIMARY KEY AUTOINCREMENT,model TEXT NOT NULL,version TEXT NOT NULL,tool TEXT NOT NULL,report TEXT NOT NULL,pushed INTEGER NOT NULL DEFAULT 0); INSERT INTO grievances(model,version,tool,report) VALUES('m','v','read','legacy');").unwrap();
    }
    f.seed(1);
    let created = f
        .owner
        .with_db(|db| {
            db.query_row("SELECT created_at FROM grievances WHERE id=1", [], |r| {
                r.get::<_, String>(0)
            })
        })
        .unwrap();
    assert!(!created.is_empty());
    assert_eq!(
        f.owner
            .with_db(|db| db.query_row("PRAGMA busy_timeout", [], |r| r.get::<_, u64>(0)))
            .unwrap(),
        5000
    );
    let f = Fixture::new();
    let hold = Arc::new(Semaphore::new(0));
    *f.http.hold.lock().unwrap() = Some(hold);
    f.owner
        .dispatch(
            Some(settings(Consent::Granted)),
            AutoQaEnvironment::default(),
            "timeout-model".into(),
            "read: awaiting HTTP",
        )
        .unwrap();
    f.http.entered.notified().await;
    let receipts = f.owner.close_and_settle(Duration::from_millis(20)).await;
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].outcome, JobOutcome::UnknownHttpOutcome);
    assert_eq!(f.counts(), (1, 0));
    assert_eq!(f.http.requests.lock().unwrap().len(), 1);
    // Controlled owner fault, not a Native/normal-binary Host receipt: both
    // pre-recorded rows belong to the same entered HTTP batch, and each
    // independently owned job must inherit the shared flight's uncertainty.
    let f = Fixture::new();
    f.seed(2);
    *f.http.hold.lock().unwrap() = Some(Arc::new(Semaphore::new(0)));
    for id in 1..=2 {
        let receipt = Arc::new(Mutex::new(JobReceipt {
            id,
            outcome: JobOutcome::RecordedPending,
            pushed: 0,
        }));
        let owner = f.owner.clone();
        let waiter = receipt.clone();
        let task = tokio::spawn(async move {
            owner
                .flush_with_receipt(
                    Some(settings(Consent::Granted)),
                    AutoQaEnvironment::default(),
                    FlushOptions::default(),
                    Some(waiter),
                )
                .await;
        });
        f.owner
            .jobs
            .lock()
            .unwrap()
            .push(RecordJob { receipt, task });
        if id == 1 {
            tokio::time::timeout(Duration::from_secs(1), f.http.entered.notified())
                .await
                .expect("first two-row batch entered HTTP");
        }
    }
    // Wait for observable registration, rather than assuming one yield made
    // the second task join the flight before shutdown.
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let registered = f
                .owner
                .flush
                .lock()
                .unwrap()
                .flight
                .as_ref()
                .is_some_and(|flight| flight.receipts.lock().unwrap().waiters.len() == 2);
            if registered {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("second job registered on the entered shared flush");
    {
        let requests = f.http.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].2["entries"].as_array().unwrap().len(), 2);
    }
    let receipts = f.owner.close_and_settle(Duration::from_millis(20)).await;
    assert_eq!(receipts.len(), 2);
    assert!(
        receipts
            .iter()
            .all(|r| r.outcome == JobOutcome::UnknownHttpOutcome)
    );
    assert_eq!(f.counts(), (2, 0));
    assert_eq!(f.http.requests.lock().unwrap().len(), 1);
    assert_eq!(f.installs.load(Ordering::SeqCst), 1);
    assert!(f.owner.flush.lock().unwrap().flight.is_none());
    println!(
        "report-issue-source:pass:shared-two-job-sent-batch-shutdown-unknown-retained-no-replay"
    );
    let f = Fixture::new();
    std::fs::create_dir(&f.db).unwrap();
    f.owner
        .dispatch(
            Some(settings(Consent::Granted)),
            AutoQaEnvironment::default(),
            "unavailable-db".into(),
            "read: DB failure",
        )
        .unwrap();
    f.settle().await;
    assert!(f.http.requests.lock().unwrap().is_empty());
    println!("report-issue-source:pass:stale-cache-push-gate-and-legacy-migration");
    println!(
        "report-issue-source:pass:bounded-shutdown-unknown-http-retained-row-no-replay-db-unavailable"
    );
}

async fn exercise_native_plain_text_owner() {
    use crate::{
        actual_tools::{BuiltinKind, BuiltinToolSource, NoUiDeviceApproval},
        native_factory::{
            NativeCatalogFactoryImports, PreparedNativeSession,
            catalog::{
                NativeCatalogMember, NativeCatalogOwner, NativeCatalogSnapshot,
                NativeCatalogSources, NativeDeviceOptions, NativeToolCatalog,
            },
        },
        native_run::NativeRun,
    };
    use ara_agent::{ActualExecuteRequest, ActualToolSource, AgentMessage};
    for case in [
        "held",
        "no-handler",
        "disabled-malformed",
        "enabled-malformed",
        "denied",
        "dismissed",
        "root-denied",
        "revoked",
    ] {
        let f = Fixture::new();
        let handler = Handler::new(
            Ok(match case {
                "denied" => Some(false),
                "dismissed" => None,
                _ => Some(true),
            }),
            case == "held",
        );
        if case != "no-handler" {
            f.owner.register_consent(Some(handler.clone()), None);
        }
        let caller = settings(Consent::Unset);
        let host = ReportIssueHost {
            owner: f.owner.clone(),
            settings: Some(caller),
            environment: AutoQaEnvironment {
                enabled: (case == "disabled-malformed").then_some(false),
                ..Default::default()
            },
            model: "native-model".into(),
        };
        let realm = crate::native_realm::NativeRealm::prepare().unwrap();
        let values = &realm.values;
        let heap = values.heap();
        let source = BuiltinToolSource::new(
            values,
            ara_tools::ToolContext::new(f._dir.path().to_path_buf()),
            &[BuiltinKind::Read, BuiltinKind::Write],
        )
        .unwrap();
        source.bind_ast_module(realm.ast.clone(), values).unwrap();
        let originals = source.current_tools(values).unwrap().unwrap();
        let registry = (0..heap.array_length(&originals, values.runtime()).unwrap())
            .map(|i| {
                let original = heap
                    .get(&originals, &i.to_string().as_str().into(), values.runtime())
                    .unwrap();
                NativeCatalogMember::bind(
                    NativeCatalogOwner::Builtin(source.clone()),
                    &original,
                    values,
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        let write=registry.iter().find(|m|matches!(heap.snapshot(&heap.get(&m.binding.source,&"name".into(),values.runtime()).unwrap()).unwrap(),ara_rpc::value_heap::ValueSnapshot::String(n) if n.equals_ascii("write"))).unwrap().binding.source.clone();
        let policies = if case == "root-denied" {
            serde_json::json!({"write":"deny"})
        } else {
            serde_json::json!({})
        };
        let snapshot = NativeCatalogSnapshot::new_with_devices(
            values,
            originals,
            registry,
            NativeCatalogSources {
                builtin: source.clone(),
                mcp: None,
                rpc: None,
            },
            NativeDeviceOptions {
                enabled: true,
                report_issue: Some(host),
                approval_host: Some(Arc::new(NoUiDeviceApproval {
                    mode: "yolo".into(),
                    policies: ara_rpc::WireValue::parse(&policies.to_string()).unwrap(),
                })),
                ..Default::default()
            },
        )
        .unwrap();
        if case == "revoked" {
            source.set_granted(BuiltinKind::Write, false).unwrap();
        }
        let catalog = NativeToolCatalog::new(values, snapshot).unwrap();
        let draft = PreparedNativeSession::from_realm(realm.clone(), None).unwrap();
        let prepared = draft
            .finish_catalog_bindings(NativeCatalogFactoryImports {
                catalog,
                string_function: realm.globals.string.clone(),
                bun: realm.globals.bun.clone(),
                original_logger: None,
            })
            .unwrap();
        let cancel = CancellationToken::new();
        let driver = NativeRun::begin(prepared.clone(), cancel.clone()).unwrap();
        let body = if case.ends_with("malformed") {
            "not a report"
        } else {
            "read: native issue"
        };
        let args = heap
            .import_serde(&serde_json::json!({"path":"xd://report_issue","content":body}))
            .unwrap();
        let binding = source.bind_selected(&write, values).unwrap();
        let call = heap
            .import_serde(
                &serde_json::json!({"type":"toolCall","id":"report-native","name":"write"}),
            )
            .unwrap();
        heap.define_own_data(&call, "arguments".into(), args.clone(), values.runtime())
            .unwrap();
        let mut assistant = serde_json::to_value(ara_ai::AssistantMessage::empty(
            "fixture", "fixture", "fixture",
        ))
        .unwrap();
        assistant["role"] = "assistant".into();
        assistant["stopReason"] = "toolUse".into();
        let root = heap.import_serde(&assistant).unwrap();
        heap.define_own_data(
            &root,
            "content".into(),
            heap.array(vec![call.clone()]).unwrap(),
            values.runtime(),
        )
        .unwrap();
        let owner = AgentMessage::from_graph(root, None).unwrap();
        let context = heap.object().unwrap();
        prepared
            .require_tool_hooks()
            .unwrap()
            .bind_run_context(values, &context)
            .unwrap();
        let request = ActualExecuteRequest {
            callable: heap
                .get(&write, &"execute".into(), values.runtime())
                .unwrap(),
            receiver: write,
            arguments_slot: owner.current_arguments_slot(&call).unwrap(),
            owner,
            tool_call: call,
            call_id: "report-native".into(),
            args,
            values: values.clone(),
            cancel,
            on_update: Arc::new(|_| Ok(())),
            tool_context: context,
        };
        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            driver.drive(
                binding
                    .capability
                    .actual_invoker()
                    .unwrap()
                    .invoke_settled(request),
            ),
        )
        .await
        .unwrap();
        if matches!(case, "root-denied" | "revoked") {
            assert!(outcome.is_err() || outcome.as_ref().is_ok_and(|result| result.is_err()));
            assert_eq!(handler.calls.load(Ordering::SeqCst), 0);
            assert!(!f.db.exists());
            continue;
        }
        let result = outcome.unwrap().unwrap();
        let wire = heap
            .stringify(
                &result,
                ara_rpc::value_heap::StringifyMode::Plain,
                None,
                values.runtime(),
            )
            .unwrap()
            .unwrap()
            .to_utf8()
            .unwrap();
        let result: serde_json::Value = serde_json::from_str(&wire).unwrap();
        if case == "enabled-malformed" {
            assert_eq!(result["isError"], true);
            assert!(
                result["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains(&usage())
            );
            assert!(!f.db.exists());
            continue;
        }
        assert_eq!(result["content"][0]["text"], "Noted, thanks!");
        assert_eq!(result["details"]["xdev"]["tool"], "report_issue");
        assert_eq!(result["details"]["xdev"]["mode"], "execute");
        assert_eq!(result["details"]["xdev"]["args"]["report"], body);
        if case == "held" {
            handler.entered.notified().await;
            assert!(!f.db.exists());
            assert!(f.http.requests.lock().unwrap().is_empty());
            let receipts = f.owner.close_and_settle(Duration::from_millis(100)).await;
            assert_eq!(receipts[0].outcome, JobOutcome::CancelledBeforeRecord);
        } else {
            f.settle().await;
        }
        assert!(!f.db.exists());
        assert_eq!(f.installs.load(Ordering::SeqCst), 0);
        assert!(f.http.requests.lock().unwrap().is_empty());
    }
    println!("report-issue-native:pass:held-headless-disabled-malformed-deny-dismiss-grants");
}
