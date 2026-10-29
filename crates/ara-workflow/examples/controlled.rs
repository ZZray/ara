//! Local deterministic demonstration of real Core, WriteTool, SessionJournal,
//! Host feedback, and a joined native process. No remote model is contacted.
#[path = "../tests/support/mod.rs"]
mod support;
use ara_agent::{Agent, AgentEvent, AgentEventSink, AgentTool};
use ara_session::SessionJournal;
use ara_tools::{ToolContext, write::WriteTool};
use ara_workflow::*;
use async_trait::async_trait;
use ring::digest::{SHA256, digest};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

struct JournalSink {
    journal: Mutex<SessionJournal>,
    failure: Mutex<Option<String>>,
    entries: Mutex<Vec<(ara_agent::AgentMessage, String)>>,
}

#[async_trait]
impl AgentEventSink for JournalSink {
    async fn emit(&self, event: AgentEvent) {
        if let AgentEvent::MessageEnd { message } = event {
            let result = (|| -> Result<(), String> {
                let body = message.json().map_err(|error| error.to_string())?;
                let mut journal = self.journal.lock().unwrap();
                let entry = journal.append_agent_message_body(&body).map_err(|error| error.to_string())?;
                if let Some(source) = message.source() {
                    let path = journal.path().with_file_name(format!("provenance-{entry}.json"));
                    fs::write(
                        &path,
                        serde_json::to_vec_pretty(&json!({"entry":entry,"source":source.as_ref()})).unwrap(),
                    )
                    .map_err(|error| error.to_string())?;
                    fs::OpenOptions::new()
                        .write(true)
                        .open(path)
                        .and_then(|file| file.sync_all())
                        .map_err(|error| error.to_string())?;
                }
                self.entries.lock().unwrap().push((message.clone(), entry));
                Ok(())
            })();
            if let Err(error) = result {
                *self.failure.lock().unwrap() = Some(error);
            }
        }
    }
}

struct FileHost {
    root: PathBuf,
    sink: Arc<JournalSink>,
}
#[async_trait]
impl WorkflowHost for FileHost {
    async fn review(&self, candidate: &ReviewCandidate, _: &CancellationToken) -> Result<ReviewReceipt, ReviewError> {
        if let Some(error) = self.sink.failure.lock().unwrap().clone() {
            return Err(ReviewError(error));
        }
        let before = fs::read(self.root.join("answer.txt")).map_err(|error| ReviewError(error.to_string()))?;
        // The original child is awaited. The SDK also retains this callback
        // through cancellation, so its process cannot be discarded by a waiter.
        #[cfg(windows)]
        let output = tokio::process::Command::new("cmd.exe")
            .args(["/d", "/c", "type", "answer.txt"])
            .current_dir(&self.root)
            .output()
            .await;
        #[cfg(not(windows))]
        let output = tokio::process::Command::new("cat").arg("answer.txt").current_dir(&self.root).output().await;
        let output = output.map_err(|error| ReviewError(error.to_string()))?;
        let after = fs::read(self.root.join("answer.txt")).map_err(|error| ReviewError(error.to_string()))?;
        let journal_path = self.sink.journal.lock().unwrap().path().to_path_buf();
        fs::OpenOptions::new()
            .write(true)
            .open(&journal_path)
            .and_then(|file| file.sync_all())
            .map_err(|error| ReviewError(error.to_string()))?;
        let reopened = SessionJournal::open(&journal_path).map_err(|error| ReviewError(error.to_string()))?;
        // Reopening tolerates corrupt entries, so verify each actual Run record
        // against the exact append ID and its separately persisted provenance.
        for message in &candidate.run.messages {
            let entries = self.sink.entries.lock().unwrap();
            let (_, id) = entries
                .iter()
                .find(|(record, _)| record.same_record(message))
                .ok_or_else(|| ReviewError("Run message has no journal append receipt".into()))?;
            let persisted = reopened
                .entries()
                .iter()
                .find(|entry| &entry.id == id)
                .ok_or_else(|| ReviewError("journal reopen lost an appended Run record".into()))?;
            if persisted.raw["message"] != message.json().map_err(|error| ReviewError(error.to_string()))? {
                return Err(ReviewError("canonical Run record differs from persisted body".into()));
            }
            if let Some(source) = message.source() {
                let path = journal_path.with_file_name(format!("provenance-{id}.json"));
                let saved: serde_json::Value =
                    serde_json::from_slice(&fs::read(path).map_err(|error| ReviewError(error.to_string()))?)
                        .map_err(|error| ReviewError(error.to_string()))?;
                if saved["entry"] != *id || saved["source"] != *source {
                    return Err(ReviewError("input provenance differs from its journal append receipt".into()));
                }
            }
        }
        let stable = before == after
            && output.status.success()
            && output.stdout == before
            && reopened.session_id() == candidate.basis.workflow.session_id;
        let verdict = if !stable {
            ReviewVerdict::Unknown { reason: "artifact/process/journal mismatch".into() }
        } else if before == b"correct" {
            ReviewVerdict::Verified
        } else {
            ReviewVerdict::NeedsChanges {
                feedback: "The sealed answer is incorrect. Write exactly correct to answer.txt, then submit again."
                    .into(),
            }
        };
        let evidence = self.root.join(format!("review-{}.json", candidate.basis.attempt));
        let revision = digest(&SHA256, &before).as_ref().iter().map(|byte| format!("{byte:02x}")).collect::<String>();
        let raw = json!({"basis":candidate.basis,"artifact_sha256":revision,"process_exit":output.status.code(),"stdout":String::from_utf8_lossy(&output.stdout),"stderr":String::from_utf8_lossy(&output.stderr),"journal":journal_path,"stable":stable,"verdict":format!("{verdict:?}")});
        fs::write(&evidence, serde_json::to_vec_pretty(&raw).unwrap())
            .map_err(|error| ReviewError(error.to_string()))?;
        fs::OpenOptions::new()
            .write(true)
            .open(&evidence)
            .and_then(|file| file.sync_all())
            .map_err(|error| ReviewError(error.to_string()))?;
        Ok(ReviewReceipt {
            basis: candidate.basis.clone(),
            artifacts: vec![ArtifactEvidence {
                artifact_id: "answer.txt".into(),
                revision,
                evidence_ref: evidence.to_string_lossy().into_owned(),
            }],
            settlement: if stable {
                Settlement::Settled
            } else {
                Settlement::Unknown { reason: "artifact/process/journal mismatch".into() }
            },
            commit: ReceiptCommit::Committed { receipt_id: format!("review-{}", candidate.basis.attempt) },
            verdict,
        })
    }
}

async fn run(root: &Path) -> Result<Arc<WorkflowOutcome>, Box<dyn std::error::Error>> {
    use support::*;
    let upstream = server(vec![write("wrong"), text("stop"), write("correct"), text("stop")]).await;
    let journal = SessionJournal::create(&root.join("session"), root)?;
    let mut identity = basis();
    identity.session_id = journal.session_id().to_owned();
    let sink =
        Arc::new(JournalSink { journal: Mutex::new(journal), failure: Mutex::new(None), entries: Mutex::new(vec![]) });
    let host = Arc::new(FileHost { root: root.into(), sink: sink.clone() });
    let tool: Arc<dyn AgentTool> = Arc::new(WriteTool::new(ToolContext::new(root)));
    let cfg = config(&upstream, vec![tool]);
    let agent = Agent::new(cfg.clone(), vec![]);
    let mut input = request();
    input.basis = identity;
    let handle = Workflow::start(agent, host, input, cfg, sink.clone(), policy(3, 6), CancellationToken::new())?;
    let outcome = handle.wait().await?;
    fs::write(root.join("http-requests.json"), serde_json::to_vec_pretty(&*upstream.requests.lock().await)?)?;
    fs::write(root.join("outcome.txt"), format!("{outcome:#?}"))?;
    Ok(outcome)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("ara-workflow-{}", uuid::Uuid::new_v4())));
    fs::create_dir(&root)?;
    let outcome = run(&root).await?;
    println!(
        "{:?}; {} attempts; {} logical requests; evidence={}",
        outcome.stop,
        outcome.attempts.len(),
        outcome.prepared_requests,
        root.display()
    );
    if outcome.stop != WorkflowStop::VerifiedArtifact {
        return Err("artifact was not verified".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn actual_write_journal_and_process_repair_one_agent_artifact() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = run(dir.path()).await.unwrap();
        assert_eq!(outcome.stop, WorkflowStop::VerifiedArtifact);
        assert_eq!(outcome.attempts.len(), 2);
        assert_eq!(outcome.prepared_requests, 4);
        assert_eq!(fs::read(dir.path().join("answer.txt")).unwrap(), b"correct");
        assert!(outcome.reconciliation.is_none());
        let first: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.path().join("review-1.json")).unwrap()).unwrap();
        let last: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.path().join("review-2.json")).unwrap()).unwrap();
        assert_eq!(first["stdout"], "wrong");
        assert_eq!(last["stdout"], "correct");
        assert_eq!(last["stable"], true);
        assert_eq!(last["process_exit"], 0);
        assert!(
            SessionJournal::open(Path::new(last["journal"].as_str().unwrap())).unwrap().build_context().unwrap().len()
                >= 6
        );
        let reopened = SessionJournal::open(Path::new(last["journal"].as_str().unwrap())).unwrap();
        let sidecars = fs::read_dir(dir.path().join("session"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.file_name().unwrap().to_string_lossy().starts_with("provenance-"))
            .collect::<Vec<_>>();
        assert_eq!(sidecars.len(), 1, "one committed repair feedback input retains its original source");
        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&sidecars[0]).unwrap()).unwrap();
        assert_eq!(saved["source"]["kind"], "ara-workflow-review");
        assert_eq!(saved["source"]["receipt_id"], "review-1");
        assert_eq!(saved["source"]["basis"]["attempt"], 1);
        assert_eq!(saved["source"]["basis"], serde_json::to_value(&outcome.attempts[0].basis).unwrap());
        assert_eq!(saved["source"]["basis"], first["basis"]);
        assert_eq!(
            saved["source"]["artifacts"],
            serde_json::to_value(&outcome.attempts[0].review.as_ref().unwrap().as_ref().unwrap().artifacts).unwrap()
        );
        let entry = reopened.entries().iter().find(|entry| entry.id == saved["entry"].as_str().unwrap()).unwrap();
        assert_eq!(entry.raw["message"]["role"], "user");
        let feedback = outcome.attempts[1]
            .core_result
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .messages
            .iter()
            .find(|record| record.source().is_some())
            .unwrap();
        assert_eq!(entry.raw["message"], feedback.json().unwrap());
        assert_eq!(saved["source"], *feedback.source().unwrap());
        assert!(entry.raw["message"]["content"].to_string().contains("The sealed answer is incorrect"));
    }
}
