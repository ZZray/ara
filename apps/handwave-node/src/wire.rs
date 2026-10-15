use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const MAX_FRAME: usize = 1024 * 1024;
pub const CHUNK: usize = 16 * 1024;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Frame {
    pub k: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub cmd: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub msg: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub protocol: String,
    #[serde(default)]
    pub smart: bool,
    #[serde(default)]
    pub brief: String,
    /// Explicit web workspace/session identity. Empty retains the legacy path.
    #[serde(default)]
    pub session_key: String,
    #[serde(default)]
    pub b64: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub timeout: u64,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub task: Option<Value>,
}

pub fn dc(frame: Value) -> Value {
    json!({"k":"dc", "d":STANDARD.encode(serde_json::to_vec(&frame).expect("JSON serializes"))})
}

pub fn decode_dc(sig: &Value) -> anyhow::Result<Frame> {
    let encoded = sig["d"].as_str().ok_or_else(|| anyhow::anyhow!("missing dc payload"))?;
    anyhow::ensure!(encoded.len() <= MAX_FRAME * 4 / 3 + 4, "dc payload too large");
    let bytes = STANDARD.decode(encoded)?;
    anyhow::ensure!(bytes.len() <= MAX_FRAME, "dc payload too large");
    Ok(serde_json::from_slice(&bytes)?)
}

pub fn out(id: &str, data: &[u8], done: bool, code: i32, error: &str) -> Value {
    dc(json!({"k":"out","id":id,"b64":STANDARD.encode(data),"done":done,"code":code,"err":error}))
}

pub fn event(id: &str, mut event: Value) -> Value {
    event["id"] = json!(fresh_id());
    event["task_id"] = json!(id);
    event["ts"] = json!(chrono::Utc::now().to_rfc3339());
    dc(json!({"k":"agent-event", "id":id, "event":event}))
}

pub fn fresh_id() -> String {
    let mut bytes = [0u8; 12];
    getrandom::fill(&mut bytes).expect("OS random source");
    data_encoding::HEXLOWER.encode(&bytes)
}
