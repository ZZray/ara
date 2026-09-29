//! SQLite schema, open and load (Go: `store_sqlite.go`, `ensureMessageSchema`).
//!
//! The Rust schema does not have to match Go's `state.db`; it follows the same
//! idea: JSON blobs for clients/tasks/events/history/receipts plus relational
//! message tables.

use crate::error::{Error, Result};
use crate::types::{Client, Task};
use crate::util::random_hex;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

pub const STATE_DB_NAME: &str = "state.db";
const FORMAT: i64 = 1;

/// Durable state loaded at startup.
pub struct Loaded {
    pub clients: BTreeMap<String, Client>,
    pub tasks: BTreeMap<String, Task>,
    pub next_seq: i64,
    pub history_len: i64,
    pub master_join: String,
    pub worker_join: String,
}

pub fn open_sqlite(path: &Path) -> Result<Connection> {
    let db = Connection::open(path).map_err(|e| Error::new(format!("open {}: {e}", path.display())))?;
    db.busy_timeout(Duration::from_millis(5000))?;
    for (name, value) in [("journal_mode", "DELETE"), ("synchronous", "FULL")] {
        db.pragma_update(None, name, value).map_err(|e| Error::new(format!("open {}: {e}", path.display())))?;
    }
    Ok(db)
}

pub fn ensure_message_schema(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS messages (seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE, task_id TEXT NOT NULL, from_id TEXT NOT NULL, to_id TEXT NOT NULL, kind TEXT NOT NULL, event_kind TEXT NOT NULL, body TEXT NOT NULL, at_ns INTEGER NOT NULL, expires_ns INTEGER NOT NULL);
         CREATE INDEX IF NOT EXISTS messages_task_seq ON messages(task_id, seq);
         CREATE INDEX IF NOT EXISTS messages_expiry ON messages(expires_ns);
         CREATE TABLE IF NOT EXISTS message_deliveries (message_seq INTEGER NOT NULL, recipient_id TEXT NOT NULL, PRIMARY KEY(message_seq, recipient_id));
         CREATE INDEX IF NOT EXISTS message_deliveries_recipient_seq ON message_deliveries(recipient_id, message_seq);
         CREATE TABLE IF NOT EXISTS message_cursors (client_id TEXT PRIMARY KEY, ack_seq INTEGER NOT NULL DEFAULT 0, delivered_seq INTEGER NOT NULL DEFAULT 0, expired_through_seq INTEGER NOT NULL DEFAULT 0);
         CREATE TABLE IF NOT EXISTS message_receipts (client_id TEXT NOT NULL, request_id TEXT NOT NULL, payload_digest BLOB NOT NULL, message_seq INTEGER NOT NULL, expires_ns INTEGER NOT NULL, PRIMARY KEY(client_id,request_id));
         CREATE INDEX IF NOT EXISTS message_receipts_expiry ON message_receipts(expires_ns);",
    )?;
    Ok(())
}

fn create_schema(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE clients (id TEXT PRIMARY KEY, data BLOB NOT NULL);
         CREATE TABLE tasks (id TEXT PRIMARY KEY, data BLOB NOT NULL);
         CREATE TABLE events (seq INTEGER PRIMARY KEY, to_id TEXT NOT NULL, task_id TEXT NOT NULL, data BLOB NOT NULL);
         CREATE INDEX events_task_seq ON events (task_id, seq);
         CREATE TABLE history (id INTEGER PRIMARY KEY, data BLOB NOT NULL);
         CREATE TABLE receipts (id TEXT PRIMARY KEY, data BLOB NOT NULL);",
    )?;
    ensure_message_schema(db)
}

/// Open `dir/state.db`, creating it atomically (temp file + rename) on first use.
/// A file that exists but cannot be loaded is never overwritten.
pub fn open_state_db(dir: &Path) -> Result<(Connection, Loaded)> {
    let path = dir.join(STATE_DB_NAME);
    if path.exists() {
        let db = open_sqlite(&path)?;
        ensure_message_schema(&db)?;
        let loaded =
            load_state(&db).map_err(|e| e.context(&format!("invalid {STATE_DB_NAME}; preserve it for recovery")))?;
        return Ok((db, loaded));
    }
    let tmp = dir.join(format!(".state-{}.db", random_hex(6)?));
    let created = (|| -> Result<()> {
        let db = open_sqlite(&tmp)?;
        create_schema(&db)?;
        let master = random_hex(32)?;
        let worker = random_hex(32)?;
        for (key, value) in [
            ("format", FORMAT.to_string()),
            ("next_seq", "0".to_string()),
            ("master_join", master),
            ("worker_join", worker),
        ] {
            db.execute("INSERT INTO meta (key,value) VALUES (?1,?2)", params![key, value])?;
        }
        db.close().map_err(|(_, e)| Error::from(e))?;
        Ok(())
    })();
    if let Err(e) = created {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::new(format!("publish {STATE_DB_NAME}: {e}")));
    }
    let db = open_sqlite(&path)?;
    let loaded = load_state(&db)?;
    Ok((db, loaded))
}

fn meta(db: &Connection, key: &str) -> Result<String> {
    db.query_row("SELECT value FROM meta WHERE key=?1", params![key], |row| row.get::<_, String>(0))
        .optional()?
        .ok_or_else(|| Error::new(format!("missing meta key {key}")))
}

pub fn load_state(db: &Connection) -> Result<Loaded> {
    let format: i64 = meta(db, "format")?.parse().map_err(|_| Error::new("invalid format value"))?;
    if format != FORMAT {
        return Err(Error::new("missing or unsupported state fields"));
    }
    let next_seq: i64 = meta(db, "next_seq")?.parse().map_err(|_| Error::new("invalid next_seq value"))?;
    if next_seq < 0 {
        return Err(Error::new("negative event sequence"));
    }
    let master_join = meta(db, "master_join")?;
    let worker_join = meta(db, "worker_join")?;
    if master_join.is_empty() || worker_join.is_empty() {
        return Err(Error::new("missing or unsupported state fields"));
    }
    let mut clients = BTreeMap::new();
    {
        let mut stmt = db.prepare("SELECT id,data FROM clients")?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)))?;
        for row in rows {
            let (id, raw) = row?;
            let client: Client =
                serde_json::from_slice(&raw).map_err(|e| Error::new(format!("invalid client {id:?}: {e}")))?;
            if client.id != id {
                return Err(Error::new(format!("invalid client {id:?}: id mismatch")));
            }
            clients.insert(id, client);
        }
    }
    let mut tasks = BTreeMap::new();
    {
        let mut stmt = db.prepare("SELECT id,data FROM tasks")?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)))?;
        for row in rows {
            let (id, raw) = row?;
            let task: Task =
                serde_json::from_slice(&raw).map_err(|e| Error::new(format!("invalid task {id:?}: {e}")))?;
            if task.id != id {
                return Err(Error::new(format!("invalid task {id:?}: id mismatch")));
            }
            tasks.insert(id, task);
        }
    }
    let max_event: i64 = db.query_row("SELECT COALESCE(MAX(seq),0) FROM events", [], |row| row.get(0))?;
    if max_event > next_seq {
        return Err(Error::new("invalid event sequence"));
    }
    let history_len: i64 = db.query_row("SELECT COALESCE(MAX(id),0) FROM history", [], |row| row.get(0))?;
    Ok(Loaded { clients, tasks, next_seq, history_len, master_join, worker_join })
}
