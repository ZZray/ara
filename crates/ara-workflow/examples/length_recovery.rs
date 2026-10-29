//! Controlled HTTP + real WriteTool/SessionJournal/native-child demonstration.
//! This exercises settlement and continuation, not real-model coding ability.
#[path = "../tests/support/mod.rs"]
mod support;

use ara_agent::{Agent, AgentEvent, AgentEventSink, AgentTool};
use ara_session::SessionJournal;
use ara_tools::{ToolContext, write::WriteTool};
use ara_workflow::*;
use async_trait::async_trait;
use ring::digest::{SHA256, digest};
use serde_json::{Value, json};
use std::{
    fs,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

struct JournalSink {
    journal: Mutex<SessionJournal>,
    failure: Mutex<Option<String>>,
    appended: Mutex<Vec<AppendedMessage>>,
}

struct AppendedMessage {
    id: String,
    body: Value,
    source: Option<Arc<Value>>,
}

fn persist(path: &Path, value: &Value) -> Result<(), std::io::Error> {
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    fs::OpenOptions::new().write(true).open(path)?.sync_all()
}

fn sha(bytes: &[u8]) -> String {
    digest(&SHA256, bytes).as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

#[async_trait]
impl AgentEventSink for JournalSink {
    async fn emit(&self, event: AgentEvent) {
        if let AgentEvent::MessageEnd { message } = event {
            let result = (|| -> Result<(), Box<dyn std::error::Error>> {
                let mut journal = self.journal.lock().unwrap();
                let body = message.json()?;
                let source = message.source();
                let id = journal.append_agent_message_body(&body)?;
                if let Some(source) = &source {
                    persist(
                        &journal.path().with_file_name(format!("source-{id}.json")),
                        &json!({"entry":id,"source":source.as_ref()}),
                    )?;
                }
                // A new Session remains lazy until its first Assistant. The
                // later materialization includes all earlier append receipts.
                if journal.is_on_disk() {
                    fs::OpenOptions::new().write(true).open(journal.path())?.sync_all()?;
                }
                self.appended.lock().unwrap().push(AppendedMessage { id, body, source });
                Ok(())
            })();
            if let Err(error) = result {
                *self.failure.lock().unwrap() = Some(error.to_string());
            }
        }
    }
}

struct FileHost {
    root: PathBuf,
    sink: Arc<JournalSink>,
}

impl FileHost {
    async fn inspect(&self, basis: &AttemptBasis, label: &str) -> Result<(Vec<u8>, ArtifactEvidence), ReviewError> {
        let fail = |error: &dyn std::fmt::Display| ReviewError(error.to_string());
        if let Some(error) = self.sink.failure.lock().unwrap().clone() {
            return Err(ReviewError(error));
        }
        let before = fs::read(self.root.join("answer.txt")).map_err(|e| fail(&e))?;
        #[cfg(windows)]
        let mut command = tokio::process::Command::new("cmd.exe");
        #[cfg(windows)]
        command.args(["/d", "/c", "type", "answer.txt"]);
        #[cfg(not(windows))]
        let mut command = tokio::process::Command::new("cat");
        #[cfg(not(windows))]
        command.arg("answer.txt");
        command.current_dir(&self.root).env_remove("ARA_MODEL_API_KEY").env_remove("RAY_GATEWAY_KEY");
        let child = command
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| fail(&e))?;
        let pid = child.id();
        let output = child.wait_with_output().await.map_err(|e| fail(&e))?;
        let after = fs::read(self.root.join("answer.txt")).map_err(|e| fail(&e))?;
        let journal_path = self.sink.journal.lock().unwrap().path().to_path_buf();
        let reopened = SessionJournal::open(&journal_path).map_err(|e| fail(&e))?;
        for AppendedMessage { id, body, source } in self.sink.appended.lock().unwrap().iter() {
            let entry = reopened
                .entries()
                .iter()
                .find(|entry| entry.id == *id)
                .ok_or_else(|| ReviewError("journal lost an append receipt".into()))?;
            if entry.raw["message"] != *body {
                return Err(ReviewError("journal body differs from its original append receipt".into()));
            }
            if let Some(source) = source {
                let path = journal_path.with_file_name(format!("source-{id}.json"));
                let saved: Value =
                    serde_json::from_slice(&fs::read(path).map_err(|e| fail(&e))?).map_err(|e| fail(&e))?;
                if saved["entry"] != *id || saved["source"] != **source {
                    return Err(ReviewError("journal source sidecar differs from its original receipt".into()));
                }
            }
        }
        let stable = before == after
            && output.status.success()
            && output.stdout == before
            && reopened.session_id() == basis.workflow.session_id;
        let path = self.root.join(format!("{label}-{}.json", basis.attempt));
        persist(
            &path,
            &json!({"basis":basis,"pid":pid,"exit_code":output.status.code(),
            "stdout_sha256":sha(&output.stdout),"stderr_sha256":sha(&output.stderr),
            "artifact_sha256":sha(&after),"journal":journal_path,"stable":stable,
            "coverage":"original joined child; not arbitrary descendant containment"}),
        )
        .map_err(|e| fail(&e))?;
        if !stable {
            return Err(ReviewError("artifact/process/Session identity mismatch".into()));
        }
        Ok((
            after.clone(),
            ArtifactEvidence {
                artifact_id: "answer.txt".into(),
                revision: sha(&after),
                evidence_ref: path.display().to_string(),
            },
        ))
    }
}

#[async_trait]
impl WorkflowHost for FileHost {
    async fn settle_length_stop(
        &self,
        stopped: &LengthStoppedRun,
        _: &CancellationToken,
    ) -> Result<LengthSettlementReceipt, ReviewError> {
        let (bytes, artifact) = self.inspect(&stopped.basis, "length-settlement").await?;
        if bytes != b"partial" {
            return Err(ReviewError("unexpected partial artifact".into()));
        }
        Ok(LengthSettlementReceipt {basis:stopped.basis.clone(),artifacts:vec![artifact],
            settlement:Settlement::Settled,commit:ReceiptCommit::Committed{receipt_id:"length-settlement-1".into()},
            continuation:Some("The earlier response hit its output limit. Its file effect and journal are settled; do not replay that write. Replace answer.txt with exactly correct, then submit a brief final response.".into())})
    }

    async fn review(&self, candidate: &ReviewCandidate, _: &CancellationToken) -> Result<ReviewReceipt, ReviewError> {
        let (bytes, artifact) = self.inspect(&candidate.basis, "review").await?;
        Ok(ReviewReceipt {
            basis: candidate.basis.clone(),
            artifacts: vec![artifact],
            settlement: Settlement::Settled,
            commit: ReceiptCommit::Committed { receipt_id: "final-review".into() },
            verdict: if bytes == b"correct" {
                ReviewVerdict::Verified
            } else {
                ReviewVerdict::NeedsChanges { feedback: "answer.txt must contain exactly correct".into() }
            },
        })
    }
}

async fn run(root: &Path) -> Result<Arc<WorkflowOutcome>, Box<dyn std::error::Error>> {
    let upstream = support::server(vec![
        support::write("partial"),
        support::text("length"),
        support::write("correct"),
        support::text("stop"),
    ])
    .await;
    let tools: Vec<Arc<dyn AgentTool>> = vec![Arc::new(WriteTool::new(ToolContext::new(root.to_path_buf())))];
    let config = support::config(&upstream, tools);
    let journal = SessionJournal::create(&root.join("session"), root)?;
    let mut request = support::request();
    request.basis.session_id = journal.session_id().to_owned();
    let sink =
        Arc::new(JournalSink { journal: Mutex::new(journal), failure: Mutex::new(None), appended: Mutex::new(vec![]) });
    let host = Arc::new(FileHost { root: root.to_path_buf(), sink: sink.clone() });
    let outcome = Workflow::start_with_length_recovery(
        Agent::new(config.clone(), vec![]),
        host,
        request,
        config,
        sink,
        support::policy(3, 4),
        CancellationToken::new(),
        LengthRecoveryPolicy { max_continuations: NonZeroUsize::new(1).unwrap() },
    )?
    .wait()
    .await?;
    persist(
        &root.join("result.json"),
        &json!({"stop":format!("{:?}",outcome.stop),"attempts":outcome.attempts.len(),
            "prepared_requests":outcome.prepared_requests,"reconciliation_required":outcome.reconciliation.is_some(),
        "provider_requests":upstream.served(),"task_accepted":false,"provider":"controlled HTTP"}),
    )?;
    Ok(outcome)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args_os().nth(1).map(PathBuf::from).ok_or("expected new evidence directory")?;
    fs::create_dir(&root)?;
    let result = run(&root).await?;
    println!("{:?}; {} requests; evidence={}", result.stop, result.prepared_requests, root.display());
    if result.stop != WorkflowStop::VerifiedArtifact {
        return Err("artifact was not verified".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn interrupted_write_is_settled_then_same_session_continues_without_replay() {
        let dir = tempfile::tempdir().unwrap();
        let result = run(dir.path()).await.unwrap();
        assert_eq!(result.stop, WorkflowStop::VerifiedArtifact, "{result:#?}");
        assert_eq!(result.attempts.len(), 2);
        assert_eq!(result.prepared_requests, 4);
        assert!(result.reconciliation.is_none());
        assert!(result.attempts[0].review.is_none());
        assert!(result.attempts[0].length_settlement.as_ref().unwrap().is_ok());
        assert!(result.attempts[1].length_settlement.is_none());
        assert_eq!(fs::read(dir.path().join("answer.txt")).unwrap(), b"correct");
        let first: Value =
            serde_json::from_slice(&fs::read(dir.path().join("length-settlement-1.json")).unwrap()).unwrap();
        let last: Value = serde_json::from_slice(&fs::read(dir.path().join("review-2.json")).unwrap()).unwrap();
        assert_eq!(first["artifact_sha256"], sha(b"partial"));
        assert_eq!(last["artifact_sha256"], sha(b"correct"));
        assert_eq!(first["exit_code"], 0);
        assert_eq!(last["exit_code"], 0);
        let reopened = SessionJournal::open(Path::new(last["journal"].as_str().unwrap())).unwrap();
        let writes = reopened.entries().iter().filter(|e| e.raw["message"]["role"] == "toolResult").count();
        assert_eq!(writes, 2, "one partial write and one repair; no replay");
        let feedback = result.attempts[1]
            .core_result
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .messages
            .iter()
            .find(|m| m.source().is_some())
            .unwrap();
        assert_eq!(feedback.source().unwrap()["kind"], "ara-workflow-length-continuation");
        assert_eq!(feedback.source().unwrap()["basis"], serde_json::to_value(&result.attempts[0].basis).unwrap());
        let sidecars = fs::read_dir(dir.path().join("session"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with("source-"))
            .collect::<Vec<_>>();
        assert_eq!(sidecars.len(), 1);
        let saved: Value = serde_json::from_slice(&fs::read(sidecars[0].path()).unwrap()).unwrap();
        assert_eq!(saved["source"], *feedback.source().unwrap());
        let entry = reopened.entries().iter().find(|e| e.id == saved["entry"].as_str().unwrap()).unwrap();
        assert_eq!(entry.raw["message"], feedback.json().unwrap());
    }
}
