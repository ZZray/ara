//! Public embedding contracts, exercised without a provider or OS tool effect.
use anyhow::Result;
use ara_agent_lite_core::{
    context_memory::{hash, CloudRecord, CloudSnapshot},
    execute,
    model::{ModelHost, ModelSelection, Route},
    rpi_agent::{AgentError, AgentTool, AgentToolResult, ToolResultPartial},
    rpi_ai::{Schema, Tool},
    RunMode, RunRequest, RuntimePaths, ToolRegistration,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

fn request() -> RunRequest {
    let root = std::env::temp_dir().join(format!("lite-host-boundary-{}", ara_agent_lite_core::fresh_id()));
    RunRequest::new("run", "Inspect business state", RuntimePaths::new(&root, root.join("session"), root.join("env")))
}

#[test]
fn constructor_requires_explicit_local_and_worker_authority() {
    let request = request();
    assert_eq!(request.mode, RunMode::Plan);
    assert!(!request.enable_local_tools);
    assert!(request.worker_executable.is_none());
    assert!(request.extra_tools.is_empty());
    assert!(request.context_snapshot.is_none());
    assert!(request.knowledge_roots.is_none());
    assert!(request.goal_lease.is_none());
    assert!(request.resume_job_lease.is_none());
    assert!(!request.cwd.exists());
}

struct MustNotResolve(Arc<AtomicUsize>);
#[async_trait]
impl ModelHost for MustNotResolve {
    async fn resolve(&self, _: Route, _: &str, _: &str, _: &str) -> Result<ModelSelection> {
        self.0.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("model resolution must not happen for invalid registrations")
    }
}

struct MustNotExecute(Tool);
#[async_trait]
impl AgentTool for MustNotExecute {
    fn schema(&self) -> &Tool {
        &self.0
    }
    fn label(&self) -> &str {
        &self.0.name
    }
    async fn execute(
        &self,
        _: &str,
        _: Value,
        _: CancellationToken,
        _: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        panic!("invalid custom registration must not reach tool execution")
    }
}

fn registration(name: &str) -> ToolRegistration {
    ToolRegistration {
        tool: Arc::new(MustNotExecute(Tool {
            name: name.into(),
            description: "Must not execute".into(),
            parameters: Schema(json!({"type":"object","properties":{}})),
            constrained_sampling: None,
        })),
        allow_in_plan: true,
    }
}

#[tokio::test]
async fn invalid_custom_names_are_rejected_before_model_state_or_tool_effects() {
    let mut cases: Vec<Vec<&str>> =
        ara_agent_lite_core::agent::BUILTIN_TOOL_NAMES.iter().map(|name| vec![*name]).collect();
    cases.extend([vec![""], vec!["bad name"], vec!["business_lookup", "business_lookup"]]);
    for names in cases {
        let mut request = request();
        let root = request.cwd.clone();
        let resolutions = Arc::new(AtomicUsize::new(0));
        request.model_host = Arc::new(MustNotResolve(resolutions.clone()));
        request.extra_tools = names.iter().map(|name| registration(name)).collect();
        let (events, mut receiver) = mpsc::channel(1);
        let error =
            execute(request, CancellationToken::new(), events).await.err().expect("invalid registration accepted");
        assert!(error.to_string().contains("custom tool name"), "unexpected error: {error}");
        assert_eq!(resolutions.load(Ordering::SeqCst), 0);
        assert!(!root.exists());
        assert!(receiver.try_recv().is_err());
    }
}

fn snapshot() -> CloudSnapshot {
    CloudSnapshot {
        version: 1,
        status: "ready".into(),
        device_id: "device-a".into(),
        epoch: "epoch-a".into(),
        records: vec![CloudRecord {
            id: "record-a".into(),
            revision: 1,
            kind: "fact".into(),
            text: "bounded source".into(),
            hash: hash(b"bounded source"),
            scope: "device".into(),
            scope_id: "device-a".into(),
        }],
    }
}

#[test]
fn typed_snapshot_enforces_identity_integrity_and_whole_payload_size() {
    let good = snapshot();
    good.validate("device-a").unwrap();
    assert!(good.validate("device-b").is_err());
    let mut bad = good.clone();
    bad.records[0].hash = hash(b"different source");
    assert!(bad.validate("device-a").is_err());
    let mut bad = good.clone();
    bad.records[0].scope_id = "device-b".into();
    assert!(bad.validate("device-a").is_err());
    let mut large = good;
    large.records = (0..4)
        .map(|index| {
            let mut record = snapshot().records.remove(0);
            record.id = format!("record-{index}");
            record.text = "x".repeat(8192);
            record.hash = hash(record.text.as_bytes());
            record
        })
        .collect();
    assert!(serde_json::to_vec(&large).unwrap().len() > 32768);
    assert!(large.validate("device-a").unwrap_err().to_string().contains("32768"));
}
