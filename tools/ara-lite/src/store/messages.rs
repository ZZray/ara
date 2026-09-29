//! Retained chat and task notifications (Go: `message_store.go`).
//!
//! Messages live in relational tables so cursors, per-recipient deliveries and the
//! 24 h retention window can be enforced in SQL. All functions run under the store
//! lock; callers never see a half-written message.

use super::Inner;
use crate::error::{Error, Result};
use crate::types::*;
use crate::util::{equal_bytes, random_hex};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use rusqlite::{Connection, OptionalExtension, Row, params};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// A notification queued during a state change and inserted in the same transaction.
pub(super) struct MessageDraft {
    pub to: String,
    pub task_id: String,
    pub event_kind: String,
    pub body: String,
    pub at: Time,
}

const MESSAGE_COLUMNS: &str = "m.seq,m.id,m.task_id,m.from_id,m.to_id,m.kind,m.event_kind,m.body,m.at_ns";
const RECENT_BYTES: usize = 2 << 20;
const MAX_BODY: usize = 512 << 10;

fn nanos(t: Time) -> i64 {
    t.timestamp_nanos_opt().unwrap_or(0)
}

fn scan_message(row: &Row<'_>) -> rusqlite::Result<Message> {
    let at_ns: i64 = row.get(8)?;
    Ok(Message {
        seq: row.get(0)?,
        id: row.get(1)?,
        task_id: row.get(2)?,
        from_id: row.get(3)?,
        to_id: row.get(4)?,
        kind: row.get(5)?,
        event_kind: row.get(6)?,
        body: row.get(7)?,
        at: DateTime::<Utc>::from_timestamp_nanos(at_ns),
    })
}

/// Insert one message plus its frozen recipient set (Go: `insertMessageTx`).
/// An empty `to` broadcasts to every client except the sender.
#[allow(clippy::too_many_arguments)]
pub(super) fn insert_message(
    tx: &Connection,
    clients: &BTreeMap<String, Client>,
    from: &str,
    to: &str,
    task_id: &str,
    kind: &str,
    event_kind: &str,
    body: &str,
    at: Time,
) -> Result<Message> {
    let recipients: Vec<String> = if to.is_empty() {
        // BTreeMap iteration is already sorted by client id.
        clients.keys().filter(|id| id.as_str() != from).cloned().collect()
    } else {
        if !clients.contains_key(to) {
            return Err(Error::new(format!("unknown recipient {to:?}")));
        }
        vec![to.to_string()]
    };
    if recipients.is_empty() {
        return Err(Error::new("message has no recipients"));
    }
    let id = random_hex(12)?;
    tx.execute(
        "INSERT INTO messages(id,task_id,from_id,to_id,kind,event_kind,body,at_ns,expires_ns) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            id,
            task_id,
            from,
            to,
            kind,
            event_kind,
            body,
            nanos(at),
            nanos(at + ChronoDuration::seconds(MESSAGE_LIFETIME_SECONDS))
        ],
    )?;
    let seq = tx.last_insert_rowid();
    for recipient in &recipients {
        tx.execute("INSERT INTO message_deliveries(message_seq,recipient_id) VALUES(?1,?2)", params![seq, recipient])?;
    }
    Ok(Message {
        id,
        seq,
        task_id: task_id.to_string(),
        from_id: from.to_string(),
        to_id: to.to_string(),
        kind: kind.to_string(),
        event_kind: event_kind.to_string(),
        body: body.to_string(),
        at,
    })
}

fn reverse_within_bytes(mut rows: Vec<Message>) -> Vec<Message> {
    rows.reverse();
    rows
}

impl Inner {
    /// Drop expired messages, deliveries and receipts. Recipient watermarks survive
    /// deletion so a late poll can report `retention_gap` (Go: `pruneMessages`).
    pub(super) fn prune_messages(&mut self, at: Time) -> Result<()> {
        let now_ns = nanos(at);
        let watermarks = {
            let db = self.ensure_open()?;
            let tx = db.unchecked_transaction()?;
            let mut marks: Vec<(String, i64)> = Vec::new();
            {
                let mut stmt = tx.prepare(
                    "SELECT d.recipient_id, MAX(d.message_seq) FROM message_deliveries d JOIN messages m ON m.seq=d.message_seq WHERE m.expires_ns<=?1 GROUP BY d.recipient_id",
                )?;
                let rows =
                    stmt.query_map(params![now_ns], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?;
                for row in rows {
                    marks.push(row?);
                }
            }
            for (id, seq) in &marks {
                tx.execute(
                    "INSERT INTO message_cursors(client_id,ack_seq,expired_through_seq) VALUES(?1,0,?2) ON CONFLICT(client_id) DO UPDATE SET expired_through_seq=MAX(expired_through_seq,excluded.expired_through_seq)",
                    params![id, seq],
                )?;
            }
            tx.execute(
                "DELETE FROM message_deliveries WHERE message_seq IN (SELECT seq FROM messages WHERE expires_ns<=?1)",
                params![now_ns],
            )?;
            tx.execute("DELETE FROM messages WHERE expires_ns<=?1", params![now_ns])?;
            tx.execute("DELETE FROM message_receipts WHERE expires_ns<=?1", params![now_ns])?;
            tx.commit()?;
            marks.len()
        };
        if watermarks > 0 {
            self.bump();
        }
        Ok(())
    }

    pub(super) fn authenticate(&self, token: &str, requested_id: &str) -> Result<Client> {
        if self.closed {
            return Err(Error::new("store is closed"));
        }
        for client in self.clients.values() {
            if crate::util::token_equal(&client.token, token) {
                if !requested_id.is_empty() && requested_id != client.id {
                    return Err(Error::new("client identity mismatch"));
                }
                return Ok(client.clone());
            }
        }
        Err(Error::new("unauthorized client"))
    }

    pub(super) fn send_chat(&mut self, token: &str, r: &Request) -> Result<Message> {
        let actor = self.authenticate(token, &r.client_id)?;
        let now = self.now_utc();
        self.contact.insert(actor.id.clone(), now);
        self.expire(false)?;
        if r.request_id.is_empty() || r.request_id.len() > 128 {
            return Err(Error::new("send requires request_id (1..128 characters)"));
        }
        if r.body.trim().is_empty() || r.body.len() > MAX_BODY {
            return Err(Error::new("message body is required (up to 512 KiB)"));
        }
        if !r.task_id.is_empty()
            && let Some(task) = self.tasks.get(&r.task_id)
            && task.state == "cleared"
        {
            return Err(Error::new("task was cleared; use current tasks"));
        }
        let prune_at = self.now_utc();
        self.prune_messages(prune_at).map_err(|e| e.context("persist operation"))?;
        let key_body = serde_json::to_vec(&serde_json::json!({
            "TaskID": r.task_id,
            "WorkerID": r.worker_id,
            "Body": r.body,
        }))?;
        let digest: Vec<u8> = Sha256::digest(&key_body).to_vec();
        let prior: Option<(i64, Vec<u8>)> = self
            .ensure_open()?
            .query_row(
                "SELECT message_seq,payload_digest FROM message_receipts WHERE client_id=?1 AND request_id=?2",
                params![actor.id, r.request_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((prior_seq, prior_digest)) = prior {
            if !equal_bytes(&prior_digest, &digest) {
                return Err(Error::new("request_id already used with different payload"));
            }
            let replayed = self.ensure_open()?.query_row(
                &format!("SELECT {MESSAGE_COLUMNS} FROM messages m WHERE m.seq=?1"),
                params![prior_seq],
                scan_message,
            )?;
            return Ok(replayed);
        }
        let to: String;
        if !r.task_id.is_empty() {
            let task = self.tasks.get(&r.task_id).ok_or_else(|| Error::new("unknown task"))?;
            if task.worker_id.is_empty() {
                return Err(Error::new("task has no assigned worker"));
            }
            if actor.role == "master" {
                if !r.worker_id.is_empty() && r.worker_id != task.worker_id {
                    return Err(Error::new("task chat targets its current worker only"));
                }
                to = task.worker_id.clone();
            } else {
                if task.worker_id != actor.id || !r.worker_id.is_empty() {
                    return Err(Error::new("task chat is limited to the current worker and master"));
                }
                to = super::master_id(&self.clients);
            }
        } else if actor.role == "master" {
            to = r.worker_id.clone();
            if !to.is_empty() {
                match self.clients.get(&to) {
                    Some(c) if c.role == "worker" => {}
                    _ => return Err(Error::new("unknown worker")),
                }
            }
        } else {
            if !r.worker_id.is_empty() {
                return Err(Error::new("worker cannot target another worker"));
            }
            to = super::master_id(&self.clients);
        }
        if to.is_empty() && actor.role != "master" {
            return Err(Error::new("master is not connected"));
        }
        let at = self.now_utc();
        let message = {
            let db = self.ensure_open()?;
            let tx = db.unchecked_transaction().map_err(|e| Error::from(e).context("persist operation"))?;
            let m = insert_message(&tx, &self.clients, &actor.id, &to, &r.task_id, "chat", "", &r.body, at)
                .map_err(|e| e.context("persist operation"))?;
            tx.execute(
                "INSERT INTO message_receipts(client_id,request_id,payload_digest,message_seq,expires_ns) VALUES(?1,?2,?3,?4,?5)",
                params![
                    actor.id,
                    r.request_id,
                    digest,
                    m.seq,
                    nanos(at + ChronoDuration::seconds(MESSAGE_LIFETIME_SECONDS))
                ],
            )
            .map_err(|e| Error::from(e).context("persist operation"))?;
            tx.commit().map_err(|e| Error::from(e).context("persist operation"))?;
            m
        };
        self.contact.insert(actor.id.clone(), at);
        self.bump();
        Ok(message)
    }

    pub(super) fn poll_messages(&mut self, token: &str, after: i64, limit: i64) -> Result<PollResult> {
        let actor = self.authenticate(token, "")?;
        if after < -1 {
            return Err(Error::new("after must be -1 or nonnegative"));
        }
        let limit = if limit == 0 { 100 } else { limit };
        if !(1..=100).contains(&limit) {
            return Err(Error::new("limit must be 1..100"));
        }
        let at = self.now_utc();
        self.prune_messages(at)?;
        let out = {
            let db = self.ensure_open()?;
            let (ack, delivered, expired) = db
                .query_row(
                    "SELECT ack_seq,delivered_seq,expired_through_seq FROM message_cursors WHERE client_id=?1",
                    params![actor.id],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?)),
                )
                .optional()?
                .unwrap_or((0, 0, 0));
            let after = if after == -1 { ack } else { after };
            if after > delivered && after > ack && after > expired {
                return Err(Error::new("after cursor has not been delivered; resume from ack cursor"));
            }
            let mut out = PollResult {
                messages: Vec::new(),
                next_cursor: after,
                ack_cursor: ack,
                expired_through_seq: expired,
                retention_gap: after < expired,
                ..PollResult::default()
            };
            let mut stmt = db.prepare(&format!(
                "SELECT {MESSAGE_COLUMNS} FROM messages m JOIN message_deliveries d ON d.message_seq=m.seq WHERE d.recipient_id=?1 AND m.seq>?2 AND m.expires_ns>?3 ORDER BY m.seq LIMIT ?4"
            ))?;
            let mut rows = stmt.query(params![actor.id, after, nanos(at), limit + 1])?;
            let mut page_bytes = 0usize;
            while let Some(row) = rows.next()? {
                let m = scan_message(row)?;
                if out.messages.len() as i64 == limit
                    || (!out.messages.is_empty() && page_bytes + m.body.len() > MESSAGE_PAGE_BYTES)
                {
                    out.has_more = true;
                    break;
                }
                page_bytes += m.body.len();
                out.next_cursor = m.seq;
                out.messages.push(m);
            }
            drop(rows);
            drop(stmt);
            let unread: i64 = db.query_row(
                "SELECT COUNT(*) FROM message_deliveries d JOIN messages m ON m.seq=d.message_seq WHERE d.recipient_id=?1 AND m.seq>?2 AND m.expires_ns>?3",
                params![actor.id, ack, nanos(at)],
                |row| row.get(0),
            )?;
            out.has_new = unread > 0;
            if out.next_cursor > delivered {
                db.execute(
                    "INSERT INTO message_cursors(client_id,ack_seq,delivered_seq,expired_through_seq) VALUES(?1,0,?2,0) ON CONFLICT(client_id) DO UPDATE SET delivered_seq=MAX(delivered_seq,excluded.delivered_seq)",
                    params![actor.id, out.next_cursor],
                )?;
            }
            out
        };
        self.poll_contact.insert(actor.id.clone(), at);
        self.contact.insert(actor.id.clone(), at);
        Ok(out)
    }

    pub(super) fn ack_messages(&mut self, token: &str, expected: i64, seq: i64) -> Result<AckResult> {
        let actor = self.authenticate(token, "")?;
        if expected < 0 || seq < expected {
            return Err(Error::new("ack sequence must advance from a nonnegative expected cursor"));
        }
        let at = self.now_utc();
        self.prune_messages(at)?;
        let db = self.ensure_open()?;
        let tx = db.unchecked_transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO message_cursors(client_id,ack_seq,expired_through_seq) VALUES(?1,0,0)",
            params![actor.id],
        )?;
        let (current, delivered, expired) = tx.query_row(
            "SELECT ack_seq,delivered_seq,expired_through_seq FROM message_cursors WHERE client_id=?1",
            params![actor.id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?)),
        )?;
        if current != expected {
            return Err(Error::new(format!("ack cursor conflict: current cursor is {current}")));
        }
        if seq != current && seq != expired {
            if seq > delivered {
                return Err(Error::new("ack sequence has not been delivered"));
            }
            let exists: i64 = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM message_deliveries WHERE recipient_id=?1 AND message_seq=?2)",
                params![actor.id, seq],
                |row| row.get(0),
            )?;
            if exists == 0 {
                return Err(Error::new("ack sequence is not a delivered message for this client"));
            }
        }
        tx.execute("UPDATE message_cursors SET ack_seq=?1 WHERE client_id=?2", params![seq, actor.id])?;
        tx.commit()?;
        Ok(AckResult { ack_cursor: seq })
    }

    pub(super) fn message_history(
        &mut self,
        token: &str,
        task_id: &str,
        worker_id: &str,
        limit: i64,
    ) -> Result<Vec<Message>> {
        let actor = self.authenticate(token, "")?;
        let limit = if limit == 0 { 100 } else { limit };
        if !(1..=100).contains(&limit) {
            return Err(Error::new("limit must be 1..100"));
        }
        if !task_id.is_empty() && !self.tasks.contains_key(task_id) {
            return Err(Error::new("unknown task"));
        }
        if !worker_id.is_empty() {
            if actor.role == "worker" && worker_id != actor.id {
                return Err(Error::new("worker history is private"));
            }
            match self.clients.get(worker_id) {
                Some(c) if c.role == "worker" => {}
                _ => return Err(Error::new("unknown worker")),
            }
        }
        let at = self.now_utc();
        self.prune_messages(at)?;
        let db = self.ensure_open()?;
        let mut stmt = db.prepare(&format!(
            "SELECT {MESSAGE_COLUMNS} FROM messages m WHERE m.expires_ns>?1 AND (?2='' OR m.task_id=?2) AND (?3='' OR m.from_id=?3 OR m.to_id=?3 OR EXISTS(SELECT 1 FROM message_deliveries p WHERE p.message_seq=m.seq AND p.recipient_id=?3)) AND (m.from_id=?4 OR EXISTS(SELECT 1 FROM message_deliveries d WHERE d.message_seq=m.seq AND d.recipient_id=?4)) ORDER BY m.seq DESC LIMIT ?5"
        ))?;
        let mut rows = stmt.query(params![nanos(at), task_id, worker_id, actor.id, limit])?;
        let mut out: Vec<Message> = Vec::new();
        let mut page_bytes = 0usize;
        while let Some(row) = rows.next()? {
            let m = scan_message(row)?;
            if !out.is_empty() && page_bytes + m.body.len() > MESSAGE_PAGE_BYTES {
                break;
            }
            page_bytes += m.body.len();
            out.push(m);
        }
        Ok(reverse_within_bytes(out))
    }

    pub(super) fn recent_messages(&self, limit: i64) -> Result<Vec<Message>> {
        let db = self.ensure_open()?;
        let mut stmt = db.prepare(&format!(
            "SELECT {MESSAGE_COLUMNS} FROM messages m WHERE m.expires_ns>?1 ORDER BY m.seq DESC LIMIT ?2"
        ))?;
        let mut rows = stmt.query(params![nanos(self.now_utc()), limit])?;
        let mut out: Vec<Message> = Vec::new();
        let mut bytes = 0usize;
        while let Some(row) = rows.next()? {
            let m = scan_message(row)?;
            if !out.is_empty() && bytes + m.body.len() > RECENT_BYTES {
                break;
            }
            bytes += m.body.len();
            out.push(m);
        }
        Ok(reverse_within_bytes(out))
    }
}
