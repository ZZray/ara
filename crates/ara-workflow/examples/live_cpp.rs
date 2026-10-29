//! Opt-in real-model smoke task through the public workflow API.
//! This small fixed-point parser task does not replace the multi-file S3 trial.
use ara_agent::{
    Agent, AgentConfig, AgentEvent, AgentEventSink, AgentInput, AgentTool, Concurrency, InterruptMode, NoHooks,
    RecordingSink, ToolError, ToolOutput, UpdateFn,
};
use ara_ai::{JsonObject, Message, Model, OpenAICompletionsProvider, Tool, UserMessage};
use ara_session::SessionJournal;
use ara_workflow::*;
use async_trait::async_trait;
use ring::digest::{SHA256, digest};
use serde_json::json;
use std::{
    fs,
    num::NonZeroUsize,
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

const SEED: &str = r#"#include <cstdint>
#include <string>
bool parse_cents(const std::string& s, std::int64_t& value) {
  try { value = static_cast<std::int64_t>(std::stod(s) * 100); return true; }
  catch (...) { return false; }
}
"#;
const RUNNER: &str = r#"#include <cstdint>
#include <iostream>
#include <string>
bool parse_cents(const std::string&, std::int64_t&);
int main(int argc, char** argv) {
  if (argc != 2) return 2;
  std::int64_t result = 713;
  if (parse_cents(argv[1], result)) std::cout << "OK:" << result;
  else std::cout << "ERR:" << result;
}
"#;
const SPEC: &str = "Repair parse_cents in amount.cpp. Parse exact decimal cents without floating point. Grammar: optional ASCII + or -, at least one ASCII digit, optionally a dot followed by one or two digits. No whitespace, exponent, trailing characters, or other bytes. Scale one fractional digit by ten. Accept the entire signed int64 cents range; overflow is invalid. Leading zeros and negative zero are valid. On failure leave the output unchanged. Use read_source, replace_source, check_cpp. check_cpp checks only the public examples; the Host checks the complete contract after each submission. Preserve the function signature. Stop after submitting a candidate; Host feedback may request another attempt.";
const CASES: &[(&str, &str)] = &[
    ("0", "OK:0"),
    ("1.20", "OK:120"),
    ("-3.4", "OK:-340"),
    ("12x", "ERR:713"),
    ("1.234", "ERR:713"),
    ("+0", "OK:0"),
    ("-0.00", "OK:0"),
    ("0003.05", "OK:305"),
    ("0.29", "OK:29"),
    ("-0.29", "OK:-29"),
    ("+7.1", "OK:710"),
    ("", "ERR:713"),
    ("+", "ERR:713"),
    ("-", "ERR:713"),
    (".5", "ERR:713"),
    ("1.", "ERR:713"),
    (" 1", "ERR:713"),
    ("1 ", "ERR:713"),
    ("1e2", "ERR:713"),
    ("--1", "ERR:713"),
    ("nan", "ERR:713"),
    ("1,2", "ERR:713"),
    ("１２", "ERR:713"),
    ("92233720368547758.07", "OK:9223372036854775807"),
    ("-92233720368547758.08", "OK:-9223372036854775808"),
    ("92233720368547758.08", "ERR:713"),
    ("-92233720368547758.09", "ERR:713"),
    ("92233720368547759", "ERR:713"),
    ("000000000000000000000000000000000000000000000000000001.00", "OK:100"),
];

fn sha(bytes: &[u8]) -> String {
    digest(&SHA256, bytes).as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

struct Host {
    root: PathBuf,
    compiler: PathBuf,
    sequence: AtomicUsize,
    sink: Option<Arc<JournalSink>>,
}

struct JournalSink {
    journal: Mutex<SessionJournal>,
    failure: Mutex<Option<String>>,
    recording: RecordingSink,
    entries: Mutex<Vec<(ara_agent::AgentMessage, String)>>,
}

#[async_trait]
impl AgentEventSink for JournalSink {
    async fn emit(&self, event: AgentEvent) {
        if let AgentEvent::MessageEnd { message } = &event {
            let result = (|| -> Result<(), String> {
                let body = message.json().map_err(|e| e.to_string())?;
                let mut journal = self.journal.lock().unwrap();
                let entry = journal.append_agent_message_body(&body).map_err(|e| e.to_string())?;
                if let Some(source) = message.source() {
                    let path = journal.path().with_file_name(format!("provenance-{entry}.json"));
                    fs::write(
                        &path,
                        serde_json::to_vec_pretty(&json!({"entry":entry,"source":source.as_ref()})).unwrap(),
                    )
                    .map_err(|e| e.to_string())?;
                    fs::OpenOptions::new()
                        .write(true)
                        .open(path)
                        .and_then(|f| f.sync_all())
                        .map_err(|e| e.to_string())?;
                }
                self.entries.lock().unwrap().push((message.clone(), entry));
                Ok(())
            })();
            if let Err(error) = result {
                *self.failure.lock().unwrap() = Some(error);
            }
        }
        self.recording.emit(event).await;
    }
}

impl Host {
    // The child is retained, killed when bounded time elapses, and reaped.
    // Raw stdout/stderr remain in the caller's explicit evidence directory.
    async fn process(
        &self,
        program: &std::path::Path,
        args: &[String],
        seconds: u64,
        cancel: &CancellationToken,
    ) -> Result<(bool, String), String> {
        if cancel.is_cancelled() {
            return Err("cancelled before process admission".into());
        }
        let n = self.sequence.fetch_add(1, Ordering::SeqCst);
        let out = self.root.join(format!("process-{n}.stdout"));
        let err = self.root.join(format!("process-{n}.stderr"));
        let mut child = tokio::process::Command::new(program)
            .args(args)
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .stdout(Stdio::from(fs::File::create(&out).map_err(|e| e.to_string())?))
            .stderr(Stdio::from(fs::File::create(&err).map_err(|e| e.to_string())?))
            .spawn()
            .map_err(|e| e.to_string())?;
        let started = json!({"pid":child.id(),"program":program,"argv":args});
        // Once spawned, even a receipt-write failure must retain the child and
        // reap it. Record the failure only after the original child settles.
        let start_write = fs::write(
            self.root.join(format!("process-{n}.started.json")),
            serde_json::to_vec_pretty(&started).unwrap(),
        );
        let end = Instant::now() + Duration::from_secs(seconds);
        let mut timed_out = false;
        let mut cancelled = false;
        let status = loop {
            if let Some(status) = child.try_wait().map_err(|e| format!("effect settlement unknown: {e}"))? {
                break status;
            }
            if Instant::now() >= end || cancel.is_cancelled() {
                cancelled = cancel.is_cancelled();
                timed_out = !cancelled;
                child.kill().await.map_err(|e| format!("effect settlement unknown: {e}"))?;
                break child.wait().await.map_err(|e| format!("effect settlement unknown: {e}"))?;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        let stdout = fs::read_to_string(out).map_err(|e| e.to_string())?;
        let stderr = fs::read_to_string(err).map_err(|e| e.to_string())?;
        start_write.map_err(|e| format!("settled process but start receipt write failed: {e}"))?;
        let receipt = json!({"program":program,"argv":args,"exit":status.code(),"timed_out":timed_out,"cancelled":cancelled,"stdout":stdout,"stderr":stderr});
        fs::write(self.root.join(format!("process-{n}.json")), serde_json::to_vec_pretty(&receipt).unwrap())
            .map_err(|e| e.to_string())?;
        if cancelled {
            return Err("cancelled after original process was killed and reaped".into());
        }
        Ok((status.success() && !timed_out, format!("{stdout}{stderr}")))
    }

    async fn check(&self, count: usize, cancel: &CancellationToken) -> Result<String, String> {
        if fs::read(self.root.join("runner.cpp")).map_err(|e| e.to_string())? != RUNNER.as_bytes() {
            return Err("protected runner changed".into());
        }
        let args = ["-std=c++17", "-O2", "-Wall", "-Wextra", "amount.cpp", "runner.cpp", "-o", "amount-check.exe"]
            .map(str::to_owned);
        let (ok, output) = self.process(&self.compiler, &args, 30, cancel).await?;
        if !ok {
            return Ok(format!("compile failed: {output}"));
        }
        let mut failures = Vec::new();
        for (input, expected) in CASES.iter().take(count) {
            let (ok, actual) =
                self.process(&self.root.join("amount-check.exe"), &[(*input).to_owned()], 2, cancel).await?;
            if !ok || actual != *expected {
                failures.push(format!("input={input:?}; expected={expected}; actual={actual:?}; process_success={ok}"));
            }
        }
        Ok(if failures.is_empty() { format!("PASS:{count}") } else { failures.join("\n") })
    }
}

struct WorkspaceTool {
    name: &'static str,
    host: Arc<Host>,
}
#[async_trait]
impl AgentTool for WorkspaceTool {
    fn definition(&self) -> Tool {
        Tool {
            name: self.name.into(),
            description: match self.name {
                "read_source" => "Read amount.cpp",
                "replace_source" => "Replace only amount.cpp with complete C++ source",
                _ => "Compile and run five public examples",
            }
            .into(),
            parameters: if self.name == "replace_source" {
                json!({"type":"object","properties":{"source":{"type":"string"}},"required":["source"],"additionalProperties":false})
            } else {
                json!({"type":"object","properties":{},"additionalProperties":false})
            },
            ..Default::default()
        }
    }
    fn concurrency(&self, _: &JsonObject) -> Concurrency {
        Concurrency::Exclusive
    }
    async fn execute(
        &self,
        _: &str,
        args: JsonObject,
        cancel: CancellationToken,
        _: UpdateFn,
    ) -> Result<ToolOutput, ToolError> {
        if cancel.is_cancelled() {
            return Err(ToolError("cancelled before admission".into()));
        }
        let result = match self.name {
            "read_source" => fs::read_to_string(self.host.root.join("amount.cpp")).map_err(|e| e.to_string())?,
            "replace_source" => {
                let source = args.get("source").and_then(|v| v.as_str()).ok_or("missing source")?;
                if source.len() > 32_768 {
                    return Err(ToolError("source too large".into()));
                }
                fs::write(self.host.root.join("amount.cpp"), source)?;
                format!("saved sha256={}", sha(source.as_bytes()))
            }
            _ => self.host.check(5, &cancel).await?,
        };
        Ok(ToolOutput::text(result))
    }
}

#[async_trait]
impl WorkflowHost for Host {
    async fn review(
        &self,
        candidate: &ReviewCandidate,
        cancel: &CancellationToken,
    ) -> Result<ReviewReceipt, ReviewError> {
        let sink = self.sink.as_ref().ok_or_else(|| ReviewError("missing Session owner".into()))?;
        if let Some(error) = sink.failure.lock().unwrap().clone() {
            return Err(ReviewError(error));
        }
        let journal_path = sink.journal.lock().unwrap().path().to_path_buf();
        fs::OpenOptions::new()
            .write(true)
            .open(&journal_path)
            .and_then(|f| f.sync_all())
            .map_err(|e| ReviewError(e.to_string()))?;
        let reopened = SessionJournal::open(&journal_path).map_err(|e| ReviewError(e.to_string()))?;
        if reopened.session_id() != candidate.basis.workflow.session_id {
            return Err(ReviewError("Session receipt identity mismatch".into()));
        }
        // Reopen skips corrupt entries by design, so header identity alone is
        // insufficient. Match every current Run record to its exact append ID.
        for message in &candidate.run.messages {
            let entries = sink.entries.lock().unwrap();
            let (_, id) = entries
                .iter()
                .find(|(record, _)| record.same_record(message))
                .ok_or_else(|| ReviewError("Run message has no journal append receipt".into()))?;
            let persisted = reopened
                .entries()
                .iter()
                .find(|entry| &entry.id == id)
                .ok_or_else(|| ReviewError("journal reopen lost an appended Run record".into()))?;
            if persisted.raw["message"] != message.json().map_err(|e| ReviewError(e.to_string()))? {
                return Err(ReviewError("canonical Run record differs from persisted body".into()));
            }
            if let Some(source) = message.source() {
                let path = journal_path.with_file_name(format!("provenance-{id}.json"));
                let saved: serde_json::Value =
                    serde_json::from_slice(&fs::read(path).map_err(|e| ReviewError(e.to_string()))?)
                        .map_err(|e| ReviewError(e.to_string()))?;
                if saved["source"] != *source {
                    return Err(ReviewError("input provenance differs from append receipt".into()));
                }
            }
        }
        let bytes = fs::read(self.root.join("amount.cpp")).map_err(|e| ReviewError(e.to_string()))?;
        let revision = sha(&bytes);
        let attempt = candidate.basis.attempt;
        fs::write(self.root.join(format!("attempt-{attempt}.cpp")), &bytes).map_err(|e| ReviewError(e.to_string()))?;
        fs::OpenOptions::new()
            .write(true)
            .open(self.root.join(format!("attempt-{attempt}.cpp")))
            .and_then(|f| f.sync_all())
            .map_err(|e| ReviewError(e.to_string()))?;
        let check = self.check(CASES.len(), cancel).await;
        let stable = fs::read(self.root.join("amount.cpp")).map_err(|e| ReviewError(e.to_string()))? == bytes
            && fs::read(self.root.join("runner.cpp")).map_err(|e| ReviewError(e.to_string()))? == RUNNER.as_bytes();
        let (settlement, verdict) = match check {
            Ok(output) if stable => (
                Settlement::Settled,
                if output == format!("PASS:{}", CASES.len()) {
                    ReviewVerdict::Verified
                } else {
                    ReviewVerdict::NeedsChanges { feedback: output }
                },
            ),
            Ok(_) => (
                Settlement::Unknown { reason: "source changed during review".into() },
                ReviewVerdict::Unknown { reason: "source changed during review".into() },
            ),
            Err(reason) => (Settlement::Unknown { reason: reason.clone() }, ReviewVerdict::Unknown { reason }),
        };
        let evidence = self.root.join(format!("review-{attempt}.json"));
        fs::write(&evidence, serde_json::to_vec_pretty(&json!({"basis":candidate.basis,"sha256":revision,"verdict":format!("{verdict:?}"),"settlement":format!("{settlement:?}"),"cases":CASES.len()})).unwrap()).map_err(|e| ReviewError(e.to_string()))?;
        fs::OpenOptions::new()
            .write(true)
            .open(&evidence)
            .and_then(|f| f.sync_all())
            .map_err(|e| ReviewError(e.to_string()))?;
        Ok(ReviewReceipt {
            basis: candidate.basis.clone(),
            artifacts: vec![ArtifactEvidence {
                artifact_id: "amount.cpp".into(),
                revision,
                evidence_ref: evidence.to_string_lossy().into_owned(),
            }],
            settlement,
            commit: ReceiptCommit::Committed { receipt_id: format!("cpp-review-{attempt}") },
            verdict,
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().nth(1).as_deref() != Some("--run-live") {
        println!(
            "Opt-in: --run-live NEW_EVIDENCE_DIRECTORY; set ARA_CPP_COMPILER, ARA_MODEL_BASE_URL, ARA_MODEL_ID, ARA_MODEL_API_KEY. This sends up to 16 logical requests and modifies only the new directory."
        );
        return Ok(());
    }
    let root = PathBuf::from(std::env::args().nth(2).ok_or("missing new evidence directory")?);
    if !root.is_absolute() {
        return Err("evidence directory must be absolute".into());
    }
    let compiler = PathBuf::from(std::env::var("ARA_CPP_COMPILER")?);
    if !compiler.is_absolute() || !compiler.is_file() {
        return Err("compiler must be an existing absolute executable path".into());
    }
    let model_id = std::env::var("ARA_MODEL_ID")?;
    let base_url = std::env::var("ARA_MODEL_BASE_URL")?;
    let key = std::env::var("ARA_MODEL_API_KEY")?;
    fs::create_dir(&root)?;
    fs::write(root.join("amount.cpp"), SEED)?;
    fs::write(root.join("runner.cpp"), RUNNER)?;
    fs::write(root.join("task.txt"), SPEC)?;
    fs::write(
        root.join("trial-plan.json"),
        serde_json::to_vec_pretty(&json!({
            "scope":"small public API smoke task; original S3 acceptance unchanged",
            "requested_model":model_id,"api":"openai-completions","base_url":base_url,
            "logical_request_limit":16,"attempt_limit":3,"deadline_seconds":300,
            "max_tokens":16384,"reasoning_override":"provider_default_no_override",
            "seed_sha256":sha(SEED.as_bytes()),"runner_sha256":sha(RUNNER.as_bytes()),
            "spec_sha256":sha(SPEC.as_bytes()),"oracle_sha256":sha(&serde_json::to_vec(CASES)?),
            "public_examples":5,"host_cases":CASES.len(),"task_accepted":false
        }))?,
    )?;
    let journal = SessionJournal::create(&root.join("session"), &root)?;
    let session_id = journal.session_id().to_owned();
    let sink = Arc::new(JournalSink {
        journal: Mutex::new(journal),
        failure: Mutex::new(None),
        recording: RecordingSink::default(),
        entries: Mutex::new(Vec::new()),
    });
    let host = Arc::new(Host { root: root.clone(), compiler, sequence: AtomicUsize::new(0), sink: Some(sink.clone()) });
    // Establish the actual seed failure before spending model calls.
    let seed_check = host.check(CASES.len(), &CancellationToken::new()).await?;
    fs::write(root.join("seed-check.txt"), &seed_check)?;
    if seed_check.starts_with("PASS:") {
        return Err("seed unexpectedly passes".into());
    }
    let provider = Arc::new(OpenAICompletionsProvider {
        client: reqwest::Client::new(),
        base: ara_ai::providers::openai_completions::StreamOptions { api_key: Some(key), ..Default::default() },
    });
    let config = AgentConfig { model:Model { id:model_id, api:"openai-completions".into(), provider:"explicit-host-route".into(), base_url,
        reasoning:true, max_tokens:Some(16384), context_window:None, tokenizer:None }, provider,
        system_prompt:vec!["Use the supplied tools to repair the C++ implementation. Treat checks as evidence. Do not claim Task acceptance.".into()],
        tools:["read_source", "replace_source", "check_cpp"].into_iter().map(|name| Arc::new(WorkspaceTool { name, host:host.clone() }) as Arc<dyn AgentTool>).collect(),
        tool_choice:None, max_tokens:Some(16384), temperature:None, deadline:None, max_model_calls:Some(16), hooks:Arc::new(NoHooks), interrupt_mode:InterruptMode::Immediate };
    let agent = Agent::new(config.clone(), vec![]);
    let outcome = Workflow::start(
        agent,
        host,
        WorkflowRequest {
            basis: WorkflowBasis {
                workflow_id: format!("cpp-{}", uuid::Uuid::new_v4()),
                session_id,
                session_generation: "1".into(),
                criteria_revision: sha(SPEC.as_bytes()),
                artifact_evidence_basis: sha(SEED.as_bytes()),
            },
            inputs: vec![AgentInput::from(Message::User(UserMessage::text(SPEC)))],
        },
        config,
        sink.clone(),
        WorkflowPolicy {
            max_attempts: NonZeroUsize::new(3).unwrap(),
            max_prepared_requests: NonZeroUsize::new(16).unwrap(),
            deadline: Instant::now() + Duration::from_secs(300),
        },
        CancellationToken::new(),
    )?
    .wait()
    .await?;
    fs::write(root.join("agent-events.txt"), format!("{:#?}", sink.recording.events.lock().await))?;
    fs::write(root.join("workflow-outcome.txt"), format!("{outcome:#?}"))?;
    println!(
        "stop={:?}; attempts={}; logical_requests={}; evidence={}",
        outcome.stop,
        outcome.attempts.len(),
        outcome.prepared_requests,
        root.display()
    );
    if outcome.stop != WorkflowStop::VerifiedArtifact {
        return Err("artifact not verified; Task remains unaccepted".into());
    }
    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancellation_before_admission_starts_no_process() {
        let dir = tempfile::tempdir().unwrap();
        let host =
            Host { root: dir.path().into(), compiler: PathBuf::new(), sequence: AtomicUsize::new(0), sink: None };
        let cancel = CancellationToken::new();
        cancel.cancel();
        let result = host.process(std::path::Path::new("powershell.exe"), &[], 30, &cancel).await;
        assert!(result.is_err());
        assert_eq!(host.sequence.load(Ordering::SeqCst), 0);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn cancellation_reaps_original_child_and_records_it() {
        let dir = tempfile::tempdir().unwrap();
        let host = Arc::new(Host {
            root: dir.path().into(),
            compiler: PathBuf::new(),
            sequence: AtomicUsize::new(0),
            sink: None,
        });
        let cancel = CancellationToken::new();
        let worker_host = host.clone();
        let worker_cancel = cancel.clone();
        let worker = tokio::spawn(async move {
            worker_host
                .process(
                    std::path::Path::new("powershell.exe"),
                    &[
                        "-NoProfile".into(),
                        "-NonInteractive".into(),
                        "-Command".into(),
                        "Start-Sleep -Seconds 20".into(),
                    ],
                    30,
                    &worker_cancel,
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while !dir.path().join("process-0.started.json").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        cancel.cancel();
        let result = tokio::time::timeout(Duration::from_secs(5), worker).await.unwrap().unwrap();
        assert!(result.unwrap_err().contains("killed and reaped"));
        let receipt: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.path().join("process-0.json")).unwrap()).unwrap();
        assert_eq!(receipt["cancelled"], true);
        assert_eq!(receipt["timed_out"], false);
        assert_eq!(host.sequence.load(Ordering::SeqCst), 1);
    }
}
