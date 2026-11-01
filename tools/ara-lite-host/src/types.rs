use ara_agent_lite_harness::{TaskOptions, TaskSpec};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EventBody {
    Work { title: String, workspace: PathBuf, objective: String },
    Followup { task_id: String, generation: i64, text: String },
    Answer { task_id: String, generation: i64, question_id: String, text: String },
    Cancel { task_id: String, generation: i64 },
}

/// The transport must authenticate source/actor. This reference Host accepts
/// these only from its operator's local stdin, never from arbitrary remote JSON.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputEvent {
    pub source: String,
    pub event_id: String,
    pub actor: String,
    pub authorization_revision: String,
    pub explicit: bool,
    pub body: EventBody,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    Pending,
    Dispatching,
    Applied,
    Unapplied,
    Unknown,
    Observed,
    Rejected,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventRecord {
    pub key: String,
    pub input: InputEvent,
    pub task_id: Option<String>,
    pub generation: Option<i64>,
    pub state: DeliveryState,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionPhase {
    Queued,
    Starting,
    Running,
    WaitingHuman,
    ReadyForReview,
    NeedsReconciliation,
    Finished,
}

/// Runtime binding only, not another business Task record. No credentials.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionBinding {
    pub spec: TaskSpec,
    pub options: TaskOptions,
    pub session_path: PathBuf,
    pub environment_path: PathBuf,
    pub workspace: PathBuf,
    pub generation: i64,
    pub phase: ExecutionPhase,
    pub attempt: u32,
    pub adapter_revision: String,
    pub initial_run_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attention {
    pub id: String,
    pub task_id: String,
    pub generation: i64,
    pub question: String,
    pub answer: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeState {
    Pending,
    Dispatching,
    Sent,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notice {
    pub id: String,
    pub task_id: String,
    pub generation: i64,
    pub kind: String,
    pub body: String,
    pub state: NoticeState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceSource {
    pub id: String,
    pub task_id: String,
    pub generation: i64,
    pub version: u64,
    pub path: PathBuf,
    pub sha256: String,
    pub allowed_tasks: Vec<String>,
    pub active: bool,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock before epoch").as_millis() as u64
}
