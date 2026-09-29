//! Wire and storage types. Field names and JSON shapes follow the Go structs in
//! `core.go` and `message_store.go` so clients of either implementation agree.

use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub type Time = DateTime<Utc>;

/// Go's zero `time.Time` (0001-01-01T00:00:00Z): "unset" for leases and progress.
pub fn zero_time() -> Time {
    Utc.with_ymd_and_hms(1, 1, 1, 0, 0, 0).single().expect("valid zero time")
}

pub fn is_zero_time(t: &Time) -> bool {
    *t == zero_time()
}

/// Worker execution lease. A running task whose lease passes becomes unconfirmed.
pub const LEASE_SECONDS: i64 = 120;
/// Maximum audit events returned by status.
pub const EVENT_LIMIT: usize = 1000;
/// Chat and task notification retention.
pub const MESSAGE_LIFETIME_SECONDS: i64 = 24 * 60 * 60;
/// Poll/history page byte budget (message bodies).
pub const MESSAGE_PAGE_BYTES: usize = 8 << 20;

/// One API request. Unknown JSON fields are rejected; absent fields are zero, except
/// `version` / `generation`, which stay absent: only claim may omit them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Request {
    pub op: String,
    pub request_id: String,
    pub client_id: String,
    pub role: String,
    pub name: String,
    pub task_id: String,
    pub worker_id: String,
    pub title: String,
    pub workspace: String,
    pub mode: String,
    pub body: String,
    pub snapshot: String,
    pub decision: String,
    pub evidence: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation: Option<i64>,
    pub cursor: i64,
    pub after: i64,
    pub limit: i64,
    pub ack_seq: i64,
    pub expected_cursor: i64,
    pub expected_count: i64,
    pub percent: i64,
    pub description: String,
    pub subtask_id: String,
    pub subtask_title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Client {
    pub id: String,
    pub role: String,
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub token: String,
    pub seen: Time,
    pub poll_at: Time,
}

impl Default for Client {
    fn default() -> Self {
        Client {
            id: String::new(),
            role: String::new(),
            name: String::new(),
            token: String::new(),
            seen: zero_time(),
            poll_at: zero_time(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Subtask {
    pub id: String,
    pub title: String,
    pub percent: i64,
    pub description: String,
    pub updated: Time,
}

impl Default for Subtask {
    fn default() -> Self {
        Subtask {
            id: String::new(),
            title: String::new(),
            percent: 0,
            description: String::new(),
            updated: zero_time(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub workspace: String,
    pub mode: String,
    pub worker_id: String,
    pub state: String,
    pub instruction: String,
    pub body: String,
    pub snapshot: String,
    pub version: i64,
    pub generation: i64,
    pub updated: Time,
    pub lease_until: Time,
    pub progress_percent: i64,
    pub progress_note: String,
    pub progress_at: Time,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub subtasks: Vec<Subtask>,
}

impl Default for Task {
    fn default() -> Self {
        Task {
            id: String::new(),
            title: String::new(),
            workspace: String::new(),
            mode: String::new(),
            worker_id: String::new(),
            state: String::new(),
            instruction: String::new(),
            body: String::new(),
            snapshot: String::new(),
            version: 0,
            generation: 0,
            updated: zero_time(),
            lease_until: zero_time(),
            progress_percent: 0,
            progress_note: String::new(),
            progress_at: zero_time(),
            subtasks: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClearTasksResult {
    pub workspace: String,
    pub count: i64,
    pub task_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Event {
    pub seq: i64,
    pub to: String,
    pub task_id: String,
    pub kind: String,
    pub body: String,
    pub at: Time,
}

impl Default for Event {
    fn default() -> Self {
        Event {
            seq: 0,
            to: String::new(),
            task_id: String::new(),
            kind: String::new(),
            body: String::new(),
            at: zero_time(),
        }
    }
}

/// The client-visible state returned by `status` (tokens and join keys removed).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub clients: BTreeMap<String, Client>,
    pub tasks: BTreeMap<String, Task>,
    pub events: Vec<Event>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub master_join: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub worker_join: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub at: Time,
    pub client_id: String,
    pub request: Request,
    pub task: Task,
}

/// Stored idempotency record: the exact request and its result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Receipt {
    pub request: Request,
    pub result: serde_json::Value,
}

/// One chat message or task notification. A broadcast is a single message with a
/// frozen recipient set stored in `message_deliveries`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Message {
    pub id: String,
    pub seq: i64,
    pub task_id: String,
    pub from_id: String,
    pub to_id: String,
    pub kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub event_kind: String,
    pub body: String,
    pub at: Time,
}

impl Default for Message {
    fn default() -> Self {
        Message {
            id: String::new(),
            seq: 0,
            task_id: String::new(),
            from_id: String::new(),
            to_id: String::new(),
            kind: String::new(),
            event_kind: String::new(),
            body: String::new(),
            at: zero_time(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PollResult {
    pub messages: Vec<Message>,
    pub has_new: bool,
    pub has_more: bool,
    pub next_cursor: i64,
    pub ack_cursor: i64,
    pub retention_gap: bool,
    pub expired_through_seq: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AckResult {
    pub ack_cursor: i64,
}

/// Profile / join-file content: where the service is and which credential to use.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connection {
    pub url: String,
    pub token: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    pub role: String,
}

/// Result of `Store::execute`, mirroring the dynamic return values in Go.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Outcome {
    Status(State),
    Clients(Vec<Client>),
    Tasks(Vec<Task>),
    Client(Client),
    Task(Task),
    Cleared(ClearTasksResult),
}

impl Outcome {
    pub fn to_value(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("outcome is always serializable")
    }

    pub fn into_task(self) -> Option<Task> {
        match self {
            Outcome::Task(t) => Some(t),
            _ => None,
        }
    }

    pub fn into_tasks(self) -> Option<Vec<Task>> {
        match self {
            Outcome::Tasks(t) => Some(t),
            _ => None,
        }
    }

    pub fn into_client(self) -> Option<Client> {
        match self {
            Outcome::Client(c) => Some(c),
            _ => None,
        }
    }

    pub fn into_clients(self) -> Option<Vec<Client>> {
        match self {
            Outcome::Clients(c) => Some(c),
            _ => None,
        }
    }

    pub fn into_cleared(self) -> Option<ClearTasksResult> {
        match self {
            Outcome::Cleared(c) => Some(c),
            _ => None,
        }
    }

    pub fn into_status(self) -> Option<State> {
        match self {
            Outcome::Status(s) => Some(s),
            _ => None,
        }
    }
}
