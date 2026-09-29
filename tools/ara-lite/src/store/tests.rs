//! Behavioral tests for the state machine and message store.
//!
//! These are ports of the Go tests in `core_test.go`, `message_store_test.go` and
//! `clear_tasks_test.go`. They live inside the crate because they need the store
//! internals (memory vs. disk comparison, the SQLite handle, the injectable clock).
//! `docs/port-map.md` lists the one-to-one mapping.

use super::*;
use chrono::{Duration as ChronoDuration, Utc};
use std::cell::Cell;
use std::thread;

/// Wire value of the claim note (Chinese "claimed"), kept identical to the Go service.
const WIRE_CLAIMED: &str = "\u{5df2}\u{9886}\u{53d6}";
/// Wire value of the rework note (Chinese "needs rework"), kept identical to the Go service.
const WIRE_REWORK: &str = "\u{9700}\u{8fd4}\u{5de5}";

/// A movable clock shared between a test and the store under test.
#[derive(Clone)]
struct TestClock(Arc<Mutex<Time>>);

impl TestClock {
    fn new(start: Time) -> TestClock {
        TestClock(Arc::new(Mutex::new(start)))
    }

    fn advance(&self, seconds: i64) {
        let mut t = self.0.lock().unwrap();
        *t += ChronoDuration::seconds(seconds);
    }

    fn install(&self, store: &Store) {
        let shared = self.0.clone();
        store.set_clock(move || *shared.lock().unwrap());
    }
}

struct Fixture {
    root: tempfile::TempDir,
    data: PathBuf,
    s: Store,
    master: Client,
    a: Client,
    b: Client,
    seq: Cell<u64>,
    workspaces: Cell<u32>,
}

fn req(op: &str) -> Request {
    Request { op: op.to_string(), ..Request::default() }
}

fn task_request(op: &str, t: &Task) -> Request {
    Request {
        op: op.to_string(),
        task_id: t.id.clone(),
        version: Some(t.version),
        generation: Some(t.generation),
        ..Request::default()
    }
}

impl Fixture {
    fn new() -> Fixture {
        Fixture::with_clock(None)
    }

    fn with_clock(clock: Option<&TestClock>) -> Fixture {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let s = Store::open(&data).unwrap();
        if let Some(clock) = clock {
            clock.install(&s);
        }
        let (master_key, worker_key) = s.join_keys();
        let mut f = Fixture {
            root,
            data,
            s,
            master: Client::default(),
            a: Client::default(),
            b: Client::default(),
            seq: Cell::new(0),
            workspaces: Cell::new(0),
        };
        f.master = f.join(&master_key, "master", "master");
        f.a = f.join(&worker_key, "worker", "worker-a");
        f.b = f.join(&worker_key, "worker", "worker-b");
        f
    }

    fn join(&self, key: &str, role: &str, name: &str) -> Client {
        let r = Request { op: "join".into(), role: role.into(), name: name.into(), ..Request::default() };
        self.call(key, r).into_client().expect("join returns a client")
    }

    /// A fresh directory that exists and never overlaps another fixture workspace.
    fn workspace(&self) -> String {
        let n = self.workspaces.get() + 1;
        self.workspaces.set(n);
        let dir = self.root.path().join(format!("ws-{n}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir.to_string_lossy().into_owned()
    }

    fn request(&self, mut r: Request) -> Request {
        let n = self.seq.get() + 1;
        self.seq.set(n);
        if r.request_id.is_empty() {
            r.request_id = n.to_string();
        }
        r
    }

    fn call(&self, token: &str, r: Request) -> Outcome {
        let op = r.op.clone();
        match self.s.execute(token, &self.request(r)) {
            Ok(v) => v,
            Err(e) => panic!("{op}: {e}"),
        }
    }

    fn call_task(&self, token: &str, r: Request) -> Task {
        self.call(token, r).into_task().expect("operation returns a task")
    }

    fn fail(&self, token: &str, r: Request, part: &str) {
        let op = r.op.clone();
        match self.s.execute(token, &self.request(r)) {
            Err(e) if e.message().contains(part) => {}
            other => panic!("{op}: want error containing {part:?}, got {other:?}"),
        }
    }

    fn create(&self, workspace: &str, mode: &str, worker: &str) -> Task {
        let r = Request {
            op: "create".into(),
            title: "Implement narrow batch".into(),
            workspace: workspace.into(),
            mode: mode.into(),
            worker_id: worker.into(),
            body: "Original task instruction".into(),
            ..Request::default()
        };
        self.call_task(&self.master.token, r)
    }

    fn send(&self, token: &str, request_id: &str, task_id: &str, worker_id: &str, body: &str) -> Result<Message> {
        let r = Request {
            request_id: request_id.into(),
            task_id: task_id.into(),
            worker_id: worker_id.into(),
            body: body.into(),
            ..Request::default()
        };
        self.s.send_chat(token, &r)
    }

    fn poll(&self, token: &str, after: i64) -> PollResult {
        self.s.poll_messages(token, after, 100).unwrap()
    }

    fn client_view(&self, id: &str) -> Client {
        self.s.snapshot().unwrap().clients.get(id).cloned().expect("client is visible")
    }

    /// Close and reopen the same data directory (a process restart).
    fn reopen(&mut self) {
        self.s.close();
        self.s = Store::open(&self.data).unwrap();
    }
}

/// The durable, in-memory part of the store (used to prove a failed write changed nothing).
fn memory(s: &Store) -> (BTreeMap<String, Client>, BTreeMap<String, Task>, i64, i64) {
    let g = lock_inner(&s.inner);
    (g.clients.clone(), g.tasks.clone(), g.next_seq, g.history_len)
}

fn sql(s: &Store, statement: &str) {
    let g = lock_inner(&s.inner);
    g.db.as_ref().expect("store is open").execute_batch(statement).unwrap();
}

fn sql_count(s: &Store, query: &str) -> i64 {
    let g = lock_inner(&s.inner);
    g.db.as_ref().expect("store is open").query_row(query, [], |row| row.get(0)).unwrap()
}

fn history(s: &Store) -> Vec<HistoryEntry> {
    let g = lock_inner(&s.inner);
    let db = g.db.as_ref().expect("store is open");
    let mut stmt = db.prepare("SELECT data FROM history ORDER BY id").unwrap();
    let rows = stmt.query_map([], |row| row.get::<_, Vec<u8>>(0)).unwrap();
    rows.map(|r| serde_json::from_slice(&r.unwrap()).unwrap()).collect()
}

// ---- core_test.go ----

#[test]
fn multi_worker_review_and_fencing() {
    let f = Fixture::new();
    let ws = f.workspace();
    let mut task = f.create(&ws, "write", "");
    let mut claim = task_request("claim", &task);
    claim.request_id = "race-claim".into();

    let results: Vec<Result<Outcome>> = thread::scope(|scope| {
        let handles: Vec<_> = [&f.a, &f.b]
            .into_iter()
            .map(|worker| {
                let (store, claim) = (&f.s, claim.clone());
                scope.spawn(move || store.execute(&worker.token, &claim))
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1, "exactly one claim must win: {results:?}");
    let (winner, loser) = if results[0].is_ok() { (f.a.clone(), f.b.clone()) } else { (f.b.clone(), f.a.clone()) };
    for outcome in results.into_iter().flatten() {
        task = outcome.into_task().unwrap();
    }

    f.fail(&loser.token, task_request("heartbeat", &task), "another worker");
    let other = f.create(&ws, "write", &loser.id);
    f.fail(&loser.token, task_request("claim", &other), "workspace is still owned");
    let parallel = f.create(&f.workspace(), "write", &loser.id);
    f.call(&loser.token, task_request("claim", &parallel));

    let mut r = task_request("submit", &task);
    r.body = "First implementation + test evidence".into();
    r.snapshot = "head-a:branch:sha256-a".into();
    task = f.call_task(&winner.token, r);
    f.fail(&winner.token, task_request("heartbeat", &task), "frozen");

    let mut review = task_request("review", &task);
    review.snapshot = task.snapshot.clone();
    review.body = "Fix one reachable bug".into();
    review.decision = "changes_requested".into();
    f.fail(&loser.token, review.clone(), "master role");
    let mut bad = review.clone();
    bad.snapshot = "different".into();
    f.fail(&f.master.token, bad, "snapshot mismatch");
    let prior = task.clone();
    task = f.call_task(&f.master.token, review.clone());
    assert!(task.worker_id == winner.id && task.state == "running" && task.version > prior.version, "{task:?}");

    f.fail(&loser.token, task_request("submit", &task), "another worker");
    let mut r = task_request("submit", &task);
    r.body = "Repaired implementation + proof".into();
    r.snapshot = "head-b:branch:sha256-b".into();
    task = f.call_task(&winner.token, r);
    f.fail(&f.master.token, review, "version conflict");

    let mut review = task_request("review", &task);
    review.snapshot = task.snapshot.clone();
    review.body = "Reviewed exact revision".into();
    review.decision = "pass".into();
    task = f.call_task(&f.master.token, review);
    assert_eq!(task.state, "passed");
    f.call(&loser.token, task_request("claim", &other));

    let kinds = |token: &str| -> Vec<String> {
        f.poll(token, -1).messages.into_iter().filter(|m| m.task_id == task.id).map(|m| m.event_kind).collect()
    };
    let winner_kinds = kinds(&winner.token);
    assert!(
        winner_kinds.iter().any(|k| k == "changes_requested") && winner_kinds.iter().any(|k| k == "pass"),
        "missing review delivery: {winner_kinds:?}"
    );
    let loser_kinds = kinds(&loser.token);
    assert!(
        !loser_kinds.iter().any(|k| k == "pass" || k == "changes_requested"),
        "other worker received review: {loser_kinds:?}"
    );
    let entries = lock_inner(&f.s.inner).history_len;
    assert!(entries >= 8, "history was lost: {entries}");
}

#[test]
fn expiry_restart_and_confirmed_stop() {
    let clock = TestClock::new(Utc::now());
    let mut f = Fixture::with_clock(Some(&clock));
    let dir = f.workspace();
    let mut task = f.create(&dir, "write", &f.a.id);
    task = f.call_task(&f.a.token, task_request("claim", &task));
    let old = task.clone();

    clock.advance(LEASE_SECONDS + 1);
    f.call(&f.master.token, req("status"));
    task = f.s.snapshot().unwrap().tasks[&task.id].clone();
    assert!(task.state == "unconfirmed" && task.worker_id == f.a.id, "expiry released ownership: {task:?}");

    let other = f.create(&dir, "write", &f.b.id);
    f.fail(&f.b.token, task_request("claim", &other), "workspace is still owned");
    task = f.call_task(&f.a.token, task_request("heartbeat", &task));
    assert_eq!(task.state, "running");

    f.reopen();
    task = f.s.snapshot().unwrap().tasks[&task.id].clone();
    assert_eq!(task.state, "unconfirmed", "restart must retain unconfirmed ownership: {task:?}");
    f.fail(&f.b.token, task_request("claim", &other), "workspace is still owned");

    let mut stop = task_request("confirm-stop", &task);
    f.fail(&f.master.token, stop.clone(), "evidence");
    stop.evidence = "Owner acknowledged stopping all workspace writes".into();
    task = f.call_task(&f.master.token, stop);
    assert!(
        task.state == "queued"
            && task.worker_id.is_empty()
            && task.generation > old.generation
            && task.body == task.instruction,
        "bad requeue: {task:?}"
    );

    let mut stale = task_request("heartbeat", &old);
    stale.version = Some(task.version);
    f.fail(&f.a.token, stale, "generation");
    task = f.call_task(&f.b.token, task_request("claim", &task));
    assert!(task.worker_id == f.b.id && task.generation > old.generation, "reassigned improperly: {task:?}");
}

#[test]
fn permissions_idempotence_and_snapshots() {
    let f = Fixture::new();
    let (m, w) = f.s.join_keys();
    let join = |role: &str, name: &str| Request {
        op: "join".into(),
        role: role.into(),
        name: name.into(),
        ..Request::default()
    };
    f.fail(&w, join("master", "evil"), "unauthorized");
    f.fail(&m, join("master", "master-two"), "master already exists");
    f.fail(&w, join("worker", &f.a.name), "worker name already exists");
    f.fail(&f.a.token, Request { op: "create".into(), title: "bad".into(), ..Request::default() }, "master role");
    f.fail(
        &f.a.token,
        Request { op: "status".into(), client_id: f.b.id.clone(), ..Request::default() },
        "identity mismatch",
    );

    let mut request = Request {
        request_id: "same".into(),
        worker_id: f.a.id.clone(),
        body: "Continue bounded work".into(),
        ..Request::default()
    };
    let v1 = f.s.send_chat(&f.master.token, &request).unwrap();
    let v2 = f.s.send_chat(&f.master.token, &request).unwrap();
    assert_eq!(v1, v2, "retry changed result");
    request.body = "Different work".into();
    let changed = f.s.send_chat(&f.master.token, &request).unwrap_err();
    assert!(changed.message().contains("different payload"), "changed payload accepted: {changed}");

    for c in f.s.snapshot().unwrap().clients.values() {
        assert!(c.token.is_empty(), "monitor exposed token");
    }
    let state = f.call(&f.a.token, req("status")).into_status().unwrap();
    assert!(state.master_join.is_empty() && state.worker_join.is_empty(), "exposed join secrets");
    for c in f.call(&f.master.token, req("clients")).into_clients().unwrap() {
        assert!(c.token.is_empty(), "clients exposed token");
    }
}

#[test]
fn overlapping_workspaces_and_read_mode() {
    let f = Fixture::new();
    let parent = f.workspace();
    let child = Path::new(&parent).join("child");
    std::fs::create_dir(&child).unwrap();
    let child = child.to_string_lossy().into_owned();

    let task = f.create(&child, "write", &f.a.id);
    f.call(&f.a.token, task_request("claim", &task));
    let outer = f.create(&parent, "write", &f.b.id);
    f.fail(&f.b.token, task_request("claim", &outer), "workspace is still owned");
    let read = f.create(&parent, "read", &f.b.id);
    f.call(&f.b.token, task_request("claim", &read));

    assert!(!same_workspace(&parent, &format!("{parent}-sibling")), "sibling path falsely overlaps");
    assert!(same_workspace(&parent, &child) && same_workspace(&child, &parent));

    let file = Path::new(&parent).join("file");
    std::fs::write(&file, b"x").unwrap();
    let create = Request {
        op: "create".into(),
        title: "file".into(),
        workspace: file.to_string_lossy().into_owned(),
        mode: "write".into(),
        ..Request::default()
    };
    f.fail(&f.master.token, create, "directory");
}

#[test]
fn persistence_failure_and_notifications() {
    let f = Fixture::new();
    let epoch = f.s.epoch();
    f.call(&f.master.token, req("status"));
    assert_eq!(f.s.epoch(), epoch, "read notified");

    let before = memory(&f.s);
    let contact_before = f.client_view(&f.master.id).seen;
    f.s.set_clock(move || contact_before + ChronoDuration::seconds(1));
    sql(&f.s, "PRAGMA query_only=ON");
    let failed = f.send(&f.master.token, "failed", "", &f.a.id, "Must not commit").unwrap_err();
    assert!(failed.message().contains("persist operation"), "expected persistence failure: {failed}");
    assert_eq!(before, memory(&f.s), "failed durable write changed memory");
    assert!(f.client_view(&f.master.id).seen > contact_before, "failed authenticated request did not update contact");
    assert_eq!(f.s.epoch(), epoch, "failed write notified");

    sql(&f.s, "PRAGMA query_only=OFF");
    f.send(&f.master.token, "durable", "", &f.a.id, "Durable event").unwrap();
    assert_ne!(f.s.epoch(), epoch, "durable mutation did not notify");

    // Closing the store wakes a blocked waiter.
    let seen = f.s.epoch();
    let woke = thread::scope(|scope| {
        let waiter = scope.spawn(|| f.s.wait_change(seen, Duration::from_secs(20)));
        thread::sleep(Duration::from_millis(100));
        f.s.close();
        waiter.join().unwrap()
    });
    assert!(woke, "close did not wake waiter");
}

#[test]
fn message_history_outlives_monitor_window() {
    let f = Fixture::new();
    // Populate the same durable stream without generating 1,001 network receipts.
    {
        let mut g = lock_inner(&f.s.inner);
        let mut d = Draft::from_inner(&g);
        for i in 0..EVENT_LIMIT + 5 {
            d.add_event(&f.a.id, "", "test", &i.to_string(), Utc::now());
        }
        g.save(d).unwrap();
    }
    assert_eq!(f.s.snapshot().unwrap().events.len(), EVENT_LIMIT, "monitor not bounded");

    let mut all: Vec<Message> = Vec::new();
    let mut after = 0;
    loop {
        let page = f.poll(&f.a.token, after);
        all.extend(page.messages);
        if !page.has_more {
            break;
        }
        assert!(page.next_cursor > after, "cursor did not advance");
        after = page.next_cursor;
    }
    assert_eq!(all.len(), EVENT_LIMIT + 5, "paged durable messages lost");
    assert_eq!(all[0].body, "0");
    assert_eq!(all[all.len() - 1].body, (EVENT_LIMIT + 4).to_string());
    let tail = f.poll(&f.a.token, all[all.len() - 2].seq);
    assert_eq!(tail.messages.len(), 1, "cursor resume failed");
}

#[test]
fn lock_and_corrupt_state() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("data");
    let s = Store::open(&dir).unwrap();
    assert!(Store::open(&dir).is_err(), "same data directory opened twice");
    s.close();

    let db_path = dir.join(STATE_DB_NAME);
    std::fs::write(&db_path, b"{corrupt").unwrap();
    assert!(Store::open(&dir).is_err(), "corruption became empty store");
    assert_eq!(std::fs::read(&db_path).unwrap(), b"{corrupt", "corrupt state file was overwritten");
}

#[test]
fn history_persists_across_restart() {
    let mut f = Fixture::new();
    let ws = f.workspace();
    let mut task = f.create(&ws, "write", &f.a.id);
    task = f.call_task(&f.a.token, task_request("claim", &task));
    let mut r = task_request("submit", &task);
    r.body = "Original evidence".into();
    r.snapshot = "snapshot-1".into();
    task = f.call_task(&f.a.token, r);
    let mut r = task_request("review", &task);
    r.body = "Needs correction".into();
    r.decision = "changes_requested".into();
    r.snapshot = task.snapshot.clone();
    f.call(&f.master.token, r);

    f.reopen();
    let entries = history(&f.s);
    let found_submit = entries.iter().any(|h| h.request.op == "submit" && h.request.body == "Original evidence");
    let found_review = entries.iter().any(|h| h.request.op == "review" && h.request.body == "Needs correction");
    assert!(found_submit && found_review, "revision evidence was overwritten");
}

#[test]
fn read_contact_is_visible_but_does_not_rewrite_durable_state() {
    let f = Fixture::new();
    let before = lock_inner(&f.s.inner).clients[&f.a.id].seen;
    let later = before + ChronoDuration::seconds(90);
    f.s.set_clock(move || later);
    f.poll(&f.a.token, -1);
    assert_eq!(f.client_view(&f.a.id).seen, later, "read contact not visible to console");
    assert_eq!(lock_inner(&f.s.inner).clients[&f.a.id].seen, before, "read contact rewrote durable client state");
    let state = f.call(&f.master.token, req("status")).into_status().unwrap();
    assert_eq!(state.clients[&f.a.id].seen, later, "status disagrees with console contact");
}

#[test]
fn new_state_db_leaves_unrelated_files_untouched() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("data");
    std::fs::create_dir_all(&dir).unwrap();
    let old = dir.join("state.json");
    std::fs::write(&old, b"old state is not read").unwrap();

    let s = Store::open(&dir).unwrap();
    assert!(dir.join(STATE_DB_NAME).is_file(), "SQLite database missing");
    assert_eq!(std::fs::read(&old).unwrap(), b"old state is not read", "old JSON changed");
    let view = s.snapshot().unwrap();
    assert!(view.tasks.is_empty() && view.clients.is_empty(), "new instance imported old state");
    let keys = s.join_keys();
    s.close();

    let s = Store::open(&dir).unwrap();
    assert_eq!(keys, s.join_keys(), "join identity changed across restart");
}

#[test]
fn tick_durability_and_wake() {
    let clock = TestClock::new(Utc::now());
    let f = Fixture::with_clock(Some(&clock));
    let ws = f.workspace();
    let mut task = f.create(&ws, "write", &f.a.id);
    task = f.call_task(&f.a.token, task_request("claim", &task));
    let epoch = f.s.epoch();
    f.s.tick().unwrap();
    assert_eq!(f.s.epoch(), epoch, "idle tick notified");

    clock.advance(LEASE_SECONDS + 1);
    // A monitor read must not silently mutate durable state.
    assert_eq!(f.s.snapshot().unwrap().tasks[&task.id].state, "running", "snapshot persisted expiry");
    sql(&f.s, "PRAGMA query_only=ON");
    assert!(f.s.tick().is_err(), "failed expiry save looked successful");
    assert_eq!(f.s.snapshot().unwrap().tasks[&task.id].state, "running", "failed tick changed memory");
    assert_eq!(f.s.epoch(), epoch, "failed tick notified");

    sql(&f.s, "PRAGMA query_only=OFF");
    f.s.tick().unwrap();
    assert_ne!(f.s.epoch(), epoch, "persisted expiry did not wake waiters");
    task = f.s.snapshot().unwrap().tasks[&task.id].clone();
    assert!(task.state == "unconfirmed" && task.worker_id == f.a.id, "expiry did not retain ownership");
}

// ---- round 2: simplification proposals 1 and 3 ----

fn progress_request(t: &Task, description: &str) -> Request {
    Request { percent: 10, description: description.into(), ..task_request("progress", t) }
}

#[test]
fn owner_progress_renews_the_lease() {
    let clock = TestClock::new(Utc::now());
    let f = Fixture::with_clock(Some(&clock));
    let mut task = f.create(&f.workspace(), "write", &f.a.id);
    task = f.call_task(&f.a.token, task_request("claim", &task));
    let claimed_lease = task.lease_until;

    clock.advance(100);
    task = f.call_task(&f.a.token, progress_request(&task, "halfway"));
    assert_eq!(task.lease_until, claimed_lease + ChronoDuration::seconds(100), "progress did not renew: {task:?}");
    clock.advance(100);
    f.call(&f.master.token, req("status"));
    let task = f.s.snapshot().unwrap().tasks[&task.id].clone();
    assert_eq!(task.state, "running", "200 s after claim and 100 s after progress: {task:?}");

    // Companion: the same claim without progress expires.
    let clock = TestClock::new(Utc::now());
    let f = Fixture::with_clock(Some(&clock));
    let mut task = f.create(&f.workspace(), "write", &f.a.id);
    task = f.call_task(&f.a.token, task_request("claim", &task));
    clock.advance(LEASE_SECONDS + 1);
    f.call(&f.master.token, req("status"));
    let task = f.s.snapshot().unwrap().tasks[&task.id].clone();
    assert_eq!(task.state, "unconfirmed", "{task:?}");
}

#[test]
fn claim_may_omit_version_and_generation() {
    let unfenced = |t: &Task| Request { version: None, generation: None, ..task_request("claim", t) };
    let f = Fixture::new();
    let ws = f.workspace();
    let queued = f.create(&ws, "write", &f.a.id);
    let task = f.call_task(&f.a.token, unfenced(&queued));
    assert_eq!(task.state, "running");
    assert_eq!((task.version, task.generation), (queued.version + 1, queued.generation + 1), "{task:?}");
    f.fail(&f.a.token, unfenced(&task), "task is not queued");

    // The other claim checks still apply.
    let assigned = f.create(&f.workspace(), "write", &f.a.id);
    f.fail(&f.b.token, unfenced(&assigned), "assigned to another worker");
    let overlapping = f.create(&ws, "write", "");
    f.fail(&f.b.token, unfenced(&overlapping), "workspace is still owned");

    // A value that is present is still checked; 0 is a value, not "absent".
    let other = f.create(&f.workspace(), "write", &f.a.id);
    let mut stale = task_request("claim", &other);
    stale.version = Some(other.version - 1);
    f.fail(&f.a.token, stale, "task version conflict");
    let mut stale = task_request("claim", &other);
    stale.generation = Some(other.generation + 1);
    f.fail(&f.a.token, stale, "stale claim generation");

    // Every other operation still needs both numbers.
    let mut r = progress_request(&task, "no version");
    r.version = None;
    f.fail(&f.a.token, r, "task version conflict");
    let mut r = progress_request(&task, "no generation");
    r.generation = None;
    f.fail(&f.a.token, r, "stale claim generation");
    let mut r = task_request("heartbeat", &task);
    r.version = None;
    f.fail(&f.a.token, r, "task version conflict");
    f.call_task(&f.a.token, progress_request(&task, "with both numbers"));
}

// ---- message_store_test.go ----

/// The Go test pins 2026-09-24 12:00 UTC. A restart re-prunes with the real clock, so a
/// pinned date turns into a time bomb once 24 h have passed; start from the real time instead.
fn message_clock() -> TestClock {
    TestClock::new(Utc::now())
}

#[test]
fn directed_broadcast_history_and_ack() {
    let clock = message_clock();
    let f = Fixture::with_clock(Some(&clock));
    let m = f.send(&f.master.token, "direct", "", &f.a.id, "hello a").unwrap();
    assert!(!m.id.is_empty() && m.kind == "chat" && m.from_id == f.master.id && m.to_id == f.a.id, "{m:?}");
    let again = f.send(&f.master.token, "direct", "", &f.a.id, "hello a").unwrap();
    assert_eq!(again, m, "idempotent send changed message");
    assert!(f.send(&f.master.token, "direct", "", &f.a.id, "changed").is_err(), "request ID accepted changed payload");

    let p = f.poll(&f.b.token, -1);
    assert!(p.messages.is_empty() && !p.has_new, "wrong recipient got directed chat: {p:?}");
    let p = f.poll(&f.a.token, -1);
    assert!(p.messages.len() == 1 && p.messages[0].seq == m.seq && p.has_new && p.ack_cursor == 0, "{p:?}");
    assert!(!is_zero_time(&f.client_view(&f.a.id).poll_at), "poll not shown as contact");
    assert!(!is_zero_time(&f.client_view(&f.b.id).poll_at), "other worker poll not shown as contact");
    assert!(is_zero_time(&f.client_view(&f.master.id).poll_at), "send marked master as polling");

    assert!(f.s.ack_messages(&f.a.token, 0, m.seq + 1).is_err(), "ack accepted undelivered sequence");
    let ack = f.s.ack_messages(&f.a.token, 0, m.seq).unwrap();
    assert_eq!(ack.ack_cursor, m.seq);
    assert!(f.s.ack_messages(&f.a.token, 0, m.seq).is_err(), "stale expected cursor accepted");
    let p = f.poll(&f.a.token, -1);
    assert!(!p.has_new && p.messages.is_empty() && p.ack_cursor == m.seq, "acked message remained new: {p:?}");

    let reply = f.send(&f.a.token, "reply", "", "", "reply to master").unwrap();
    assert_eq!(reply.to_id, f.master.id, "worker did not address master");
    let p = f.poll(&f.master.token, -1);
    assert!(p.messages.len() == 1 && p.messages[0].id == reply.id, "master missed reply: {p:?}");

    let broadcast = f.send(&f.master.token, "broadcast", "", "", "all workers").unwrap();
    assert!(broadcast.to_id.is_empty(), "broadcast became directed");
    let p = f.poll(&f.b.token, -1);
    assert!(p.messages.len() == 1 && p.messages[0].id == broadcast.id, "worker b missed broadcast: {p:?}");

    let (_, worker_key) = f.s.join_keys();
    let late = f.join(&worker_key, "worker", "late");
    assert!(f.poll(&late.token, -1).messages.is_empty(), "late join received old broadcast");

    let history = f.s.message_history(&f.master.token, "", &f.a.id, 100).unwrap();
    assert!(history.len() >= 2, "sender cannot read directed history: {history:?}");
}

#[test]
fn task_privacy_progress_and_restart() {
    let clock = message_clock();
    let mut f = Fixture::with_clock(Some(&clock));
    let ws = f.workspace();
    let create = Request {
        op: "create".into(),
        title: "test".into(),
        workspace: ws,
        mode: "read".into(),
        worker_id: f.a.id.clone(),
        body: "work".into(),
        ..Request::default()
    };
    let mut task = f.call_task(&f.master.token, create);
    task = f.call_task(&f.a.token, task_request("claim", &task));
    assert!(task.progress_percent == 0 && task.progress_note == WIRE_CLAIMED, "claim did not initialize progress");

    assert!(f.send(&f.b.token, "intrude", &task.id, "", "wrong worker").is_err(), "other worker entered task chat");
    assert!(
        f.send(&f.master.token, "wrong-target", &task.id, &f.b.id, "wrong target").is_err(),
        "master addressed wrong task worker"
    );
    let chat = f.send(&f.a.token, "task-chat", &task.id, "", "status").unwrap();
    assert!(chat.to_id == f.master.id && chat.task_id == task.id, "bad task chat: {chat:?}");

    let progress = |percent: i64, description: &str, t: &Task| Request {
        op: "progress".into(),
        task_id: t.id.clone(),
        version: Some(t.version),
        generation: Some(t.generation),
        percent,
        description: description.into(),
        ..Request::default()
    };
    let first = progress(25, "first step", &task);
    task = f.call_task(&f.a.token, first.clone());
    assert!(task.progress_percent == 25 && task.subtasks.is_empty(), "progress missing: {task:?}");
    f.fail(&f.a.token, Request { percent: 50, description: "stale".into(), ..first }, "version conflict");
    f.fail(&f.b.token, progress(50, "other", &task), "another worker");

    let mut sub = progress(50, "design", &task);
    sub.subtask_id = "s1".into();
    sub.subtask_title = "design".into();
    task = f.call_task(&f.a.token, sub);
    assert!(
        task.progress_percent == 25 && task.subtasks.len() == 1 && task.subtasks[0].percent == 50,
        "subtask changed overall progress: {task:?}"
    );
    let mut update = progress(60, "design updated", &task);
    update.subtask_id = "s1".into();
    task = f.call_task(&f.a.token, update);
    assert!(
        task.subtasks.len() == 1 && task.subtasks[0].description == "design updated",
        "subtask duplicated: {task:?}"
    );
    f.fail(&f.a.token, progress(101, "invalid", &task), "percent must be");
    let mut untitled = progress(65, "new subtask", &task);
    untitled.subtask_id = "s2".into();
    f.fail(&f.a.token, untitled, "requires a title");

    // Submission freezes the task: progress must not resume a submitted task.
    let mut submit = task_request("submit", &task);
    submit.body = "done".into();
    submit.snapshot = "test-snapshot".into();
    task = f.call_task(&f.a.token, submit);
    f.fail(&f.a.token, progress(100, "too late", &task), "not running");

    f.reopen();
    clock.install(&f.s);
    let restored = f.s.snapshot().unwrap().tasks[&task.id].clone();
    assert!(restored.progress_percent == 25 && restored.subtasks.len() == 1, "progress lost across restart");
    let history = f.s.message_history(&f.master.token, &task.id, "", 100).unwrap();
    assert!(!history.is_empty(), "task history missing");
    assert_eq!(history[history.len() - 1].kind, "task_event", "task_event missing: {history:?}");
}

#[test]
fn progress_resets_at_rework_and_requeue() {
    let clock = message_clock();
    let f = Fixture::with_clock(Some(&clock));
    let create = Request {
        op: "create".into(),
        title: "reset".into(),
        workspace: f.workspace(),
        mode: "read".into(),
        worker_id: f.a.id.clone(),
        body: "initial work".into(),
        ..Request::default()
    };
    let mut task = f.call_task(&f.master.token, create);
    task = f.call_task(&f.a.token, task_request("claim", &task));

    let advance = |t: &Task, description: &str, percent: i64, subtask: &str| -> Task {
        let mut r = task_request("progress", t);
        r.percent = percent;
        r.description = description.into();
        r.subtask_id = subtask.into();
        if !subtask.is_empty() {
            r.subtask_title = "one".into();
        }
        f.call_task(&f.a.token, r)
    };
    task = advance(&task, "full", 100, "");
    task = advance(&task, "subtask-full", 100, "subtask");
    assert!(task.progress_percent == 100 && task.subtasks.len() == 1, "test did not establish old progress");

    let mut submit = task_request("submit", &task);
    submit.body = "done".into();
    submit.snapshot = "snapshot".into();
    task = f.call_task(&f.a.token, submit);
    let mut review = task_request("review", &task);
    review.decision = "changes_requested".into();
    review.snapshot = "snapshot".into();
    review.body = "retry".into();
    task = f.call_task(&f.master.token, review);
    assert!(
        task.state == "running"
            && task.progress_percent == 0
            && task.progress_note == WIRE_REWORK
            && !is_zero_time(&task.progress_at)
            && task.subtasks.is_empty(),
        "rework retained prior attempt progress: {task:?}"
    );

    task = advance(&task, "new-attempt", 40, "");
    let mut stop = task_request("confirm-stop", &task);
    stop.evidence = "worker stopped".into();
    task = f.call_task(&f.master.token, stop);
    assert!(
        task.state == "queued"
            && task.progress_percent == 0
            && task.progress_note.is_empty()
            && is_zero_time(&task.progress_at)
            && task.subtasks.is_empty(),
        "requeue retained prior owner progress: {task:?}"
    );

    task = f.call_task(&f.b.token, task_request("claim", &task));
    assert!(
        task.progress_percent == 0 && task.progress_note == WIRE_CLAIMED && task.subtasks.is_empty(),
        "new owner inherited progress: {task:?}"
    );
}

#[test]
fn message_expiry_gap_and_receipt_cleanup() {
    let clock = message_clock();
    let f = Fixture::with_clock(Some(&clock));
    let m = f.send(&f.master.token, "ephemeral", "", &f.a.id, "old-secret-body").unwrap();
    clock.advance(MESSAGE_LIFETIME_SECONDS + 1);
    f.s.tick().unwrap();

    let p = f.poll(&f.a.token, -1);
    assert!(
        p.messages.is_empty() && p.retention_gap && p.expired_through_seq == m.seq,
        "expiry did not report gap: {p:?}"
    );
    let resumed = f.poll(&f.a.token, p.expired_through_seq);
    assert!(!resumed.retention_gap, "cannot resume after undelivered expired waterline: {resumed:?}");
    f.s.ack_messages(&f.a.token, 0, p.expired_through_seq).expect("acknowledge expired gap");
    assert!(!f.poll(&f.a.token, -1).retention_gap, "gap remained after explicit ack");

    assert_eq!(sql_count(&f.s, "SELECT COUNT(*) FROM messages WHERE body LIKE '%old-secret-body%'"), 0);
    assert_eq!(sql_count(&f.s, "SELECT COUNT(*) FROM events WHERE CAST(data AS TEXT) LIKE '%old-secret-body%'"), 0);
    assert_eq!(sql_count(&f.s, "SELECT COUNT(*) FROM message_receipts WHERE request_id='ephemeral'"), 0);
}

// ---- clear_tasks_test.go ----

#[test]
fn clear_tasks_archives_exact_workspace_and_allows_new_work() {
    let mut f = Fixture::new();
    let workspace = f.workspace();
    let other_workspace = f.workspace();
    let queued_create = f.request(Request {
        op: "create".into(),
        task_id: "old-queued".into(),
        title: "queued".into(),
        workspace: workspace.clone(),
        mode: "write".into(),
        worker_id: f.a.id.clone(),
        body: "old work".into(),
        ..Request::default()
    });
    let queued = f.call_task(&f.master.token, queued_create.clone());
    let mut awaiting = f.create(&workspace, "write", &f.a.id);
    let old_claim = f.request(task_request("claim", &awaiting));
    awaiting = f.call_task(&f.a.token, old_claim.clone());
    let mut submit = task_request("submit", &awaiting);
    submit.body = "ready for review".into();
    submit.snapshot = "old:dev/working:hash".into();
    awaiting = f.call_task(&f.a.token, submit);
    let other = f.create(&other_workspace, "write", &f.b.id);

    let clear = f.request(Request {
        op: "clear-tasks".into(),
        workspace: workspace.clone(),
        expected_count: 2,
        body: "replace obsolete task queue".into(),
        ..Request::default()
    });
    f.fail(&f.a.token, clear.clone(), "master role required");
    let wrong_count = Request { expected_count: 1, request_id: String::new(), ..clear.clone() };
    f.fail(&f.master.token, wrong_count, "task count changed");

    let got = f.call(&f.master.token, clear.clone()).into_cleared().unwrap();
    let mut want = vec![awaiting.id.clone(), queued.id.clone()];
    want.sort();
    assert!(got.count == 2 && got.task_ids == want, "wrong clear result: {got:?}");

    let visible = f.call(&f.master.token, req("tasks")).into_tasks().unwrap();
    assert!(visible.len() == 1 && visible[0].id == other.id, "cleared tasks still visible: {visible:?}");
    assert!(f.call(&f.a.token, req("tasks")).into_tasks().unwrap().is_empty(), "worker still sees cleared tasks");

    let worker_poll = f.poll(&f.a.token, -1);
    let notices: Vec<&Message> = worker_poll.messages.iter().filter(|m| m.event_kind == "tasks_cleared").collect();
    assert!(!notices.is_empty(), "affected worker did not receive clear notice");
    for notice in notices {
        assert!(
            !notice.body.contains(&workspace) && !notice.body.contains(&clear.body),
            "worker notice leaked master reason or workspace: {notice:?}"
        );
    }
    let unaffected = f.poll(&f.b.token, -1);
    assert!(unaffected.messages.iter().all(|m| m.event_kind != "tasks_cleared"), "unaffected worker received notice");

    f.s.execute(&f.master.token, &clear).expect("same clear retry must be idempotent");
    let changed = Request { body: "changed".into(), ..clear.clone() };
    f.fail(&f.master.token, changed, "different payload");
    f.fail(&f.master.token, queued_create, "recorded task was cleared");
    f.fail(&f.a.token, old_claim, "recorded task was cleared");
    let reuse = Request {
        op: "create".into(),
        task_id: queued.id.clone(),
        title: "reuse".into(),
        workspace: workspace.clone(),
        mode: "read".into(),
        ..Request::default()
    };
    f.fail(&f.master.token, reuse, "already exists");
    let archived = f.s.raw_task(&awaiting.id).unwrap();
    f.fail(&f.master.token, task_request("confirm-stop", &archived), "no execution ownership");
    let chat = f.send(&f.master.token, "old-chat", &awaiting.id, "", "do not resume").unwrap_err();
    assert!(chat.message().contains("cleared"), "cleared task chat should fail: {chat}");

    let new_task = f.create(&workspace, "write", &f.a.id);
    f.call(&f.a.token, task_request("claim", &new_task));

    f.reopen();
    assert!(!f.s.snapshot().unwrap().tasks.contains_key(&awaiting.id), "cleared task reappeared after restart");
    let tombstone = f.s.raw_task(&awaiting.id).unwrap();
    assert!(
        tombstone.state == "cleared" && tombstone.snapshot == "old:dev/working:hash",
        "audit tombstone lost after restart: {tombstone:?}"
    );
}

#[test]
fn clear_tasks_rejects_active_work_atomically() {
    let clock = TestClock::new(Utc::now());
    let f = Fixture::with_clock(Some(&clock));
    let ws = f.workspace();
    let queued = f.create(&ws, "read", &f.a.id);
    let mut running = f.create(&ws, "write", &f.b.id);
    running = f.call_task(&f.b.token, task_request("claim", &running));
    let request = Request {
        op: "clear-tasks".into(),
        workspace: ws,
        expected_count: 2,
        body: "replace tasks".into(),
        ..Request::default()
    };
    f.fail(&f.master.token, request.clone(), "still running");
    assert_eq!(f.s.raw_task(&queued.id).unwrap().state, "queued", "partial clear changed queued task");

    let expiry = running.lease_until + ChronoDuration::seconds(1);
    f.s.set_clock(move || expiry);
    f.fail(&f.master.token, request, "still unconfirmed");
    assert_eq!(f.s.raw_task(&queued.id).unwrap().state, "queued", "partial clear changed queued task after expiry");
}

#[test]
fn wire_notes_are_the_go_service_values() {
    // The Go service stores these two Chinese notes in `progress_note`; clients may match on them.
    assert_eq!(NOTE_CLAIMED, WIRE_CLAIMED);
    assert_eq!(NOTE_REWORK, WIRE_REWORK);
}
