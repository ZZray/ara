//! Fixed OMP596f2da report-tool-issue.ts: plain-text WRITE dependency.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
//! Host-selected process service. No Graph, product identity or consent UI is
//! stored in Core. Immediate ingress and background delivery are separate.
use futures::{
    FutureExt,
    future::{BoxFuture, Shared},
};
use rusqlite::Connection;
use serde::Serialize;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio_util::sync::CancellationToken;

pub const DEVICE_NAME: &str = "report_issue";
pub const DEVICE_PATH: &str = "xd://report_issue";
pub const DEFAULT_ENDPOINT: &str = "https://qa.omp.sh/v1/grievances";
pub fn usage() -> String {
    format!(
        "Write `<tool>: <concise description>` as plain text to {DEVICE_PATH}. A two-line fallback also works: tool name on line 1, report body below."
    )
}
fn trim(s: &str) -> &str {
    s.trim_matches(ara_prompt::js::is_space)
}
pub fn parse_body(text: &str) -> Result<(String, String), String> {
    let body = trim(text);
    if body.is_empty() {
        return Err(format!("Empty report. {}", usage()));
    }
    if let Some((tool, report)) = body.split_once('\n') {
        if !trim(tool).is_empty() && !trim(report).is_empty() {
            return Ok((trim(tool).into(), trim(report).into()));
        }
    }
    if let Some((tool, report)) = body.split_once(':') {
        if !trim(tool).is_empty() && !trim(report).is_empty() {
            return Ok((trim(tool).into(), trim(report).into()));
        }
    }
    Err(format!("Invalid report format. {}", usage()))
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Consent {
    #[default]
    Unset,
    Granted,
    Denied,
}
#[derive(Clone, Debug)]
pub struct AutoQaConfig {
    pub enabled: bool,
    pub explicitly_configured: bool,
    pub consent: Consent,
    pub endpoint: Option<String>,
    pub token: Option<String>,
}
impl Default for AutoQaConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            explicitly_configured: false,
            consent: Consent::Unset,
            endpoint: Some(DEFAULT_ENDPOINT.into()),
            token: None,
        }
    }
}
/// Caller snapshots and a registered persistent owner are intentionally distinct.
pub trait AutoQaSettings: Send + Sync {
    fn snapshot(&self) -> AutoQaConfig;
    fn set_consent(&self, consent: Consent) -> Result<(), String>;
}
#[derive(Clone, Debug, Default)]
pub struct AutoQaEnvironment {
    pub enabled: Option<bool>,
    pub push: bool,
    pub endpoint: Option<String>,
    pub token: Option<String>,
}
impl AutoQaEnvironment {
    pub fn enabled(&self, settings: Option<&Arc<dyn AutoQaSettings>>) -> bool {
        self.enabled.unwrap_or_else(|| {
            settings.is_some_and(|s| {
                let c = s.snapshot();
                c.enabled && (c.explicitly_configured || c.consent != Consent::Denied)
            })
        })
    }
    fn push_config(
        &self,
        settings: Option<&Arc<dyn AutoQaSettings>>,
        bypass: bool,
    ) -> Option<(String, Option<String>)> {
        if !self.enabled(settings) {
            return None;
        }
        let c = settings.map(|s| s.snapshot());
        // Fixed source uses caller persisted consent here, never process cache.
        if !bypass && !self.push && !c.as_ref().is_some_and(|c| c.consent == Consent::Granted) {
            return None;
        }
        let endpoint = self
            .endpoint
            .clone()
            .or_else(|| c.as_ref().and_then(|c| c.endpoint.clone()))?;
        if trim(&endpoint).is_empty() {
            return None;
        }
        let token = self
            .token
            .clone()
            .or_else(|| c.and_then(|c| c.token))
            .filter(|s| !s.is_empty());
        Some((trim(&endpoint).into(), token))
    }
}
#[async_trait::async_trait]
pub trait AutoQaConsentHandler: Send + Sync {
    async fn decide(&self) -> Result<Option<bool>, String>;
}
pub trait AutoQaClock: Send + Sync {
    fn now_ms(&self) -> u64;
}
pub struct SystemAutoQaClock;
impl AutoQaClock for SystemAutoQaClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
}
#[derive(Clone, Debug)]
pub struct HttpFailure {
    pub message: String,
    pub outcome_unknown: bool,
}
#[async_trait::async_trait]
pub trait AutoQaHttp: Send + Sync {
    async fn post(
        &self,
        endpoint: &str,
        token: Option<&str>,
        body: serde_json::Value,
    ) -> Result<u16, HttpFailure>;
}
pub struct ReqwestAutoQaHttp;
#[async_trait::async_trait]
impl AutoQaHttp for ReqwestAutoQaHttp {
    async fn post(
        &self,
        endpoint: &str,
        token: Option<&str>,
        body: serde_json::Value,
    ) -> Result<u16, HttpFailure> {
        let mut request = reqwest::Client::new()
            .post(endpoint)
            .timeout(Duration::from_millis(5000))
            .json(&body);
        if let Some(token) = token {
            request = request.bearer_auth(token)
        }
        request
            .send()
            .await
            .map(|r| r.status().as_u16())
            .map_err(|e| HttpFailure {
                message: e.to_string(),
                outcome_unknown: true,
            })
    }
}
pub struct AutoQaHostPorts {
    pub database_path: Option<PathBuf>,
    pub version: String,
    pub platform: String,
    pub arch: String,
    /// Lazy: no install identity is created before an actual HTTP batch.
    pub install_id: Arc<dyn Fn() -> String + Send + Sync>,
    pub clock: Arc<dyn AutoQaClock>,
    pub http: Arc<dyn AutoQaHttp>,
}
#[derive(Clone, Debug, Serialize)]
pub struct FlushResult {
    pub pushed: usize,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<bool>,
    pub outcome_unknown: bool,
}
impl FlushResult {
    fn skip() -> Arc<Self> {
        Arc::new(Self {
            pushed: 0,
            ok: false,
            skipped: Some(true),
            outcome_unknown: false,
        })
    }
    fn done(pushed: usize, ok: bool, unknown: bool) -> Arc<Self> {
        Arc::new(Self {
            pushed,
            ok,
            skipped: None,
            outcome_unknown: unknown,
        })
    }
}
#[derive(Clone, Default)]
pub struct FlushOptions {
    pub bypass_consent: bool,
    pub on_start: Option<Arc<dyn Fn(usize) + Send + Sync>>,
    pub on_progress: Option<Arc<dyn Fn(usize) + Send + Sync>>,
}
type ConsentFlight = Shared<BoxFuture<'static, bool>>;
#[derive(Clone)]
struct FlushFlight {
    future: Shared<BoxFuture<'static, Arc<FlushResult>>>,
    receipts: Arc<Mutex<FlushReceipts>>,
}
#[derive(Default)]
struct FlushReceipts {
    http_entered: bool,
    waiters: Vec<Arc<Mutex<JobReceipt>>>,
}
impl FlushReceipts {
    fn register(&mut self, receipt: Option<Arc<Mutex<JobReceipt>>>) {
        if let Some(receipt) = receipt {
            if self.http_entered {
                receipt.lock().unwrap_or_else(|e| e.into_inner()).outcome = JobOutcome::HttpEntered;
            }
            self.waiters.push(receipt);
        }
    }
    fn enter_http(&mut self) {
        // Every waiter owns the aggregate flush outcome. After joining an
        // entered flight it cannot claim that its external outcome is known.
        self.http_entered = true;
        for receipt in &self.waiters {
            let mut receipt = receipt.lock().unwrap_or_else(|e| e.into_inner());
            if receipt.outcome == JobOutcome::RecordedPending {
                receipt.outcome = JobOutcome::HttpEntered;
            }
        }
    }
}
#[derive(Default)]
struct ConsentState {
    cache: Option<bool>,
    handler: Option<Arc<dyn AutoQaConsentHandler>>,
    persistent: Option<Arc<dyn AutoQaSettings>>,
    flight: Option<ConsentFlight>,
}
#[derive(Default)]
struct FlushState {
    flight: Option<FlushFlight>,
    last_failure_at: u64,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub enum JobOutcome {
    Queued,
    AwaitingConsent,
    CancelledBeforeRecord,
    RecordedPending,
    HttpEntered,
    /// The shared flush completed; this does not establish own-row delivery.
    FlushCompleted,
    FailedRecord,
    UnknownHttpOutcome,
}
#[derive(Clone, Debug, Serialize)]
pub struct JobReceipt {
    pub id: usize,
    pub outcome: JobOutcome,
    /// Shared flush cumulative count, not this individual record's count.
    pub pushed: usize,
}
struct RecordJob {
    receipt: Arc<Mutex<JobReceipt>>,
    task: tokio::task::JoinHandle<()>,
}
pub struct AutoQaProcessOwner {
    ports: AutoQaHostPorts,
    consent: Mutex<ConsentState>,
    flush: Mutex<FlushState>,
    database: Mutex<Option<Connection>>,
    closing: AtomicBool,
    cancel_consent: CancellationToken,
    jobs: Mutex<Vec<RecordJob>>,
}
impl AutoQaProcessOwner {
    pub fn new(ports: AutoQaHostPorts) -> Arc<Self> {
        Arc::new(Self {
            ports,
            consent: Mutex::default(),
            flush: Mutex::default(),
            database: Mutex::default(),
            closing: AtomicBool::new(false),
            cancel_consent: CancellationToken::new(),
            jobs: Mutex::default(),
        })
    }
    /// Re-registration replaces the handler and persistence target, retaining
    /// process cache/in-flight choice. Settlement reads the current target.
    pub fn register_consent(
        &self,
        handler: Option<Arc<dyn AutoQaConsentHandler>>,
        persistent: Option<Arc<dyn AutoQaSettings>>,
    ) {
        let mut s = self.consent.lock().unwrap_or_else(|e| e.into_inner());
        s.handler = handler;
        s.persistent = persistent;
    }
    pub async fn resolve_consent(self: &Arc<Self>, local: Option<Arc<dyn AutoQaSettings>>) -> bool {
        let flight = {
            let mut s = self.consent.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(value) = s.cache {
                return value;
            }
            let persisted = |v: &Arc<dyn AutoQaSettings>| match v.snapshot().consent {
                Consent::Granted => Some(true),
                Consent::Denied => Some(false),
                Consent::Unset => None,
            };
            if let Some(v) = local.as_ref().and_then(persisted).or_else(|| {
                s.persistent
                    .as_ref()
                    .filter(|p| local.as_ref().is_none_or(|l| !Arc::ptr_eq(p, l)))
                    .and_then(persisted)
            }) {
                s.cache = Some(v);
                return v;
            }
            let Some(handler) = s.handler.clone() else {
                return false;
            };
            if let Some(flight) = &s.flight {
                flight.clone()
            } else {
                let owner = self.clone();
                let flight=async move {
                    let decision=tokio::select!{_ = owner.cancel_consent.cancelled()=>None, result=handler.decide()=>result.ok().flatten()};
                    if let Some(granted)=decision{
                        let persistent={let mut s=owner.consent.lock().unwrap_or_else(|e|e.into_inner());s.cache=Some(granted);s.persistent.clone()};
                        let value=if granted{Consent::Granted}else{Consent::Denied};
                        if let Some(l)=&local{let _=l.set_consent(value);}
                        if let Some(p)=persistent.filter(|p|local.as_ref().is_none_or(|l|!Arc::ptr_eq(p,l))){let _=p.set_consent(value);}
                    }
                    owner.consent.lock().unwrap_or_else(|e|e.into_inner()).flight=None;
                    decision.unwrap_or(false)
                }.boxed().shared();
                s.flight = Some(flight.clone());
                flight
            }
        };
        flight.await
    }
    fn with_db<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T, String> {
        let mut slot = self.database.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            let path = self
                .ports
                .database_path
                .as_ref()
                .ok_or("Auto-QA DB path unavailable")?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let db = Connection::open(path).map_err(|e| e.to_string())?;
            db.busy_timeout(Duration::from_millis(5000))
                .map_err(|e| e.to_string())?;
            db.execute_batch("CREATE TABLE IF NOT EXISTS grievances (id INTEGER PRIMARY KEY AUTOINCREMENT, model TEXT NOT NULL, version TEXT NOT NULL, tool TEXT NOT NULL, report TEXT NOT NULL, created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP, pushed INTEGER NOT NULL DEFAULT 0);").map_err(|e|e.to_string())?;
            let has:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('grievances') WHERE name='created_at')",[],|r|r.get(0)).map_err(|e|e.to_string())?;
            if !has {
                db.execute_batch("ALTER TABLE grievances ADD COLUMN created_at TEXT NOT NULL DEFAULT ''; UPDATE grievances SET created_at=CURRENT_TIMESTAMP WHERE created_at='';").map_err(|e|e.to_string())?;
            }
            db.execute_batch("CREATE INDEX IF NOT EXISTS grievances_pushed_created_at_idx ON grievances(pushed,created_at,id);").map_err(|e|e.to_string())?;
            *slot = Some(db);
        }
        f(slot.as_ref().expect("initialized")).map_err(|e| e.to_string())
    }
    pub fn insert(&self, model: &str, tool: &str, report: &str) -> Result<(), String> {
        self.with_db(|db|db.execute("INSERT INTO grievances(model,version,tool,report,created_at) VALUES(?,?,?,?,CURRENT_TIMESTAMP)",rusqlite::params![model,self.ports.version,tool,report]).map(|_|()))
    }
    pub async fn flush(
        self: &Arc<Self>,
        settings: Option<Arc<dyn AutoQaSettings>>,
        env: AutoQaEnvironment,
        options: FlushOptions,
    ) -> Arc<FlushResult> {
        self.flush_with_receipt(settings, env, options, None).await
    }
    async fn flush_with_receipt(
        self: &Arc<Self>,
        settings: Option<Arc<dyn AutoQaSettings>>,
        env: AutoQaEnvironment,
        options: FlushOptions,
        receipt: Option<Arc<Mutex<JobReceipt>>>,
    ) -> Arc<FlushResult> {
        let Some(config) = env.push_config(settings.as_ref(), options.bypass_consent) else {
            return FlushResult::skip();
        };
        let flight = {
            let mut s = self.flush.lock().unwrap_or_else(|e| e.into_inner());
            let existing = if options.bypass_consent {
                None
            } else {
                s.flight.clone()
            };
            if let Some(flight) = existing {
                flight
                    .receipts
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .register(receipt);
                flight
            } else {
                if !options.bypass_consent
                    && s.last_failure_at > 0
                    && self.ports.clock.now_ms().saturating_sub(s.last_failure_at) < 30000
                {
                    return FlushResult::skip();
                }
                if self.with_db(|_| Ok(())).is_err() {
                    return FlushResult::skip();
                }
                let owner = self.clone();
                let bypass = options.bypass_consent;
                let receipts = Arc::new(Mutex::new(FlushReceipts::default()));
                receipts
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .register(receipt);
                let flush_receipts = receipts.clone();
                let future = async move {
                    let result = owner.perform_flush(config, options, flush_receipts).await;
                    if !bypass {
                        owner.flush.lock().unwrap_or_else(|e| e.into_inner()).flight = None;
                    }
                    result
                }
                .boxed()
                .shared();
                let flight = FlushFlight { future, receipts };
                if !bypass {
                    s.flight = Some(flight.clone())
                };
                flight
            }
        };
        flight.future.await
    }
    async fn perform_flush(
        &self,
        config: (String, Option<String>),
        options: FlushOptions,
        receipts: Arc<Mutex<FlushReceipts>>,
    ) -> Arc<FlushResult> {
        #[derive(Serialize)]
        struct Row {
            id: i64,
            model: String,
            version: String,
            tool: String,
            report: String,
        }
        if let Some(start) = options.on_start {
            match self.with_db(|db| {
                db.query_row("SELECT COUNT(*) FROM grievances WHERE pushed=0", [], |r| {
                    r.get::<_, usize>(0)
                })
            }) {
                Ok(n) => start(n),
                Err(_) => return FlushResult::skip(),
            }
        }
        let mut pushed = 0;
        loop {
            let rows=self.with_db(|db|{let mut statement=db.prepare("SELECT id,model,version,tool,report FROM grievances WHERE pushed=0 ORDER BY id ASC LIMIT 50")?;
                let rows=statement.query_map([],|r|Ok(Row{id:r.get(0)?,model:r.get(1)?,version:r.get(2)?,tool:r.get(3)?,report:r.get(4)?}))?.collect::<rusqlite::Result<Vec<_>>>();rows});
            let rows = match rows {
                Ok(rows) => rows,
                Err(_) => {
                    self.failure();
                    return FlushResult::done(0, false, false);
                }
            };
            if rows.is_empty() {
                return FlushResult::done(pushed, true, false);
            }
            let body = serde_json::json!({"agent":{"name":"omp","version":self.ports.version},"installId":(self.ports.install_id)(),"platform":self.ports.platform,"arch":self.ports.arch,"entries":&rows});
            receipts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .enter_http();
            let status = self
                .ports
                .http
                .post(&config.0, config.1.as_deref(), body)
                .await;
            match status {
                Ok(s) if (200..300).contains(&s) => {}
                other => {
                    self.failure();
                    return FlushResult::done(
                        pushed,
                        false,
                        matches!(
                            other,
                            Err(HttpFailure {
                                outcome_unknown: true,
                                ..
                            })
                        ),
                    );
                }
            }
            if self
                .with_db(|db| {
                    let transaction = db.unchecked_transaction()?;
                    for row in &rows {
                        transaction
                            .execute("UPDATE grievances SET pushed=1 WHERE id=?", [row.id])?;
                    }
                    transaction.commit()
                })
                .is_err()
            {
                self.failure();
                return FlushResult::done(0, false, true);
            }
            pushed += rows.len();
            if let Some(progress) = &options.on_progress {
                progress(pushed);
            }
        }
    }
    fn failure(&self) {
        self.flush
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .last_failure_at = self.ports.clock.now_ms();
    }
    pub fn dispatch(
        self: &Arc<Self>,
        settings: Option<Arc<dyn AutoQaSettings>>,
        env: AutoQaEnvironment,
        model: String,
        text: &str,
    ) -> Result<(), String> {
        if !env.enabled(settings.as_ref()) {
            return Ok(());
        }
        let (tool, report) = parse_body(text)?;
        let tool = tool.strip_prefix("proxy_").unwrap_or(&tool).to_owned();
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            eprintln!("ara: AutoQA background record could not start without a Host runtime");
            return Ok(());
        };
        let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        if self.closing.load(Ordering::Acquire) {
            return Ok(());
        }
        let receipt = Arc::new(Mutex::new(JobReceipt {
            id: jobs.len() + 1,
            outcome: JobOutcome::Queued,
            pushed: 0,
        }));
        let r = receipt.clone();
        let owner = self.clone();
        let task = runtime.spawn(async move {
            r.lock().unwrap_or_else(|e| e.into_inner()).outcome = JobOutcome::AwaitingConsent;
            if !env.push && !owner.resolve_consent(settings.clone()).await {
                r.lock().unwrap_or_else(|e| e.into_inner()).outcome =
                    JobOutcome::CancelledBeforeRecord;
                return;
            }
            if owner.insert(&model, &tool, &report).is_err() {
                r.lock().unwrap_or_else(|e| e.into_inner()).outcome = JobOutcome::FailedRecord;
                return;
            }
            r.lock().unwrap_or_else(|e| e.into_inner()).outcome = JobOutcome::RecordedPending;
            let result = owner
                .flush_with_receipt(settings, env, FlushOptions::default(), Some(r.clone()))
                .await;
            let mut receipt = r.lock().unwrap_or_else(|e| e.into_inner());
            receipt.pushed = result.pushed;
            receipt.outcome = if result.outcome_unknown {
                JobOutcome::UnknownHttpOutcome
            } else if result.ok {
                // The aggregate may have finished its final empty DB read
                // before this job inserted its row. Host acceptance separately
                // checks each persisted pushed flag, not this shared result.
                JobOutcome::FlushCompleted
            } else {
                JobOutcome::RecordedPending
            };
        });
        jobs.push(RecordJob { receipt, task });
        Ok(())
    }
    /// Close ingress, cancel unresolved consent, then settle accepted work.
    /// A deadline never replays an HTTP request; pending rows remain on disk.
    pub async fn close_and_settle(&self, bound: Duration) -> Vec<JobReceipt> {
        let mut jobs = {
            let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
            self.closing.store(true, Ordering::Release);
            std::mem::take(&mut *jobs)
        };
        self.cancel_consent.cancel();
        let deadline = tokio::time::Instant::now() + bound;
        for job in &mut jobs {
            let settled = tokio::time::timeout_at(deadline, &mut job.task).await;
            if !matches!(settled, Ok(Ok(()))) {
                if settled.is_err() {
                    job.task.abort();
                    let _ = (&mut job.task).await;
                }
                let mut r = job.receipt.lock().unwrap_or_else(|e| e.into_inner());
                r.outcome = match r.outcome {
                    JobOutcome::HttpEntered => JobOutcome::UnknownHttpOutcome,
                    JobOutcome::Queued | JobOutcome::AwaitingConsent => {
                        JobOutcome::CancelledBeforeRecord
                    }
                    other => other,
                };
            }
        }
        self.consent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .flight = None;
        self.flush.lock().unwrap_or_else(|e| e.into_inner()).flight = None;
        jobs.iter()
            .map(|j| j.receipt.lock().unwrap_or_else(|e| e.into_inner()).clone())
            .collect()
    }
}
#[derive(Clone)]
pub struct ReportIssueHost {
    pub owner: Arc<AutoQaProcessOwner>,
    pub settings: Option<Arc<dyn AutoQaSettings>>,
    pub environment: AutoQaEnvironment,
    pub model: String,
}
impl ReportIssueHost {
    pub fn dispatch(&self, text: &str) -> Result<(), String> {
        self.owner.dispatch(
            self.settings.clone(),
            self.environment.clone(),
            self.model.clone(),
            text,
        )
    }
}

#[cfg(test)]
mod tests;
