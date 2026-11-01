//! Durable Host delivery and execution bindings. Business Tasks remain in ara-lite.
use std::{
    fs::{File, OpenOptions},
    path::Path,
    sync::Mutex,
};

use anyhow::{ensure, Context, Result};
use fs2::FileExt;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{de::DeserializeOwned, Serialize};
use sha2::{Digest, Sha256};

use crate::types::*;

const MAX_EVENTS: usize = 100_000;
const MAX_BINDINGS: usize = 10_000;
const MAX_ATTENTIONS: usize = 100_000;
const MAX_NOTICES: usize = 100_000;
const MAX_SOURCES: usize = 10_000;
const MAX_BODY: usize = 64 * 1024;

pub struct Journal {
    connection: Mutex<Connection>,
    _lock: File,
}

fn id(value: &str) -> Result<()> {
    ensure!(!value.trim().is_empty() && value.len() <= 256, "identifier must contain 1..256 UTF-8 bytes");
    Ok(())
}

fn body(value: &str) -> Result<()> {
    ensure!(!value.trim().is_empty() && value.len() <= MAX_BODY, "text must contain 1..65536 UTF-8 bytes");
    Ok(())
}

fn read<T: DeserializeOwned>(db: &Connection, table: &str, key: &str) -> Result<Option<T>> {
    let value: Option<String> =
        db.query_row(&format!("SELECT data FROM {table} WHERE id=?1"), [key], |r| r.get(0)).optional()?;
    value.map(|value| serde_json::from_str(&value).context("invalid persisted Host record")).transpose()
}

fn list<T: DeserializeOwned>(db: &Connection, table: &str) -> Result<Vec<T>> {
    let mut statement = db.prepare(&format!("SELECT data FROM {table} ORDER BY rowid"))?;
    let rows = statement.query_map([], |r| r.get::<_, String>(0))?;
    rows.map(|row| serde_json::from_str(&row?).context("invalid persisted Host record")).collect()
}

fn list_state<T: DeserializeOwned>(db: &Connection, table: &str, state: &impl Serialize) -> Result<Vec<T>> {
    let state = serde_json::to_value(state)?;
    let state = state.as_str().context("Host state must serialize as text")?;
    let mut statement = db.prepare(&format!("SELECT data FROM {table} WHERE state=?1 ORDER BY rowid"))?;
    let rows = statement.query_map([state], |r| r.get::<_, String>(0))?;
    rows.map(|row| serde_json::from_str(&row?).context("invalid persisted Host record")).collect()
}

fn write<T: Serialize>(db: &Connection, table: &str, key: &str, value: &T) -> Result<()> {
    if table == "events" {
        ensure!(
            db.execute("UPDATE events SET data=?2 WHERE id=?1", params![key, serde_json::to_string(value)?])? == 1,
            "event disappeared during update"
        );
        return Ok(());
    }
    db.execute(
        &format!("INSERT INTO {table}(id,data) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET data=excluded.data"),
        params![key, serde_json::to_string(value)?],
    )?;
    Ok(())
}

fn capacity(db: &Connection, table: &str, max: usize) -> Result<()> {
    let count: i64 = db.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0))?;
    ensure!(count < max as i64, "{table} capacity reached; retained identities cannot be evicted automatically");
    Ok(())
}

impl Journal {
    pub fn open(path: &Path) -> Result<Self> {
        ensure!(path.is_absolute(), "absolute Host journal path required");
        ensure!(path.parent().is_some_and(Path::is_dir), "Host journal parent must exist");
        let lock =
            OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path.with_extension("lock"))?;
        lock.try_lock_exclusive().context("another Host owns this journal")?;
        let mut connection = Connection::open(path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.execute_batch("CREATE TABLE IF NOT EXISTS events(id TEXT PRIMARY KEY, source TEXT NOT NULL, event_id TEXT NOT NULL, digest TEXT NOT NULL, data TEXT NOT NULL, state TEXT GENERATED ALWAYS AS (json_extract(data,'$.state')) VIRTUAL, UNIQUE(source,event_id));
            CREATE INDEX IF NOT EXISTS events_state ON events(state);
            CREATE TABLE IF NOT EXISTS bindings(id TEXT PRIMARY KEY,data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS attentions(id TEXT PRIMARY KEY,data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS notices(id TEXT PRIMARY KEY,data TEXT NOT NULL,state TEXT GENERATED ALWAYS AS (json_extract(data,'$.state')) VIRTUAL);
            CREATE INDEX IF NOT EXISTS notices_state ON notices(state);
            CREATE TABLE IF NOT EXISTS notice_settlements(id TEXT PRIMARY KEY,evidence TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS sources(id TEXT PRIMARY KEY,data TEXT NOT NULL);")?;
        let tx = connection.transaction()?;
        for mut event in list_state::<EventRecord>(&tx, "events", &DeliveryState::Dispatching)? {
            if event.state == DeliveryState::Dispatching {
                event.state = DeliveryState::Unknown;
                event.detail =
                    "Host restarted while delivery was dispatching; inspect original context and effects before retry"
                        .into();
                write(&tx, "events", &event.key, &event)?;
            }
        }
        for mut notice in list_state::<Notice>(&tx, "notices", &NoticeState::Dispatching)? {
            if notice.state == NoticeState::Dispatching {
                notice.state = NoticeState::Unknown;
                write(&tx, "notices", &notice.id, &notice)?;
            }
        }
        for mut binding in list::<ExecutionBinding>(&tx, "bindings")? {
            if matches!(
                binding.phase,
                ExecutionPhase::Starting
                    | ExecutionPhase::Running
                    | ExecutionPhase::WaitingHuman
                    | ExecutionPhase::ReadyForReview
            ) {
                binding.phase = ExecutionPhase::NeedsReconciliation;
                write(&tx, "bindings", &binding.spec.task_id, &binding)?;
            }
        }
        tx.commit()?;
        Ok(Self { connection: Mutex::new(connection), _lock: lock })
    }

    fn transaction<T>(&self, operation: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        let mut db = self.connection.lock().map_err(|_| anyhow::anyhow!("Host journal lock poisoned"))?;
        let tx = db.transaction()?;
        let result = operation(&tx)?;
        tx.commit()?;
        Ok(result)
    }

    pub fn ingest(&self, input: &InputEvent) -> Result<(EventRecord, bool)> {
        for value in [&input.source, &input.event_id, &input.actor, &input.authorization_revision] {
            id(value)?;
        }
        ensure!(serde_json::to_vec(&input.body)?.len() <= MAX_BODY, "event body exceeds 64 KiB");
        match &input.body {
            EventBody::Work { title, workspace, objective } => {
                body(title)?;
                body(objective)?;
                ensure!(workspace.is_absolute(), "absolute work event workspace required");
            }
            EventBody::Followup { task_id, generation, text } => {
                id(task_id)?;
                ensure!(*generation > 0, "positive generation required");
                body(text)?;
            }
            EventBody::Answer { task_id, generation, question_id, text } => {
                id(task_id)?;
                id(question_id)?;
                ensure!(*generation > 0, "positive generation required");
                body(text)?;
            }
            EventBody::Cancel { task_id, generation } => {
                id(task_id)?;
                ensure!(*generation > 0, "positive generation required");
            }
        }
        let bytes = serde_json::to_vec(input)?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let key = format!("{:x}", Sha256::digest(serde_json::to_vec(&(&input.source, &input.event_id))?));
        self.transaction(|tx| {
            let previous: Option<String> =
                tx.query_row("SELECT digest FROM events WHERE id=?1", [&key], |r| r.get(0)).optional()?;
            if let Some(previous) = previous {
                ensure!(previous == digest, "source event ID reused with different input");
                return Ok((read(tx, "events", &key)?.context("event digest without original record")?, false));
            }
            capacity(tx, "events", MAX_EVENTS)?;
            let record = EventRecord {
                key: key.clone(),
                input: input.clone(),
                task_id: None,
                generation: None,
                state: DeliveryState::Pending,
                detail: String::new(),
            };
            tx.execute(
                "INSERT INTO events(id,source,event_id,digest,data) VALUES(?1,?2,?3,?4,?5)",
                params![key, input.source, input.event_id, digest, serde_json::to_string(&record)?],
            )?;
            Ok((record, true))
        })
    }

    pub fn event(&self, key: &str) -> Result<Option<EventRecord>> {
        self.transaction(|tx| read(tx, "events", key))
    }

    pub fn events(&self, state: DeliveryState) -> Result<Vec<EventRecord>> {
        self.transaction(|tx| list_state(tx, "events", &state))
    }

    pub fn bind_event(&self, key: &str, task_id: &str, generation: i64) -> Result<()> {
        id(task_id)?;
        ensure!(generation > 0, "positive generation required");
        self.transaction(|tx| {
            let mut event: EventRecord = read(tx, "events", key)?.context("unknown event")?;
            if let Some(previous) = &event.task_id {
                ensure!(
                    previous == task_id && event.generation == Some(generation),
                    "event is already bound to another Task generation"
                );
            }
            match &event.input.body {
                EventBody::Followup { task_id: target, generation: target_generation, .. }
                | EventBody::Answer { task_id: target, generation: target_generation, .. }
                | EventBody::Cancel { task_id: target, generation: target_generation } => {
                    ensure!(target == task_id && *target_generation == generation, "event target and binding differ");
                }
                EventBody::Work { .. } => {}
            }
            event.task_id = Some(task_id.into());
            event.generation = Some(generation);
            write(tx, "events", key, &event)
        })
    }

    pub fn mark_event(&self, key: &str, state: DeliveryState, detail: &str) -> Result<()> {
        ensure!(detail.len() <= MAX_BODY, "event detail exceeds 64 KiB");
        self.transaction(|tx| {
            let mut event: EventRecord = read(tx, "events", key)?.context("unknown event")?;
            let allowed = event.state == state
                || match event.state {
                    DeliveryState::Pending => state != DeliveryState::Pending,
                    DeliveryState::Dispatching => matches!(
                        state,
                        DeliveryState::Applied
                            | DeliveryState::Unapplied
                            | DeliveryState::Unknown
                            | DeliveryState::Rejected
                    ),
                    DeliveryState::Unknown => {
                        matches!(state, DeliveryState::Applied | DeliveryState::Unapplied | DeliveryState::Rejected)
                    }
                    _ => false,
                };
            ensure!(allowed, "event state cannot be reset or redispatched");
            event.state = state;
            event.detail = detail.into();
            write(tx, "events", key, &event)
        })
    }

    pub fn put_binding(&self, binding: &ExecutionBinding) -> Result<()> {
        self.transaction(|tx| Self::put_binding_record(tx, binding))
    }

    fn put_binding_record(tx: &Connection, binding: &ExecutionBinding) -> Result<()> {
        for value in [
            &binding.spec.task_id,
            &binding.spec.criteria_revision,
            &binding.spec.authorization_revision,
            &binding.adapter_revision,
            &binding.initial_run_id,
        ] {
            id(value)?;
        }
        ensure!(
            binding.generation > 0
                && binding.spec.max_calls > 0
                && binding.spec.max_runs > 0
                && binding.spec.deadline_unix_ms > 0,
            "positive binding terms required"
        );
        ensure!(
            [&binding.spec.evidence_dir, &binding.session_path, &binding.environment_path, &binding.workspace]
                .iter()
                .all(|p| p.is_absolute()),
            "absolute binding paths required"
        );
        if let Some(previous) = read::<ExecutionBinding>(tx, "bindings", &binding.spec.task_id)? {
            let old = &previous.spec;
            let new = &binding.spec;
            ensure!(
                old.task_id == new.task_id
                    && old.criteria_revision == new.criteria_revision
                    && old.authorization_revision == new.authorization_revision
                    && old.kind == new.kind
                    && old.max_runs == new.max_runs
                    && old.max_calls == new.max_calls
                    && old.deadline_unix_ms == new.deadline_unix_ms
                    && previous.options == binding.options
                    && previous.session_path == binding.session_path
                    && previous.environment_path == binding.environment_path
                    && previous.workspace == binding.workspace
                    && previous.generation == binding.generation
                    && previous.adapter_revision == binding.adapter_revision
                    && previous.initial_run_id == binding.initial_run_id,
                "binding Task, Session, authority and budget terms are immutable"
            );
            ensure!(binding.attempt >= previous.attempt, "binding attempt cannot go backwards");
            ensure!(
                binding.spec.evidence_dir == previous.spec.evidence_dir || binding.attempt > previous.attempt,
                "new evidence directory requires a new explicit attempt"
            );
        } else {
            capacity(tx, "bindings", MAX_BINDINGS)?;
        }
        write(tx, "bindings", &binding.spec.task_id, binding)
    }

    pub fn binding(&self, task_id: &str) -> Result<Option<ExecutionBinding>> {
        self.transaction(|tx| read(tx, "bindings", task_id))
    }
    pub fn bindings(&self) -> Result<Vec<ExecutionBinding>> {
        self.transaction(|tx| list(tx, "bindings"))
    }

    pub fn put_attention(&self, attention: &Attention) -> Result<()> {
        id(&attention.id)?;
        id(&attention.task_id)?;
        body(&attention.question)?;
        ensure!(attention.generation > 0, "positive generation required");
        self.transaction(|tx| {
            if let Some(previous) = read::<Attention>(tx, "attentions", &attention.id)? {
                ensure!(
                    previous.task_id == attention.task_id
                        && previous.generation == attention.generation
                        && previous.question == attention.question,
                    "attention ID reused with different question or Task"
                );
                ensure!(
                    attention.answer.is_none() || attention.answer == previous.answer,
                    "attention answer must use answer()"
                );
                return Ok(());
            }
            ensure!(attention.answer.is_none(), "new attention cannot already contain an answer");
            capacity(tx, "attentions", MAX_ATTENTIONS)?;
            write(tx, "attentions", &attention.id, attention)
        })
    }

    pub fn attention(&self, attention_id: &str) -> Result<Option<Attention>> {
        self.transaction(|tx| read(tx, "attentions", attention_id))
    }

    pub fn answer(&self, attention_id: &str, task_id: &str, generation: i64, text: &str) -> Result<()> {
        body(text)?;
        self.transaction(|tx| {
            let mut attention: Attention = read(tx, "attentions", attention_id)?.context("unknown attention")?;
            ensure!(
                attention.task_id == task_id && attention.generation == generation,
                "answer targets another Task generation"
            );
            if let Some(previous) = &attention.answer {
                ensure!(previous == text, "question already has a different answer");
                return Ok(());
            }
            attention.answer = Some(text.into());
            write(tx, "attentions", attention_id, &attention)
        })
    }

    pub fn put_notice(&self, notice: &Notice) -> Result<()> {
        self.transaction(|tx| Self::put_notice_record(tx, notice))
    }

    fn put_notice_record(tx: &Connection, notice: &Notice) -> Result<()> {
        id(&notice.id)?;
        id(&notice.task_id)?;
        id(&notice.kind)?;
        body(&notice.body)?;
        ensure!(notice.generation > 0, "positive generation required");
        if let Some(previous) = read::<Notice>(tx, "notices", &notice.id)? {
            ensure!(
                previous.task_id == notice.task_id
                    && previous.generation == notice.generation
                    && previous.kind == notice.kind
                    && previous.body == notice.body,
                "notice ID reused with different content"
            );
            return Ok(());
        }
        ensure!(notice.state == NoticeState::Pending, "new notice must be pending");
        capacity(tx, "notices", MAX_NOTICES)?;
        write(tx, "notices", &notice.id, notice)
    }

    /// Publish a settled execution binding and its result notification together.
    /// A failed/conflicting notification rolls the binding update back as well.
    pub fn complete(&self, binding: &ExecutionBinding, notice: &Notice) -> Result<()> {
        ensure!(
            matches!(binding.phase, ExecutionPhase::Finished | ExecutionPhase::NeedsReconciliation),
            "completion must settle execution or require reconciliation"
        );
        ensure!(
            notice.kind == "result"
                && notice.task_id == binding.spec.task_id
                && notice.generation == binding.generation,
            "result notice must identify the completed Task generation"
        );
        self.transaction(|tx| {
            ensure!(
                read::<ExecutionBinding>(tx, "bindings", &binding.spec.task_id)?.is_some(),
                "cannot complete an unbound Task"
            );
            Self::put_binding_record(tx, binding)?;
            Self::put_notice_record(tx, notice)
        })
    }

    pub fn notices(&self, state: NoticeState) -> Result<Vec<Notice>> {
        self.transaction(|tx| list_state(tx, "notices", &state))
    }

    pub fn notice(&self, notice_id: &str) -> Result<Option<Notice>> {
        self.transaction(|tx| read(tx, "notices", notice_id))
    }

    pub fn mark_notice(&self, notice_id: &str, state: NoticeState) -> Result<()> {
        self.transaction(|tx| {
            let mut notice: Notice = read(tx, "notices", notice_id)?.context("unknown notice")?;
            ensure!(
                notice.state == state
                    || matches!(
                        (notice.state, state),
                        (NoticeState::Pending, NoticeState::Dispatching)
                            | (NoticeState::Dispatching, NoticeState::Sent)
                            | (NoticeState::Dispatching, NoticeState::Unknown)
                    ),
                "notice cannot be reset or resent automatically"
            );
            notice.state = state;
            write(tx, "notices", notice_id, &notice)
        })
    }

    /// Explicit operator reconciliation of an uncertain external delivery. A
    /// normal transport ACK must use mark_notice(Dispatching -> Sent) instead.
    pub fn settle_notice(&self, notice_id: &str, evidence: &str) -> Result<()> {
        body(evidence)?;
        self.transaction(|tx| {
            let mut notice: Notice = read(tx, "notices", notice_id)?.context("unknown notice")?;
            let previous: Option<String> = tx
                .query_row("SELECT evidence FROM notice_settlements WHERE id=?1", [notice_id], |row| row.get(0))
                .optional()?;
            if let Some(previous) = previous {
                ensure!(
                    previous == evidence && notice.state == NoticeState::Sent,
                    "notice settlement already has different evidence"
                );
                return Ok(());
            }
            ensure!(
                notice.state == NoticeState::Unknown,
                "only uncertain notice delivery requires explicit settlement"
            );
            tx.execute("INSERT INTO notice_settlements(id,evidence) VALUES(?1,?2)", params![notice_id, evidence])?;
            notice.state = NoticeState::Sent;
            write(tx, "notices", notice_id, &notice)
        })
    }

    pub fn put_source(&self, source: &EvidenceSource) -> Result<()> {
        id(&source.id)?;
        id(&source.task_id)?;
        ensure!(source.generation > 0 && source.version > 0, "positive source generation and version required");
        ensure!(source.path.is_absolute(), "absolute source path required");
        ensure!(
            source.sha256.len() == 64 && source.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid source SHA-256"
        );
        ensure!(source.allowed_tasks.len() <= 1024, "source task allowlist exceeds 1024 entries");
        for task in &source.allowed_tasks {
            id(task)?;
        }
        self.transaction(|tx| {
            if let Some(previous) = read::<EvidenceSource>(tx, "sources", &source.id)? {
                if previous == *source {
                    return Ok(());
                }
                ensure!(source.version > previous.version, "source update requires a newer explicit version");
                ensure!(
                    source.task_id == previous.task_id && source.generation == previous.generation,
                    "source owner is immutable"
                );
            } else {
                capacity(tx, "sources", MAX_SOURCES)?;
            }
            write(tx, "sources", &source.id, source)
        })
    }

    pub fn source(&self, source_id: &str) -> Result<Option<EvidenceSource>> {
        self.transaction(|tx| read(tx, "sources", source_id))
    }
    pub fn sources(&self) -> Result<Vec<EvidenceSource>> {
        self.transaction(|tx| list(tx, "sources"))
    }
}
