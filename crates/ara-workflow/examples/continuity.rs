//! Two-process Host example for sourced soft compaction and knowledge retrieval.
//! Core, Session storage and product acceptance remain separate owners.
#[path = "continuity/host.rs"]
mod host;
#[path = "continuity/session_bridge.rs"]
mod session_bridge;
#[path = "../tests/support/mod.rs"]
mod support;

use ara_agent::compaction::{SummarySource, summarize_sources_with_instructions};
use ara_agent::{Agent, AgentConfig, AgentInput, InterruptMode, NoHooks, RunEnd};
use ara_ai::{
    AssistantMessage, AssistantMessageEvent, AssistantStream, CallOptions, Context, Message, Model, ModelProvider,
    OpenAICompletionsProvider, StopReason, UserMessage,
};
use ara_session::{CompactedContextItem, SessionJournal};
use ara_workflow::*;
use host::{JournalSink, ResumeHost, save, sha};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    error::Error,
    fs,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio_util::sync::CancellationToken;

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const LIMIT: usize = 16;
const FIRST: &str = "Prepare for a later project task. The result object initially uses key ALPHA. Project knowledge is indexed at knowledge/INDEX.md, but do not read it or implement yet. Acknowledge and wait for revised constraints.";
const CORRECTION: &str = "Correction: the result key must be beta_case; ALPHA is obsolete. Keep sentinel.txt unchanged. When implementation is requested, retrieve the token from the note linked in the project knowledge index and write result.json with exactly the fields key and token. Do not implement yet; acknowledge this correction.";
const KEPT: &str =
    "The preparatory discussion is complete. Implementation and project knowledge retrieval are still pending.";
const RESUME: &str = "Continue the pending project task using the current corrected constraints from the restored context. Read the project knowledge index and its linked note, then write result.json with exactly the fields key and token. The read_source_entry tool can retrieve original user references when needed. Submit after writing; the Host will verify the actual file.";
const FOCUS: &str = "Preserve the latest user correction, superseded constraints, protected files, knowledge index location and unperformed next action. Preserve source entry references. Do not invent knowledge-note contents. This is a derived summary, not a new user instruction.";

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
}

fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

#[derive(Serialize, Deserialize)]
struct Prepared {
    session_id: String,
    journal: PathBuf,
    journal_sha256: String,
    summary_entry_id: String,
    source_entry_ids: Vec<String>,
    correction_entry_id: String,
    kept_entry_id: String,
    protected: BTreeMap<String, String>,
    provenance_pins: BTreeMap<PathBuf, String>,
    model_id: String,
    route_identity: String,
    deadline_ms: u64,
    reserved: usize,
    logical_limit: usize,
    effect_state: String,
}

struct BudgetProvider {
    inner: Arc<dyn ModelProvider>,
    used: AtomicUsize,
    root: PathBuf,
    deadline_ms: u64,
}

impl ModelProvider for BudgetProvider {
    fn stream(&self, model: &Model, context: &Context, options: CallOptions) -> AssistantStream {
        let reservation = if now_ms() < self.deadline_ms && !options.cancel.is_cancelled() {
            self.used.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| (n < LIMIT).then_some(n + 1)).ok()
        } else {
            None
        };
        if let Some(previous) = reservation {
            let path = self.root.join(format!("provider-reservation-{}.json", previous + 1));
            let result = (|| -> Result<()> {
                let mut file = fs::OpenOptions::new().write(true).create_new(true).open(path)?;
                use std::io::Write;
                file.write_all(&serde_json::to_vec(&json!({"reservation":previous+1,"model":model.id,
                    "at_ms":now_ms(),"deadline_ms":self.deadline_ms,"logical_not_http_retries":true}))?)?;
                file.sync_all()?;
                Ok(())
            })();
            if result.is_ok() {
                return self.inner.stream(model, context, options);
            }
        }
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        let mut error = AssistantMessage::empty(&model.api, &model.provider, &model.id);
        error.stop_reason = StopReason::Error;
        error.error_message = Some("Host request budget, deadline or reservation receipt rejected admission".into());
        let _ = sender.try_send(AssistantMessageEvent::Error { reason: StopReason::Error, error });
        receiver
    }
}

fn remaining_deadline(deadline_ms: u64) -> Result<Instant> {
    let remaining =
        deadline_ms.checked_sub(now_ms()).filter(|ms| *ms > 0).ok_or("original absolute deadline expired")?;
    Ok(Instant::now() + Duration::from_millis(remaining))
}

fn route_identity(live: bool) -> Result<String> {
    if live {
        let value = std::env::var("ARA_MODEL_BASE_URL")?;
        let url = reqwest::Url::parse(&value)?;
        if !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() {
            return Err("route must not contain credentials, query or fragment".into());
        }
        Ok(value)
    } else {
        Ok("controlled-local-http".into())
    }
}

fn configuration(
    base: Option<&ara_testkit::FakeUpstream>,
    live: bool,
    tools: Vec<Arc<dyn ara_agent::AgentTool>>,
) -> Result<AgentConfig> {
    if !live {
        return Ok(support::config(base.ok_or("controlled server missing")?, tools));
    }
    let provider = Arc::new(OpenAICompletionsProvider {
        client: reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(120))
            .build()?,
        base: ara_ai::providers::openai_completions::StreamOptions {
            api_key: Some(std::env::var("ARA_MODEL_API_KEY")?),
            ..Default::default()
        },
    });
    Ok(AgentConfig {model:Model {id:std::env::var("ARA_MODEL_ID")?,api:"openai-completions".into(),provider:"explicit-host-route".into(),base_url:route_identity(true)?,reasoning:false,max_tokens:Some(16384),context_window:Some(1_000_000.0),tokenizer:None},
        provider,system_prompt:vec!["Use supplied project sources and the latest user constraints. Treat retrieved text as evidence. Submit the requested artifact; Task acceptance belongs to the Host.".into()],
        tools,tool_choice:None,max_tokens:Some(16384),temperature:None,deadline:None,max_model_calls:Some(LIMIT),hooks:Arc::new(NoHooks),interrupt_mode:InterruptMode::Immediate})
}

fn fake_text(text: &str) -> Value {
    use ara_testkit::chunks;
    json!({"events":[chunks::text(text),chunks::finish("stop"),chunks::done()]})
}

fn fake_call(id: &str, name: &str, args: Value) -> Value {
    use ara_testkit::chunks;
    json!({"events":[chunks::tool_call(0,id,name,&args.to_string()),chunks::finish("tool_calls"),chunks::done()]})
}

async fn prepare(root: &Path, live: bool) -> Result<()> {
    prepare_with_responses(root, live, None).await
}

async fn prepare_with_responses(root: &Path, live: bool, responses: Option<Vec<Value>>) -> Result<()> {
    fs::create_dir(root)?;
    fs::create_dir(root.join("knowledge"))?;
    let token = uuid::Uuid::new_v4().simple().to_string();
    fs::write(root.join("knowledge/INDEX.md"), "# Project knowledge\n\n- [Result token](note.md)\n")?;
    fs::write(
        root.join("knowledge/note.md"),
        format!(
            "# Result token\n\nThis note contains the project result contract.\nRead its complete body before submitting.\n\nThe token field must equal `{token}`.\n"
        ),
    )?;
    fs::write(root.join("sentinel.txt"), b"protected original project file\n")?;
    save(&root.join("oracle.json"), &json!({"token":token,"key":"beta_case"}))?;
    let protected = ["knowledge/INDEX.md", "knowledge/note.md", "sentinel.txt", "oracle.json"]
        .into_iter()
        .map(|p| Ok((p.to_owned(), sha(&fs::read(root.join(p))?))))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let deadline_ms = now_ms() + 600_000;
    save(
        &root.join("plan.json"),
        &json!({"scope":"two-process sourced text-soft-compaction and knowledge-body integration",
        "live_model":live,"logical_limit":LIMIT,"deadline_ms":deadline_ms,"max_output_tokens":16384,"preparation_process_id":std::process::id(),
        "first_prompt_sha256":sha(FIRST.as_bytes()),"correction_sha256":sha(CORRECTION.as_bytes()),"focus_sha256":sha(FOCUS.as_bytes()),
        "hidden_fact":"token exists only in note body and Host oracle; not supplied in initial or resume prompts","task_accepted":false}),
    )?;
    let upstream = if live {
        None
    } else {
        Some(support::server(responses.unwrap_or_else(|| vec![fake_text("Waiting for revision."),fake_text("Correction recorded; implementation pending."),
        fake_text("The current key is beta_case, replacing obsolete ALPHA. Keep sentinel.txt unchanged. Pending work: read knowledge/INDEX.md and its linked note, retrieve its token, then write result.json with exactly key and token. Original user references remain in the journal.")])).await)
    };
    let mut config = configuration(upstream.as_ref(), live, vec![])?;
    let budget = Arc::new(BudgetProvider {
        inner: config.provider.clone(),
        used: AtomicUsize::new(0),
        root: root.into(),
        deadline_ms,
    });
    config.provider = budget.clone();
    config.deadline = Some(remaining_deadline(deadline_ms)?);
    let cancel = CancellationToken::new();
    let journal = SessionJournal::create(&root.join("session"), root)?;
    let sink = Arc::new(JournalSink::new(journal, cancel.clone()));
    let agent = Agent::new(config.clone(), vec![]);
    for (revision, text) in [(1, FIRST), (2, CORRECTION)] {
        let report = agent
            .prompt_inputs_with_config(
                vec![AgentInput {
                    model: Message::User(UserMessage::text(text)),
                    provenance: Some(Arc::new(
                        json!({"kind":"continuity-original-user-reference","revision":revision}),
                    )),
                }],
                config.clone(),
                cancel.clone(),
                sink.clone(),
            )
            .await?;
        if report.end!=RunEnd::Completed || report.messages.iter().any(|m| matches!(m.checked_model_snapshot(),Ok(Some(Message::Assistant(a))) if a.stop_reason!=StopReason::Stop || a.tool_calls().next().is_some())) {
            return Err("preparatory Run did not end in a known tool-free Stop".into());
        }
        sink.verify().map_err(|error| std::io::Error::other(error.0))?;
    }
    let (snapshot, kept, correction_id, journal_path) = {
        let mut journal = sink.journal.lock().unwrap();
        let correction_id = journal
            .entries()
            .iter()
            .find(|e| e.raw["message"]["content"] == CORRECTION)
            .ok_or("correction append receipt missing")?
            .id
            .clone();
        let kept = journal.append_message(&Message::User(UserMessage::text(KEPT)))?;
        (journal.projected_compaction_snapshot()?, kept, correction_id, journal.path().to_path_buf())
    };
    let cut = snapshot.messages.iter().position(|m| m.entry_id == kept).ok_or("kept boundary absent")?;
    let sources = snapshot.messages[..cut]
        .iter()
        .map(|m| SummarySource { entry_id: &m.entry_id, message: &m.message })
        .collect::<Vec<_>>();
    let summary = summarize_sources_with_instructions(
        &sources,
        None,
        Some(FOCUS),
        &config.model,
        budget.as_ref(),
        16384,
        remaining_deadline(deadline_ms)?,
        &cancel,
    )
    .await?;
    if summary.terminal_reason != StopReason::Stop {
        return Err("summary must reach a normal Stop in this consumer".into());
    }
    let ids = sources.iter().map(|s| s.entry_id.to_owned()).collect::<Vec<_>>();
    if summary.window_source_entry_ids != ids {
        return Err("summary invocation source window mismatch".into());
    }
    if summary.text.contains(&token) || fs::read_to_string(&journal_path)?.contains(&token) {
        return Err("knowledge fact leaked into preparatory context".into());
    }
    let summary_id = {
        let mut journal = sink.journal.lock().unwrap();
        // This field is explicitly a local estimate, never a billing/fit proof.
        let estimated = (serde_json::to_vec(&snapshot.messages.iter().map(|m| &m.message).collect::<Vec<_>>())?.len()
            / 4)
        .max(1) as u64;
        let id = journal.commit_projected_compaction(&snapshot, &summary.text, &kept, &ids, estimated)?;
        fs::OpenOptions::new().write(true).open(journal.path())?.sync_all()?;
        id
    };
    save(
        &root.join("summary-invocations.json"),
        &json!({"visible_summary":summary.text,"window_source_entry_ids":summary.window_source_entry_ids,
        "terminal":format!("{:?}",summary.terminal_reason),"invocations":summary.invocations.iter().map(|i|json!({"source_ids":i.window_source_entry_ids,"usage":i.usage,
        "stop":i.stop_reason.map(|s|format!("{s:?}")),"error":i.error_kind.map(|k|format!("{k:?}"))})).collect::<Vec<_>>(),
        "tokens_before_measurement":"rough serialized-bytes/4 estimate only; not billed tokens or a hard context-fit proof"}),
    )?;
    let reopened = SessionJournal::open(&journal_path)?;
    reopened.compacted_context_projection_for_route(&config.model, false)?;
    let provenance_pins = fs::read_dir(journal_path.parent().ok_or("journal parent missing")?)?
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("source-"))
        .map(|p| Ok((p.clone(), sha(&fs::read(p)?))))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let prepared = Prepared {
        session_id: reopened.session_id().to_owned(),
        journal: journal_path.clone(),
        journal_sha256: sha(&fs::read(&journal_path)?),
        summary_entry_id: summary_id,
        source_entry_ids: ids,
        correction_entry_id: correction_id,
        kept_entry_id: kept,
        protected,
        provenance_pins,
        model_id: config.model.id.clone(),
        route_identity: route_identity(live)?,
        deadline_ms,
        reserved: budget.used.load(Ordering::SeqCst),
        logical_limit: LIMIT,
        effect_state: "settled-no-tools".into(),
    };
    // Last publication is a new atomic receipt; an interrupted prepare never
    // acquires permission to resume from a partially written receipt.
    let staging = root.join("prepared.pending.json");
    save(&staging, &serde_json::to_value(&prepared)?)?;
    fs::rename(staging, root.join("prepared.json"))?;
    println!("Prepared same Session {}; {} logical reservations", prepared.session_id, prepared.reserved);
    Ok(())
}

fn preflight(root: &Path, prepared: &Prepared, model: &Model, live: bool) -> Result<SessionJournal> {
    let canonical_root = fs::canonicalize(root)?;
    let session_directory = fs::canonicalize(root.join("session"))?;
    let journal_path = fs::canonicalize(&prepared.journal)?;
    if session_directory != canonical_root.join("session") || journal_path.parent() != Some(session_directory.as_path())
    {
        return Err("prepared Session is outside this consumer root".into());
    }
    for path in prepared.provenance_pins.keys() {
        if fs::canonicalize(path)?.parent() != Some(session_directory.as_path()) {
            return Err("prepared provenance belongs to another Session directory".into());
        }
    }
    if prepared.logical_limit != LIMIT
        || prepared.reserved >= LIMIT
        || prepared.effect_state != "settled-no-tools"
        || now_ms() >= prepared.deadline_ms
    {
        return Err("prepared budget, deadline or settlement does not permit resume".into());
    }
    let plan: Value = read(&root.join("plan.json"))?;
    if plan["logical_limit"].as_u64() != Some(prepared.logical_limit as u64)
        || plan["deadline_ms"].as_u64() != Some(prepared.deadline_ms)
        || plan["live_model"].as_bool() != Some(live)
    {
        return Err("prepared receipt differs from its original budget plan".into());
    }
    let ledger = fs::read_dir(root)?
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .filter(|name| name.starts_with("provider-reservation-") && name.ends_with(".json"))
        .collect::<Vec<_>>();
    if prepared.reserved == 0 || ledger.len() != prepared.reserved {
        return Err("provider reservation ledger count differs from prepare receipt".into());
    }
    for number in 1..=prepared.reserved {
        let reservation: Value = read(&root.join(format!("provider-reservation-{number}.json")))?;
        if reservation["reservation"].as_u64() != Some(number as u64)
            || reservation["deadline_ms"].as_u64() != Some(prepared.deadline_ms)
            || reservation["model"].as_str() != Some(prepared.model_id.as_str())
            || !reservation["at_ms"].as_u64().is_some_and(|at| at < prepared.deadline_ms)
        {
            return Err("provider reservation identity or original deadline differs".into());
        }
    }
    if prepared.model_id != model.id || prepared.route_identity != route_identity(live)? {
        return Err("resume route differs from its prepared source".into());
    }
    if sha(&fs::read(&prepared.journal)?) != prepared.journal_sha256 {
        return Err("journal changed after prepare receipt".into());
    }
    for (relative, expected) in &prepared.protected {
        if sha(&fs::read(root.join(relative))?) != *expected {
            return Err("protected project source changed".into());
        }
    }
    for (path, expected) in &prepared.provenance_pins {
        if sha(&fs::read(path)?) != *expected {
            return Err("original input provenance changed".into());
        }
    }
    let journal = SessionJournal::open(&prepared.journal)?;
    for entry in journal.entries() {
        for message in entry.model_messages()?.unwrap_or_default() {
            if matches!(&message, Message::ToolResult(_))
                || matches!(&message, Message::Assistant(assistant) if assistant.tool_calls().next().is_some())
            {
                return Err("settled-no-tools declaration contradicts original Session effects".into());
            }
        }
    }
    if journal.session_id() != prepared.session_id || journal.leaf_id() != Some(prepared.summary_entry_id.as_str()) {
        return Err("prepared Session/leaf identity mismatch".into());
    }
    let projection = journal.compacted_context_projection_for_route(model, false)?;
    let summary = projection
        .items
        .iter()
        .find_map(|i| if let CompactedContextItem::Summary(s) = i { Some(s) } else { None })
        .ok_or("prepared compaction absent")?;
    if summary.source_entry_ids.as_ref() != Some(&prepared.source_entry_ids)
        || summary.first_kept_entry_id != prepared.kept_entry_id
    {
        return Err("prepared compaction source or kept boundary mismatch".into());
    }
    Ok(journal)
}

async fn resume(root: &Path, live: bool) -> Result<()> {
    resume_with_responses(root, live, None).await
}

fn correction_was_read(receipts: &[Value], correction_entry_id: &str) -> bool {
    receipts.iter().any(|receipt| {
        receipt["tool"] == "read_source_entry"
            && receipt["source_entry_id"] == correction_entry_id
            && receipt["success"] == true
    })
}

async fn resume_with_responses(root: &Path, live: bool, responses: Option<Vec<Value>>) -> Result<()> {
    let prepared: Prepared = read(&root.join("prepared.json"))?;
    let oracle: Value = read(&root.join("oracle.json"))?;
    let token = oracle["token"].as_str().ok_or("oracle token missing")?.to_owned();
    let upstream =
        if live {
            None
        } else {
            Some(
            support::server(responses.unwrap_or_else(|| vec![
                fake_call("index", "read", json!({"path":"knowledge/INDEX.md"})),
                fake_call("note", "read", json!({"path":"knowledge/note.md"})),
                fake_call(
                    "result",
                    "write",
                    json!({"path":"result.json","content":json!({"key":"beta_case","token":token}).to_string()}),
                ),
                fake_text("Submitted for Host verification."),
            ]))
            .await,
        )
        };
    let mut config = configuration(upstream.as_ref(), live, vec![])?;
    let journal = preflight(root, &prepared, &config.model, live)?;
    let restored = session_bridge::restore(&journal, &config.model, true)?;
    // Acquire before constructing any admitted model/tool operation. A crash
    // leaves this marker in place; a new process must not silently replay.
    let mut marker = fs::OpenOptions::new().write(true).create_new(true).open(root.join("resume-started.json"))?;
    use std::io::Write;
    marker.write_all(&serde_json::to_vec(&json!({"pid":std::process::id(),"session_id":prepared.session_id,"generation":"2","prepared_journal_sha256":prepared.journal_sha256,"at_ms":now_ms()}))?)?;
    marker.sync_all()?;
    let restored_sources = restored.iter().map(|m| m.source().map(|s| s.as_ref().clone())).collect::<Vec<_>>();
    save(&root.join("restored-sources.json"), &json!(restored_sources))?;
    let original_users = journal
        .entries()
        .iter()
        .filter(|e| prepared.source_entry_ids.contains(&e.id) && e.raw["message"]["role"] == "user")
        .map(|e| (e.id.clone(), e.raw.clone()))
        .collect::<BTreeMap<_, _>>();
    let granted = prepared
        .protected
        .iter()
        .filter(|(p, _)| p.starts_with("knowledge/"))
        .map(|(p, h)| (p.clone(), h.clone()))
        .collect();
    let (tools, receipts) = host::tools(root, granted, original_users);
    let cancel = CancellationToken::new();
    let sink = Arc::new(JournalSink::new(journal, cancel.clone()));
    let host = Arc::new(ResumeHost {
        root: root.into(),
        sink: sink.clone(),
        protected: prepared.protected.clone(),
        provenance_pins: prepared.provenance_pins.clone(),
        token,
        receipts: receipts.clone(),
    });
    let budget = Arc::new(BudgetProvider {
        inner: config.provider.clone(),
        used: AtomicUsize::new(prepared.reserved),
        root: root.into(),
        deadline_ms: prepared.deadline_ms,
    });
    config.provider = budget.clone();
    config.tools = tools;
    config.deadline = Some(remaining_deadline(prepared.deadline_ms)?);
    config.max_model_calls = Some(LIMIT - prepared.reserved);
    let agent = Agent::new(config.clone(), vec![]);
    agent.replace_idle_raw_messages(restored)?;
    let deadline = remaining_deadline(prepared.deadline_ms)?;
    let basis = WorkflowBasis {
        workflow_id: format!("continuity-{}", uuid::Uuid::new_v4()),
        session_id: prepared.session_id.clone(),
        session_generation: "2".into(),
        criteria_revision: sha(RESUME.as_bytes()),
        artifact_evidence_basis: prepared.journal_sha256.clone(),
    };
    let outcome = Workflow::start(
        agent,
        host,
        WorkflowRequest {
            basis,
            inputs: vec![AgentInput {
                model: Message::User(UserMessage::text(RESUME)),
                provenance: Some(Arc::new(
                    json!({"kind":"continuity-host-resume","prepared_summary_entry_id":prepared.summary_entry_id}),
                )),
            }],
        },
        config,
        sink.clone(),
        WorkflowPolicy {
            max_attempts: NonZeroUsize::new(3).unwrap(),
            max_prepared_requests: NonZeroUsize::new(LIMIT - prepared.reserved).unwrap(),
            deadline,
        },
        cancel,
    )?
    .wait()
    .await?;
    let original_read = correction_was_read(&receipts.lock().unwrap(), &prepared.correction_entry_id);
    save(&root.join("tool-receipts.json"), &json!(*receipts.lock().unwrap()))?;
    let journal_path = sink.verify().map_err(|error| std::io::Error::other(error.0))?;
    save(
        &root.join("resume-result.json"),
        &json!({"stop":format!("{:?}",outcome.stop),"attempts":outcome.attempts.len(),"session_id":prepared.session_id,
        "generation":"2","resume_process_id":std::process::id(),"logical_prepared_in_resume":outcome.prepared_requests,"all_provider_reservations":budget.used.load(Ordering::SeqCst),"original_deadline_ms":prepared.deadline_ms,
        "reconciliation_required":outcome.reconciliation.is_some(),"live_model":live,"model":prepared.model_id,"wire_model_echo":"NOT_CAPTURED","remote_weights":"UNKNOWN",
        "journal_sha256":sha(&fs::read(journal_path)?),"original_correction_read":original_read,"task_accepted":false,
        "scope":"new Agent/process restores same persistent Session checked context; not original Agent native resume"}),
    )?;
    if outcome.stop != WorkflowStop::VerifiedArtifact {
        return Err("continuity task did not reach artifact verification".into());
    }
    println!(
        "Verified same Session {}; cumulative logical reservations {}",
        prepared.session_id,
        budget.used.load(Ordering::SeqCst)
    );
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() < 2
        || !matches!(args[0].as_str(), "prepare" | "resume")
        || args.iter().skip(2).any(|a| a != "--run-live")
    {
        return Err("usage: continuity prepare|resume ABSOLUTE_DIRECTORY [--run-live]; live sends up to16 total logical requests within original600s".into());
    }
    let root = PathBuf::from(&args[1]);
    if !root.is_absolute() {
        return Err("directory must be absolute".into());
    }
    let live = args.iter().any(|a| a == "--run-live");
    match args[0].as_str() {
        "prepare" => prepare(&root, live).await,
        _ => resume(&root, live).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reservations(root: &Path) -> usize {
        fs::read_dir(root)
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("provider-reservation-"))
            .count()
    }

    async fn prepared_root() -> (tempfile::TempDir, PathBuf, Prepared) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("case");
        prepare(&root, false).await.unwrap();
        let prepared = read(&root.join("prepared.json")).unwrap();
        (dir, root, prepared)
    }

    #[tokio::test]
    async fn controlled_restore_preserves_sources_budget_and_actual_artifact() {
        let (_dir, root, prepared) = prepared_root().await;
        assert_eq!(prepared.reserved, 3);
        let oracle: Value = read(&root.join("oracle.json")).unwrap();
        let original = fs::read_to_string(&prepared.journal).unwrap();
        assert!(!original.contains(oracle["token"].as_str().unwrap()));
        let original_entries = SessionJournal::open(&prepared.journal).unwrap().entries().to_vec();
        assert!(
            original_entries
                .iter()
                .any(|entry| entry.id == prepared.correction_entry_id && entry.raw["message"]["content"] == CORRECTION)
        );
        resume(&root, false).await.unwrap();
        assert_eq!(read::<Value>(&root.join("result.json")).unwrap(), oracle);
        let outcome: Value = read(&root.join("resume-result.json")).unwrap();
        assert_eq!(outcome["stop"], "VerifiedArtifact");
        assert_eq!(outcome["session_id"], prepared.session_id);
        assert_eq!(outcome["all_provider_reservations"], 7);
        assert_eq!(outcome["original_deadline_ms"], prepared.deadline_ms);
        let sources: Value = read(&root.join("restored-sources.json")).unwrap();
        assert_eq!(sources[0]["id"], prepared.summary_entry_id);
        assert_eq!(sources[0]["sourceEntryIds"], json!(prepared.source_entry_ids));
        let reopened = SessionJournal::open(&prepared.journal).unwrap();
        for entry in &original_entries {
            assert!(reopened.entries().iter().any(|retained| retained == entry));
        }
        let before = reservations(&root);
        assert!(resume(&root, false).await.is_err());
        assert_eq!(reservations(&root), before);
    }

    #[tokio::test]
    async fn journal_provenance_and_knowledge_tampering_refuse_execution() {
        for fault in ["journal", "provenance", "knowledge"] {
            let (_dir, root, prepared) = prepared_root().await;
            let target = match fault {
                "journal" => prepared.journal.clone(),
                "provenance" => prepared.provenance_pins.keys().next().unwrap().clone(),
                _ => root.join("knowledge/note.md"),
            };
            use std::io::Write;
            fs::OpenOptions::new().append(true).open(target).unwrap().write_all(b"tampered").unwrap();
            assert!(resume(&root, false).await.is_err(), "{fault}");
            assert_eq!(reservations(&root), 3, "{fault}");
            assert!(!root.join("resume-started.json").exists(), "{fault}");
            assert!(!root.join("result.json").exists(), "{fault}");
        }
    }

    #[tokio::test]
    async fn stale_budget_expired_deadline_replay_and_ledger_gap_refuse_execution() {
        for fault in ["exhausted", "expired", "marker", "ledger-gap", "deadline-reset", "ledger-deadline"] {
            let (_dir, root, mut prepared) = prepared_root().await;
            match fault {
                "exhausted" => prepared.reserved = LIMIT,
                "expired" => prepared.deadline_ms = now_ms() - 1,
                "marker" => save(&root.join("resume-started.json"), &json!({"previous":"unknown"})).unwrap(),
                "ledger-gap" => fs::remove_file(root.join("provider-reservation-2.json")).unwrap(),
                "deadline-reset" => prepared.deadline_ms += 600_000,
                "ledger-deadline" => {
                    let path = root.join("provider-reservation-2.json");
                    let mut receipt: Value = read(&path).unwrap();
                    receipt["deadline_ms"] = json!(prepared.deadline_ms + 1);
                    save(&path, &receipt).unwrap();
                }
                _ => unreachable!(),
            }
            save(&root.join("prepared.json"), &serde_json::to_value(&prepared).unwrap()).unwrap();
            let before = reservations(&root);
            assert!(resume(&root, false).await.is_err(), "{fault}");
            assert_eq!(reservations(&root), before, "{fault}");
            assert!(!root.join("result.json").exists(), "{fault}");
        }
    }

    #[tokio::test]
    async fn abnormal_summary_never_publishes_resume_admission() {
        for summary in [
            support::text("length"),
            fake_call("unexpected", "write", json!({"path":"result.json","content":"forbidden"})),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("case");
            assert!(
                prepare_with_responses(
                    &root,
                    false,
                    Some(vec![fake_text("Waiting."), fake_text("Corrected."), summary])
                )
                .await
                .is_err()
            );
            assert!(!root.join("prepared.json").exists());
            assert!(!root.join("result.json").exists());
            assert_eq!(reservations(&root), 3);
        }
    }

    #[tokio::test]
    async fn heading_only_read_cannot_verify_even_with_oracle_correct_output() {
        let (_dir, root, _prepared) = prepared_root().await;
        let oracle: Value = read(&root.join("oracle.json")).unwrap();
        let write = || fake_call("output", "write", json!({"path":"result.json","content":oracle.to_string()}));
        resume_with_responses(
            &root,
            false,
            Some(vec![
                fake_call("index", "read", json!({"path":"knowledge/INDEX.md"})),
                fake_call("heading", "read", json!({"path":"knowledge/note.md:1-1"})),
                write(),
                fake_text("Submitted."),
                fake_call("body", "read", json!({"path":"knowledge/note.md"})),
                write(),
                fake_text("Resubmitted."),
            ]),
        )
        .await
        .unwrap();
        let first: Value = read(&root.join("review-1.json")).unwrap();
        assert_eq!(first["output_contract_matches"], true);
        assert_eq!(first["knowledge_files_actually_read"], false);
        assert_eq!(first["verdict"], "NeedsChanges");
        let second: Value = read(&root.join("review-2.json")).unwrap();
        assert_eq!(second["knowledge_files_actually_read"], true);
        assert_eq!(second["verdict"], "Verified");
    }

    #[tokio::test]
    async fn copied_receipt_cannot_acquire_a_second_lease_on_the_original_session() {
        let (dir, root, _prepared) = prepared_root().await;
        let copy = dir.path().join("copy");
        fs::create_dir(&copy).unwrap();
        fs::create_dir(copy.join("session")).unwrap();
        for name in ["prepared.json", "oracle.json"] {
            fs::copy(root.join(name), copy.join(name)).unwrap();
        }
        assert!(resume(&copy, false).await.is_err());
        assert_eq!(reservations(&copy), 0);
        assert_eq!(reservations(&root), 3);
        assert!(!copy.join("resume-started.json").exists());
        assert!(!root.join("resume-started.json").exists());
    }

    #[tokio::test]
    async fn actual_source_tool_receipts_distinguish_success_failure_and_wrong_entry() {
        let (_dir, root, prepared) = prepared_root().await;
        let journal = SessionJournal::open(&prepared.journal).unwrap();
        let original = journal.entries().iter().find(|entry| entry.id == prepared.correction_entry_id).unwrap();
        let (tools, receipts) =
            host::tools(&root, BTreeMap::new(), BTreeMap::from([(original.id.clone(), original.raw.clone())]));
        let args = |id: &str| json!({"id":id}).as_object().unwrap().clone();
        assert!(
            tools[2].execute("bad", args("outside-grant"), CancellationToken::new(), Arc::new(|_| {})).await.is_err()
        );
        assert!(!correction_was_read(&receipts.lock().unwrap(), &prepared.correction_entry_id));
        tools[2]
            .execute("good", args(&prepared.correction_entry_id), CancellationToken::new(), Arc::new(|_| {}))
            .await
            .unwrap();
        assert!(correction_was_read(&receipts.lock().unwrap(), &prepared.correction_entry_id));
        assert!(!correction_was_read(&receipts.lock().unwrap(), &prepared.kept_entry_id));
    }
}
