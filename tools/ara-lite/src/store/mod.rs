//! The state machine and durable store (Go: `core.go`, `store_sqlite.go`).
//!
//! One `Store` owns one data directory. Every mutation is computed on a draft,
//! committed to SQLite in one transaction, and only then published in memory, so
//! a failed write never leaves memory ahead of disk. The directory lock is held
//! until the store is closed or dropped.

mod db;
mod messages;
#[cfg(test)]
mod tests;

use crate::err;
use crate::error::{Error, Result};
use crate::lock::DirLock;
use crate::types::*;
use crate::util::{random_hex, token_equal};
use chrono::Duration as ChronoDuration;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

pub use db::STATE_DB_NAME;

/// Wall-clock source; replaceable so tests can move time deterministically.
pub type Clock = Arc<dyn Fn() -> Time + Send + Sync>;

/// Wire values written into `progress_note` (kept identical to the Go service).
const NOTE_CLAIMED: &str = "\u{5df2}\u{9886}\u{53d6}";
const NOTE_REWORK: &str = "\u{9700}\u{8fd4}\u{5de5}";

pub struct Store {
    inner: Mutex<Inner>,
    cond: Condvar,
    dir: PathBuf,
}

struct Inner {
    db: Option<Connection>,
    _lock: Option<DirLock>,
    clients: BTreeMap<String, Client>,
    tasks: BTreeMap<String, Task>,
    next_seq: i64,
    history_len: i64,
    master_join: String,
    worker_join: String,
    contact: HashMap<String, Time>,
    poll_contact: HashMap<String, Time>,
    now: Clock,
    /// Bumped on every durable change; long polls wait on it (Go: `changes` channel).
    epoch: u64,
    closed: bool,
}

/// Uncommitted next state for one operation.
struct Draft {
    clients: BTreeMap<String, Client>,
    tasks: BTreeMap<String, Task>,
    next_seq: i64,
    events: Vec<Event>,
    history: Vec<HistoryEntry>,
    receipt: Option<(String, Receipt)>,
}

impl Draft {
    fn from_inner(i: &Inner) -> Draft {
        Draft {
            clients: i.clients.clone(),
            tasks: i.tasks.clone(),
            next_seq: i.next_seq,
            events: Vec::new(),
            history: Vec::new(),
            receipt: None,
        }
    }

    fn add_event(&mut self, to: &str, task_id: &str, kind: &str, body: &str, at: Time) {
        self.next_seq += 1;
        self.events.push(Event {
            seq: self.next_seq,
            to: to.to_string(),
            task_id: task_id.to_string(),
            kind: kind.to_string(),
            body: body.to_string(),
            at,
        });
    }

    fn master_id(&self) -> String {
        master_id(&self.clients)
    }
}

fn master_id(clients: &BTreeMap<String, Client>) -> String {
    clients.values().find(|c| c.role == "master").map(|c| c.id.clone()).unwrap_or_default()
}

fn lock_inner(m: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    // State is only published after a commit, so a poisoned lock holds consistent data.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn canonical_workspace(raw: &str) -> Result<String> {
    if raw.trim().is_empty() {
        return Err(Error::new("workspace is required"));
    }
    let absolute = std::path::absolute(raw)?;
    let resolved = dunce::canonicalize(&absolute).map_err(|e| err!("workspace must exist: {e}"))?;
    let meta = std::fs::metadata(&resolved)?;
    if !meta.is_dir() {
        return Err(Error::new("workspace must be a directory"));
    }
    Ok(resolved.to_string_lossy().into_owned())
}

fn fold_case(s: &str) -> String {
    if cfg!(windows) { s.to_lowercase() } else { s.to_string() }
}

/// True when one workspace equals or contains the other (Go: `sameWorkspace`).
fn same_workspace(a: &str, b: &str) -> bool {
    let (a, b) = (fold_case(a), fold_case(b));
    let (a, b) = (Path::new(&a), Path::new(&b));
    a.starts_with(b) || b.starts_with(a)
}

fn exact_workspace(a: &str, b: &str) -> bool {
    fold_case(a) == fold_case(b)
}

/// Look up the task and check the caller's `version` and `generation`. An absent value fails.
fn get_task(d: &Draft, r: &Request) -> Result<Task> {
    fenced_task(d, r, false)
}

/// `optional` lets an absent value pass (claim only); a present value is always checked.
fn fenced_task(d: &Draft, r: &Request, optional: bool) -> Result<Task> {
    let t = d.tasks.get(&r.task_id).ok_or_else(|| Error::new("unknown task"))?;
    let stale = |sent: Option<i64>, current: i64| sent.map_or(!optional, |v| v != current);
    if stale(r.version, t.version) {
        return Err(err!("task version conflict: current version is {}", t.version));
    }
    if stale(r.generation, t.generation) {
        return Err(Error::new("stale claim generation"));
    }
    Ok(t.clone())
}

fn owned(actor: &Client, r: &Request, t: &Task) -> Result<()> {
    if actor.role != "worker" || t.worker_id != actor.id {
        return Err(Error::new("task belongs to another worker"));
    }
    if r.generation != Some(t.generation) {
        return Err(Error::new("stale claim generation"));
    }
    Ok(())
}

fn master_only(actor: &Client) -> Result<()> {
    if actor.role != "master" { Err(Error::new("master role required")) } else { Ok(()) }
}

fn rune_len(s: &str) -> usize {
    s.chars().count()
}

impl Store {
    /// Open (or create) the store in `dir` and take the single-instance lock.
    pub fn open(dir: &Path) -> Result<Store> {
        let absolute = std::path::absolute(dir)?;
        crate::profile::create_private_dir(&absolute)?;
        let canonical = dunce::canonicalize(&absolute)?;
        let lock = DirLock::acquire(&canonical.join("store.lock"))
            .map_err(|e| err!("data directory is already in use or cannot be locked: {e}"))?;
        let (db, loaded) = db::open_state_db(&canonical)?;
        let mut inner = Inner {
            db: Some(db),
            _lock: Some(lock),
            clients: loaded.clients,
            tasks: loaded.tasks,
            next_seq: loaded.next_seq,
            history_len: loaded.history_len,
            master_join: loaded.master_join,
            worker_join: loaded.worker_join,
            contact: HashMap::new(),
            poll_contact: HashMap::new(),
            now: Arc::new(chrono::Utc::now),
            epoch: 0,
            closed: false,
        };
        let at = inner.now_utc();
        inner.prune_messages(at)?;
        // A restart cannot prove any worker is still executing: running becomes unconfirmed.
        inner.expire(true)?;
        Ok(Store { inner: Mutex::new(inner), cond: Condvar::new(), dir: canonical })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Release the database and the directory lock; later calls fail with "store is closed".
    pub fn close(&self) {
        let mut g = lock_inner(&self.inner);
        g.closed = true;
        g.epoch += 1;
        g.db = None;
        g._lock = None;
        drop(g);
        self.cond.notify_all();
    }

    /// Current change generation. Capture it before polling, then wait on it only
    /// if the poll returned nothing (Go: `Changes()`).
    pub fn epoch(&self) -> u64 {
        lock_inner(&self.inner).epoch
    }

    /// Block until the generation differs from `seen`, the store closes, or `timeout`
    /// passes. Returns true when a change (or close) was observed.
    pub fn wait_change(&self, seen: u64, timeout: Duration) -> bool {
        let g = lock_inner(&self.inner);
        let (g, _) = self
            .cond
            .wait_timeout_while(g, timeout, |i| i.epoch == seen && !i.closed)
            .unwrap_or_else(|e| e.into_inner());
        g.epoch != seen || g.closed
    }

    /// Replace the wall clock (tests move time with this).
    pub fn set_clock(&self, clock: impl Fn() -> Time + Send + Sync + 'static) {
        lock_inner(&self.inner).now = Arc::new(clock);
    }

    /// Run `f` under the store lock and wake pollers if the generation moved.
    fn with<T>(&self, f: impl FnOnce(&mut Inner) -> Result<T>) -> Result<T> {
        let mut g = lock_inner(&self.inner);
        let before = g.epoch;
        let out = f(&mut g);
        let moved = g.epoch != before;
        drop(g);
        if moved {
            self.cond.notify_all();
        }
        out
    }

    /// Persist expired execution leases and prune expired messages, independent of
    /// HTTP activity (Go: `Tick`).
    pub fn tick(&self) -> Result<()> {
        self.with(|i| {
            if i.closed {
                return Err(Error::new("store is closed"));
            }
            i.expire(false)?;
            let at = i.now_utc();
            i.prune_messages(at)
        })
    }

    /// Full monitor view: all clients/tasks/events plus the newest messages.
    pub fn snapshot(&self) -> Result<State> {
        self.with(|i| {
            i.ensure_open()?;
            let mut out = i.visible_state(None)?;
            out.messages = i.recent_messages(100)?;
            Ok(out)
        })
    }

    pub fn join_keys(&self) -> (String, String) {
        let g = lock_inner(&self.inner);
        (g.master_join.clone(), g.worker_join.clone())
    }

    /// Raw task record including `cleared` tombstones (audit / test access).
    pub fn raw_task(&self, id: &str) -> Option<Task> {
        lock_inner(&self.inner).tasks.get(id).cloned()
    }

    pub fn execute(&self, token: &str, r: &Request) -> Result<Outcome> {
        self.with(|i| i.execute(token, r))
    }

    pub fn send_chat(&self, token: &str, r: &Request) -> Result<Message> {
        self.with(|i| i.send_chat(token, r))
    }

    pub fn poll_messages(&self, token: &str, after: i64, limit: i64) -> Result<PollResult> {
        self.with(|i| i.poll_messages(token, after, limit))
    }

    pub fn ack_messages(&self, token: &str, expected: i64, seq: i64) -> Result<AckResult> {
        self.with(|i| i.ack_messages(token, expected, seq))
    }

    pub fn message_history(&self, token: &str, task_id: &str, worker_id: &str, limit: i64) -> Result<Vec<Message>> {
        self.with(|i| i.message_history(token, task_id, worker_id, limit))
    }
}

impl Inner {
    fn now_utc(&self) -> Time {
        (self.now)()
    }

    fn ensure_open(&self) -> Result<&Connection> {
        if self.closed {
            return Err(Error::new("store is closed"));
        }
        self.db.as_ref().ok_or_else(|| Error::new("store is closed"))
    }

    fn bump(&mut self) {
        self.epoch += 1;
    }

    fn get_receipt(&self, key: &str) -> Result<Option<Receipt>> {
        let db = self.ensure_open()?;
        let raw: Option<Vec<u8>> =
            db.query_row("SELECT data FROM receipts WHERE id=?1", params![key], |row| row.get(0)).optional()?;
        match raw {
            None => Ok(None),
            Some(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        }
    }

    /// Commit a draft in one transaction, then publish it in memory (Go: `save`).
    /// Event bodies are moved into the retained `messages` table; the audit event
    /// row keeps an empty body.
    fn save(&mut self, mut d: Draft) -> Result<()> {
        let db = self.ensure_open()?;
        let tx = db.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO meta (key,value) VALUES ('next_seq',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![d.next_seq.to_string()],
        )?;
        for (id, client) in &d.clients {
            if self.clients.get(id) != Some(client) {
                upsert_json(&tx, "clients", id, client)?;
            }
        }
        for (id, task) in &d.tasks {
            if self.tasks.get(id) != Some(task) {
                upsert_json(&tx, "tasks", id, task)?;
            }
        }
        let mut drafts = Vec::with_capacity(d.events.len());
        for event in d.events.iter_mut() {
            drafts.push(messages::MessageDraft {
                to: event.to.clone(),
                task_id: event.task_id.clone(),
                event_kind: event.kind.clone(),
                body: std::mem::take(&mut event.body),
                at: event.at,
            });
            tx.execute(
                "INSERT INTO events (seq,to_id,task_id,data) VALUES (?1,?2,?3,?4)",
                params![event.seq, event.to, event.task_id, serde_json::to_vec(&*event)?],
            )?;
        }
        for (i, entry) in d.history.iter().enumerate() {
            tx.execute(
                "INSERT INTO history (id,data) VALUES (?1,?2)",
                params![self.history_len + i as i64 + 1, serde_json::to_vec(entry)?],
            )?;
        }
        if let Some((key, receipt)) = &d.receipt {
            tx.execute("INSERT INTO receipts (id,data) VALUES (?1,?2)", params![key, serde_json::to_vec(receipt)?])?;
        }
        for draft in &drafts {
            // Before any client joined there is no recipient. The audit event stays
            // durable; a later join learns current work via tasks.
            if draft.to.is_empty() && d.clients.is_empty() {
                continue;
            }
            messages::insert_message(
                &tx,
                &d.clients,
                "system",
                &draft.to,
                &draft.task_id,
                "task_event",
                &draft.event_kind,
                &draft.body,
                draft.at,
            )?;
        }
        tx.commit()?;
        self.history_len += d.history.len() as i64;
        self.clients = d.clients;
        self.tasks = d.tasks;
        self.next_seq = d.next_seq;
        self.bump();
        Ok(())
    }

    fn expire(&mut self, restarted: bool) -> Result<()> {
        let at = self.now_utc();
        let due = |t: &Task| t.state == "running" && (restarted || at >= t.lease_until);
        let ids: Vec<String> = self.tasks.values().filter(|t| due(t)).map(|t| t.id.clone()).collect();
        if ids.is_empty() {
            return Ok(());
        }
        let mut d = Draft::from_inner(self);
        for id in ids {
            let Some(t) = d.tasks.get_mut(&id) else { continue };
            t.state = "unconfirmed".to_string();
            t.version += 1;
            t.updated = at;
            let task = t.clone();
            let master = d.master_id();
            d.add_event(
                &master,
                &id,
                "unconfirmed",
                "Worker execution is unconfirmed; ownership is retained until confirmed stopped.",
                at,
            );
            d.history.push(HistoryEntry {
                at,
                client_id: String::new(),
                request: Request { op: "unconfirmed".to_string(), ..Request::default() },
                task,
            });
        }
        self.save(d)
    }

    /// Clients with credentials removed and live contact times overlaid.
    fn visible_clients(&self, overlay: bool) -> BTreeMap<String, Client> {
        let mut out = BTreeMap::new();
        for (id, c) in &self.clients {
            let mut c = c.clone();
            c.token.clear();
            if overlay {
                if let Some(seen) = self.contact.get(id)
                    && *seen > c.seen
                {
                    c.seen = *seen;
                }
                if let Some(poll_at) = self.poll_contact.get(id) {
                    c.poll_at = *poll_at;
                }
            }
            out.insert(id.clone(), c);
        }
        out
    }

    fn visible_tasks(&self, viewer: Option<&Client>) -> BTreeMap<String, Task> {
        self.tasks
            .iter()
            .filter(|(_, t)| {
                t.state != "cleared"
                    && match viewer {
                        None => true,
                        Some(v) => {
                            v.role == "master" || t.worker_id == v.id || (t.state == "queued" && t.worker_id.is_empty())
                        }
                    }
            })
            .map(|(id, t)| (id.clone(), t.clone()))
            .collect()
    }

    /// Newest `EVENT_LIMIT` audit events visible to `viewer` (all for `None`).
    fn visible_events(&self, viewer: Option<&Client>) -> Result<Vec<Event>> {
        let db = self.ensure_open()?;
        let limit = EVENT_LIMIT as i64;
        let mut raw: Vec<Vec<u8>> = Vec::new();
        match viewer {
            None => {
                let mut stmt = db.prepare("SELECT data FROM events ORDER BY seq DESC LIMIT ?1")?;
                for row in stmt.query_map(params![limit], |row| row.get::<_, Vec<u8>>(0))? {
                    raw.push(row?);
                }
            }
            Some(v) => {
                let mut stmt =
                    db.prepare("SELECT data FROM events WHERE to_id='' OR to_id=?1 ORDER BY seq DESC LIMIT ?2")?;
                for row in stmt.query_map(params![v.id, limit], |row| row.get::<_, Vec<u8>>(0))? {
                    raw.push(row?);
                }
            }
        }
        let mut events = Vec::with_capacity(raw.len());
        for bytes in raw.into_iter().rev() {
            events.push(serde_json::from_slice::<Event>(&bytes)?);
        }
        Ok(events)
    }

    fn visible_state(&self, viewer: Option<&Client>) -> Result<State> {
        Ok(State {
            clients: self.visible_clients(true),
            tasks: self.visible_tasks(viewer),
            events: self.visible_events(viewer)?,
            ..State::default()
        })
    }

    fn replay_result(receipt: &Receipt) -> Result<Outcome> {
        let value = receipt.result.clone();
        let op = receipt.request.op.as_str();
        Ok(match op {
            "join" => Outcome::Client(serde_json::from_value(value)?),
            "clear-tasks" => Outcome::Cleared(serde_json::from_value(value)?),
            "heartbeat" if receipt.request.task_id.is_empty() => Outcome::Client(serde_json::from_value(value)?),
            _ => Outcome::Task(serde_json::from_value(value)?),
        })
    }

    fn execute(&mut self, token: &str, r: &Request) -> Result<Outcome> {
        self.ensure_open()?;
        self.expire(false).map_err(|e| e.context("persist expiry"))?;
        let mut actor = Client::default();
        let scope: String;
        if r.op == "join" {
            let key = match r.role.as_str() {
                "master" => &self.master_join,
                "worker" => &self.worker_join,
                _ => return Err(Error::new("role must be master or worker")),
            };
            if !token_equal(token, key) {
                return Err(Error::new("unauthorized join"));
            }
            scope = format!("join:{}", r.role);
        } else {
            let found = self.clients.values().find(|c| token_equal(&c.token, token)).cloned();
            actor = found.ok_or_else(|| Error::new("unauthorized client"))?;
            if !r.client_id.is_empty() && r.client_id != actor.id {
                return Err(Error::new("client identity mismatch"));
            }
            let now = self.now_utc();
            self.contact.insert(actor.id.clone(), now);
            scope = actor.id.clone();
            match r.op.as_str() {
                "status" => return Ok(Outcome::Status(self.visible_state(Some(&actor))?)),
                "clients" => {
                    let clients: Vec<Client> = self.visible_clients(true).into_values().collect();
                    return Ok(Outcome::Clients(clients));
                }
                "tasks" => {
                    let tasks: Vec<Task> = self
                        .visible_tasks(Some(&actor))
                        .into_values()
                        .filter(|t| r.task_id.is_empty() || r.task_id == t.id)
                        .collect();
                    return Ok(Outcome::Tasks(tasks));
                }
                _ => {}
            }
        }
        if r.request_id.is_empty() || r.request_id.len() > 128 {
            return Err(Error::new("mutation requires request_id (1..128 characters)"));
        }
        let key = format!("{scope}:{}", r.request_id);
        if let Some(old) = self.get_receipt(&key)? {
            if old.request != *r {
                return Err(Error::new("request_id already used with different payload"));
            }
            let replayed = Self::replay_result(&old)?;
            if let Outcome::Task(t) = &replayed
                && self.tasks.get(&t.id).map(|c| c.state == "cleared").unwrap_or(false)
            {
                return Err(Error::new("recorded task was cleared; inspect current tasks"));
            }
            if r.op == "claim"
                && let Outcome::Task(t) = &replayed
            {
                let executable = self.tasks.get(&t.id).map(|c| c.generation == t.generation && c.state == "running");
                if executable != Some(true) {
                    return Err(Error::new("recorded claim is no longer executable; inspect current task"));
                }
            }
            return Ok(replayed);
        }

        let mut d = Draft::from_inner(self);
        let at = self.now_utc();
        let lease = ChronoDuration::seconds(LEASE_SECONDS);
        let mut result: Option<Outcome> = None;
        let mut changed: Option<Task> = None;

        match r.op.as_str() {
            "join" => {
                if r.name.trim().is_empty() || r.name.len() > 120 {
                    return Err(Error::new("client name is required (up to 120 characters)"));
                }
                for c in d.clients.values() {
                    if r.role == "master" && c.role == "master" {
                        if c.name != r.name {
                            return Err(Error::new(
                                "a master already exists; reconnect using its profile or original name",
                            ));
                        }
                        let mut c = c.clone();
                        c.seen = at;
                        result = Some(Outcome::Client(c));
                        break;
                    }
                    if r.role == "worker" && c.role == "worker" && c.name == r.name {
                        return Err(Error::new("worker name already exists; reuse its profile or choose a new name"));
                    }
                }
                let joined = match result.take() {
                    Some(Outcome::Client(existing)) => existing,
                    _ => Client {
                        id: random_hex(12)?,
                        role: r.role.clone(),
                        name: r.name.clone(),
                        token: random_hex(32)?,
                        seen: at,
                        poll_at: zero_time(),
                    },
                };
                d.clients.insert(joined.id.clone(), joined.clone());
                result = Some(Outcome::Client(joined));
            }
            "create" => {
                master_only(&actor)?;
                if r.title.trim().is_empty() {
                    return Err(Error::new("title is required"));
                }
                if r.mode != "write" && r.mode != "read" {
                    return Err(Error::new("mode must be write or read"));
                }
                let workspace = canonical_workspace(&r.workspace)?;
                if !r.worker_id.is_empty() {
                    match d.clients.get(&r.worker_id) {
                        Some(c) if c.role == "worker" => {}
                        _ => return Err(Error::new("unknown worker")),
                    }
                }
                let id = if r.task_id.is_empty() { random_hex(12)? } else { r.task_id.clone() };
                if id.len() > 128 || id.trim().is_empty() {
                    return Err(Error::new("invalid task ID"));
                }
                if d.tasks.contains_key(&id) {
                    return Err(Error::new("task ID already exists"));
                }
                let t = Task {
                    id: id.clone(),
                    title: r.title.clone(),
                    workspace,
                    mode: r.mode.clone(),
                    worker_id: r.worker_id.clone(),
                    state: "queued".to_string(),
                    instruction: r.body.clone(),
                    body: r.body.clone(),
                    version: 1,
                    updated: at,
                    ..Task::default()
                };
                d.add_event(&r.worker_id, &id, "queued", &r.title, at);
                changed = Some(t);
            }
            "clear-tasks" => {
                master_only(&actor)?;
                let workspace = canonical_workspace(&r.workspace)?;
                if r.expected_count <= 0 {
                    return Err(Error::new("expected_count must be positive"));
                }
                if r.body.trim().is_empty() || rune_len(&r.body) > 1000 {
                    return Err(Error::new("clear reason is required (up to 1000 characters)"));
                }
                let mut ids: Vec<String> = Vec::new();
                for (id, t) in &d.tasks {
                    if t.state != "cleared" && exact_workspace(&t.workspace, &workspace) {
                        if t.state == "running" || t.state == "unconfirmed" {
                            return Err(err!("task {id} is still {}; stop execution before clearing", t.state));
                        }
                        ids.push(id.clone());
                    }
                }
                if ids.len() as i64 != r.expected_count {
                    return Err(err!("task count changed: expected {}, current {}", r.expected_count, ids.len()));
                }
                ids.sort();
                let mut affected: Vec<String> = Vec::new();
                for id in &ids {
                    let t = d.tasks.get_mut(id).expect("listed task exists");
                    if !t.worker_id.is_empty() && !affected.contains(&t.worker_id) {
                        affected.push(t.worker_id.clone());
                    }
                    t.state = "cleared".to_string();
                    t.version += 1;
                    t.generation += 1;
                    t.lease_until = zero_time();
                    t.updated = at;
                    let task = t.clone();
                    d.history.push(HistoryEntry { at, client_id: actor.id.clone(), request: r.clone(), task });
                }
                let master = d.master_id();
                d.add_event(
                    &master,
                    "",
                    "tasks_cleared",
                    &format!("Cleared {} tasks for {}: {}", ids.len(), workspace, r.body),
                    at,
                );
                affected.sort();
                for worker in &affected {
                    d.add_event(
                        worker,
                        "",
                        "tasks_cleared",
                        "Assigned tasks were cleared; inspect current tasks before acting.",
                        at,
                    );
                }
                result = Some(Outcome::Cleared(ClearTasksResult { workspace, count: ids.len() as i64, task_ids: ids }));
            }
            "claim" => {
                if actor.role != "worker" {
                    return Err(Error::new("worker role required"));
                }
                // Claim fencing is optional (simplification proposal 3); present values are checked.
                let mut t = fenced_task(&d, r, true)?;
                if t.state != "queued" {
                    return Err(Error::new("task is not queued"));
                }
                if !t.worker_id.is_empty() && t.worker_id != actor.id {
                    return Err(Error::new("task is assigned to another worker"));
                }
                if r.generation.is_some_and(|g| g != t.generation) {
                    return Err(Error::new("stale claim generation"));
                }
                if t.mode == "write" {
                    for other in d.tasks.values() {
                        if other.id != t.id
                            && other.mode == "write"
                            && same_workspace(&other.workspace, &t.workspace)
                            && other.state != "queued"
                            && other.state != "passed"
                            && other.state != "cleared"
                        {
                            return Err(err!("workspace is still owned by task {} ({})", other.id, other.state));
                        }
                    }
                }
                t.worker_id = actor.id.clone();
                t.state = "running".to_string();
                t.generation += 1;
                t.version += 1;
                t.updated = at;
                t.lease_until = at + lease;
                t.progress_percent = 0;
                t.progress_note = NOTE_CLAIMED.to_string();
                t.progress_at = at;
                t.subtasks.clear();
                d.add_event(&d.master_id(), &t.id, "claimed", &actor.name, at);
                changed = Some(t);
            }
            "progress" => {
                let mut t = get_task(&d, r)?;
                owned(&actor, r, &t)?;
                if t.state != "running" {
                    return Err(Error::new("task is not running"));
                }
                if !(0..=100).contains(&r.percent) {
                    return Err(Error::new("percent must be 0..100"));
                }
                if r.description.trim().is_empty() || rune_len(&r.description) > 200 {
                    return Err(Error::new("progress description is required (up to 200 characters)"));
                }
                if r.subtask_id.is_empty() && !r.subtask_title.is_empty() {
                    return Err(Error::new("subtask title requires subtask ID"));
                }
                if !r.subtask_id.is_empty() {
                    if r.subtask_id.len() > 64 || r.subtask_id.trim() != r.subtask_id {
                        return Err(Error::new("subtask ID must be nonblank and at most 64 characters"));
                    }
                    if rune_len(&r.subtask_title) > 120 {
                        return Err(Error::new("subtask title exceeds 120 characters"));
                    }
                    if let Some(sub) = t.subtasks.iter_mut().find(|s| s.id == r.subtask_id) {
                        if !r.subtask_title.is_empty() {
                            sub.title = r.subtask_title.clone();
                        }
                        sub.percent = r.percent;
                        sub.description = r.description.clone();
                        sub.updated = at;
                    } else {
                        if r.subtask_title.trim().is_empty() {
                            return Err(Error::new("new subtask requires a title"));
                        }
                        if t.subtasks.len() >= 50 {
                            return Err(Error::new("task has reached 50 subtasks"));
                        }
                        t.subtasks.push(Subtask {
                            id: r.subtask_id.clone(),
                            title: r.subtask_title.clone(),
                            percent: r.percent,
                            description: r.description.clone(),
                            updated: at,
                        });
                    }
                } else {
                    t.progress_percent = r.percent;
                    t.progress_note = r.description.clone();
                    t.progress_at = at;
                }
                t.version += 1;
                t.updated = at;
                // Owner progress renews the lease like a heartbeat (simplification proposal 1).
                t.lease_until = at + lease;
                d.add_event(&d.master_id(), &t.id, "progress", &r.description, at);
                changed = Some(t);
            }
            "heartbeat" => {
                if r.task_id.is_empty() {
                    actor.seen = at;
                    let mut c = actor.clone();
                    c.token.clear();
                    result = Some(Outcome::Client(c));
                } else {
                    let mut t = get_task(&d, r)?;
                    owned(&actor, r, &t)?;
                    if t.state != "running" && t.state != "unconfirmed" {
                        return Err(Error::new("task is frozen; heartbeat cannot resume execution"));
                    }
                    t.state = "running".to_string();
                    t.version += 1;
                    t.updated = at;
                    t.lease_until = at + lease;
                    changed = Some(t);
                }
            }
            "submit" => {
                let mut t = get_task(&d, r)?;
                owned(&actor, r, &t)?;
                if t.state != "running" {
                    return Err(Error::new("task is not running; renew an unconfirmed claim before submitting"));
                }
                if r.body.trim().is_empty() || r.snapshot.trim().is_empty() {
                    return Err(Error::new("submission body and snapshot are required"));
                }
                t.state = "awaiting_review".to_string();
                t.body = r.body.clone();
                t.snapshot = r.snapshot.clone();
                t.version += 1;
                t.updated = at;
                t.lease_until = zero_time();
                d.add_event(&d.master_id(), &t.id, "submitted", &r.body, at);
                changed = Some(t);
            }
            "review" => {
                master_only(&actor)?;
                let mut t = get_task(&d, r)?;
                if t.state != "awaiting_review" && t.state != "blocked" {
                    return Err(Error::new("task is not awaiting review"));
                }
                if r.snapshot.is_empty() || r.snapshot != t.snapshot {
                    return Err(Error::new("submission snapshot mismatch"));
                }
                if r.body.trim().is_empty() {
                    return Err(Error::new("review explanation is required"));
                }
                match r.decision.as_str() {
                    "pass" => t.state = "passed".to_string(),
                    "changes_requested" => {
                        t.state = "running".to_string();
                        t.snapshot.clear();
                        t.lease_until = at + lease;
                        t.progress_percent = 0;
                        t.progress_note = NOTE_REWORK.to_string();
                        t.progress_at = at;
                        t.subtasks.clear();
                    }
                    "blocked" => t.state = "blocked".to_string(),
                    _ => return Err(Error::new("decision must be pass, changes_requested or blocked")),
                }
                t.body = r.body.clone();
                t.version += 1;
                t.updated = at;
                d.add_event(&t.worker_id, &t.id, &r.decision, &r.body, at);
                changed = Some(t);
            }
            "confirm-stop" => {
                master_only(&actor)?;
                let mut t = get_task(&d, r)?;
                if t.state == "queued" || t.state == "passed" || t.state == "cleared" {
                    return Err(Error::new("task has no execution ownership to release"));
                }
                if r.generation != Some(t.generation) {
                    return Err(Error::new("stale claim generation"));
                }
                if r.evidence.trim().is_empty() {
                    return Err(Error::new("evidence confirming the old execution has stopped is required"));
                }
                d.add_event(&t.worker_id, &t.id, "stopped", &r.evidence, at);
                t.state = "queued".to_string();
                t.worker_id.clear();
                t.generation += 1;
                t.version += 1;
                t.updated = at;
                t.lease_until = zero_time();
                t.snapshot.clear();
                t.body = t.instruction.clone();
                t.progress_percent = 0;
                t.progress_note.clear();
                t.progress_at = zero_time();
                t.subtasks.clear();
                d.add_event(
                    "",
                    &t.id,
                    "queued",
                    "Previous execution confirmed stopped; task may be claimed again.",
                    at,
                );
                changed = Some(t);
            }
            _ => return Err(Error::new("unknown operation")),
        }

        if !actor.id.is_empty() {
            actor.seen = at;
            d.clients.insert(actor.id.clone(), actor.clone());
        }
        if let Some(t) = changed {
            d.tasks.insert(t.id.clone(), t.clone());
            d.history.push(HistoryEntry { at, client_id: actor.id.clone(), request: r.clone(), task: t.clone() });
            result = Some(Outcome::Task(t));
        }
        let outcome = result.ok_or_else(|| Error::new("operation produced no result"))?;
        d.receipt = Some((key, Receipt { request: r.clone(), result: outcome.to_value() }));
        self.save(d).map_err(|e| e.context("persist operation"))?;
        Ok(outcome)
    }
}

fn upsert_json<T: serde::Serialize>(tx: &Connection, table: &str, id: &str, value: &T) -> Result<()> {
    tx.execute(
        &format!("INSERT INTO {table} (id,data) VALUES (?1,?2) ON CONFLICT(id) DO UPDATE SET data=excluded.data"),
        params![id, serde_json::to_vec(value)?],
    )?;
    Ok(())
}
