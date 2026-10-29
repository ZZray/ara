//! Project service-control wire, adapted from OMP launch/protocol.ts and
//! pi-tui/tools/daemon.ts at b91db66e (MIT). See THIRD_PARTY_NOTICES.md.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

pub const VERSION: u32 = 1;
pub const MAX_FRAME: usize = 4 * 1024 * 1024;
pub const MAX_INPUT: usize = 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServiceSpec {
    pub name: String,
    pub application: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: PathBuf,
    pub pty: bool,
    pub persist: bool,
    pub detached: bool,
}
impl ServiceSpec {
    /// Persisted metadata remains meaningful after its historical cwd disappears.
    /// New process creation separately checks live filesystem prerequisites.
    pub fn validate_structure(&self) -> Result<(), String> {
        validate_name(&self.name)?;
        if self.application.is_empty() || self.application.contains('\0') {
            return Err("Service application is required".into());
        }
        if !self.cwd.is_absolute() {
            return Err("Service cwd must be absolute".into());
        }
        if self.args.iter().any(|v| v.contains('\0'))
            || self.env.iter().any(|(k,v)| k.is_empty() || k.contains('=') || k.contains('\0') || v.contains('\0')) {
            return Err("Service arguments/environment contain invalid bytes".into());
        }
        if self.detached && (self.pty || !self.persist) {
            return Err("Detached service requires pipe transport and persistence".into());
        }
        Ok(())
    }
    pub fn validate(&self) -> Result<(), String> {
        self.validate_structure()?;
        if !self.cwd.is_dir(){return Err("Service cwd must be an existing absolute directory".into());}
        Ok(())
    }
}
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 48 || !name.as_bytes()[0].is_ascii_alphanumeric()
        || !name.bytes().all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c)) {
        return Err("Service name must be 1-48 letters, numbers, dots, underscores, or hyphens".into());
    }
    Ok(())
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ServiceState { Starting, Running, Ready, Restarting, Stopping, Exited, Failed }
impl ServiceState { pub fn terminal(self) -> bool { matches!(self, Self::Exited | Self::Failed) } }
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ServiceMode { Session, Persist, Detached }

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessIdentity { pub pid: u32, pub created: String }
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProcessExit { pub code: Option<i32>, pub reason: Option<String> }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceSnapshot {
    pub name: String, pub id: String, pub state: ServiceState,
    pub pid: Option<u32>, pub created_at: u64, pub started_at: u64,
    pub exited_at: Option<u64>, pub exit_code: Option<i32>, pub exit_reason: Option<String>,
    pub owner: Option<String>, pub persist: bool, pub detached: bool,
    /// Internal correlation; public operations still select by service name.
    pub generation: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Completion {
    pub completion_id: String, pub owner: String, pub daemon: ServiceSnapshot,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", deny_unknown_fields)]
pub enum Operation {
    Ping,
    List,
    Find { name: String },
    Start { spec: ServiceSpec, owner: Option<String>, replace: bool },
    Send { name: String, data: String },
    Mode { name: String, mode: ServiceMode },
    Stop { name: String, timeout_ms: u64 },
    Subscribe { owner: String, subscription: String },
    Poll { owner: String, subscription: String },
    Ack { owner: String, subscription: String, completion_ids: Vec<String> },
    Unsubscribe { owner: String, subscription: String, preserve_pending: bool },
}
impl Operation {
    pub fn mutation(&self) -> bool {
        matches!(self, Self::Start{..}|Self::Send{..}|Self::Mode{..}|Self::Stop{..})
    }
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request { pub version: u32, pub id: String, pub token: String, pub operation: Operation }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag="kind", rename_all="camelCase")]
pub enum Reply {
    Pong,
    List { services: Vec<ServiceSnapshot> },
    Service { daemon: ServiceSnapshot },
    Completions { entries: Vec<Completion> },
    Acked { removed: usize },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WireError { pub code: String, pub message: String, #[serde(default)] pub outcome_unknown: bool }
#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    pub version: u32, pub id: String,
    pub result: Option<Reply>, pub error: Option<WireError>,
}
impl Response {
    pub fn new(id: String, result: Result<Reply, WireError>) -> Self {
        let (result,error)=match result { Ok(v)=>(Some(v),None), Err(e)=>(None,Some(e)) };
        Self { version: VERSION,id,result,error }
    }
}
pub(crate) fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}
