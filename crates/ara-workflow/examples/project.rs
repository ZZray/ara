//! Reusable project Host integration over the public Full Core workflow API.
//! This example is opt-in. Project paths and commands are supplied by the Host;
//! the model receives only bounded read/write and fixed public-check tools.
//! The S3 C++ fixture is one application. Reusing its original hidden oracle
//! after feedback is adaptive validation, not an independent new holdout.
//! Process receipts establish the original child's status, not all descendants.
//! Tool allowlists and sealed source checks are not an OS sandbox.
use ara_agent::{
    Agent, AgentConfig, AgentEvent, AgentEventSink, AgentInput, AgentMessage, AgentTool, Concurrency, InterruptMode,
    NoHooks, RecordingSink, ToolError, ToolOutput, UpdateFn,
};
use ara_ai::{
    JsonObject, Message, Model, OpenAICompletionsProvider, OpenAIReasoningPolicy, ReasoningRequest, Tool, UserMessage,
};
use ara_session::SessionJournal;
use ara_tools::{
    ToolContext,
    read::{Range, ReadTool, split_selector},
    write::WriteTool,
};
use ara_workflow::*;
use async_trait::async_trait;
use ring::digest::{SHA256, digest};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    num::NonZeroUsize,
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CommandSpec {
    argv: Vec<String>,
    timeout_seconds: u64,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum GenerationPolicy {
    #[default]
    ProviderDefault,
    ThinkingTypeDisabled,
}

fn stream_options(
    key: String,
    policy: GenerationPolicy,
) -> Result<ara_ai::providers::openai_completions::StreamOptions, ara_ai::ProviderError> {
    let mut options = ara_ai::providers::openai_completions::StreamOptions { api_key: Some(key), ..Default::default() };
    if policy == GenerationPolicy::ThinkingTypeDisabled {
        // An explicit Host-selected wire contract, independent of catalog flags.
        // This does not infer provider behavior from model names or route URLs.
        options.reasoning = Some(ReasoningRequest { disable_reasoning: true, ..Default::default() });
        options.reasoning_policy = Some(OpenAIReasoningPolicy::from_model_spec(&json!({
            "reasoning": true,
            "compat": {"supportsReasoningParams": true, "reasoningDisableMode": "zai-thinking-disabled"}
        }))?);
    }
    Ok(options)
}

const PUBLIC_PASS_SUBMISSION: &str = "When check_project reports PASS, immediately finish this Run with a brief candidate submission. The Host independently verifies the candidate and can return NeedsChanges to continue in this same Session. Do not speculate about hidden tests without new evidence. When public checks fail, continue repairing the project.";

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProjectConfig {
    project_root: PathBuf,
    evidence_root: PathBuf,
    readonly_paths: Vec<String>,
    writable_paths: Vec<String>,
    public_check_commands: Vec<CommandSpec>,
    public_case_receipts: Vec<String>,
    review_command: CommandSpec,
    review_result_path: String,
    expected_hidden_total: usize,
    #[serde(default)]
    generation_policy: GenerationPolicy,
}

#[derive(Deserialize)]
struct ReviewResult {
    public_status: String,
    hidden_status: String,
    hidden_passed: Option<usize>,
    hidden_total: usize,
    status: String,
    effects_unknown: bool,
    source_integrity_after_evaluation: bool,
    oracle_unchanged_after_evaluation: bool,
    public_executed: bool,
    hidden_executed: bool,
    compile_known_failure: bool,
    stage: String,
}

fn sha(bytes: &[u8]) -> String {
    digest(&SHA256, bytes).as_ref().iter().map(|byte| format!("{byte:02x}")).collect()
}

fn save(path: &Path, value: &Value) -> Result<(), String> {
    fs::write(path, serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    sync(path)
}

fn sync(path: &Path) -> Result<(), String> {
    fs::OpenOptions::new().write(true).open(path).and_then(|file| file.sync_all()).map_err(|error| error.to_string())
}

fn normal_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['\\', ':'])
        && path.split('/').all(|part| !part.is_empty() && !matches!(part, "." | ".."))
        && Path::new(path).components().all(|part| matches!(part, Component::Normal(_)))
}

fn no_link(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() {
        return Err("symlink source paths are not admitted".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err("reparse source paths are not admitted".into());
        }
    }
    Ok(())
}

fn checked_file(root: &Path, relative: &str) -> Result<PathBuf, String> {
    if !normal_relative(relative) {
        return Err("only listed relative file paths are admitted".into());
    }
    let mut current = root.to_path_buf();
    no_link(&current)?;
    for component in Path::new(relative).components() {
        current.push(component.as_os_str());
        no_link(&current)?;
    }
    if !current.is_file() {
        return Err("the listed original must remain a regular file".into());
    }
    if !fs::canonicalize(&current)
        .map_err(|error| error.to_string())?
        .starts_with(fs::canonicalize(root).map_err(|error| error.to_string())?)
    {
        return Err("source path escaped its project root".into());
    }
    Ok(current)
}

impl ProjectConfig {
    fn validate(&self) -> Result<(), String> {
        if !self.project_root.is_absolute()
            || !self.project_root.is_dir()
            || !self.evidence_root.is_absolute()
            || self.evidence_root.exists()
        {
            return Err("project_root must exist and evidence_root must be a new absolute directory".into());
        }
        if self.evidence_root.starts_with(&self.project_root) || self.project_root.starts_with(&self.evidence_root) {
            return Err("project and evidence directories must be separate".into());
        }
        if self.readonly_paths.is_empty() || self.writable_paths.is_empty() || self.public_check_commands.is_empty() {
            return Err("explicit readonly/writable files and public check commands are required".into());
        }
        let mut public_receipts = BTreeSet::new();
        if self.public_case_receipts.is_empty()
            || self.public_case_receipts.iter().any(|path| {
                !path.strip_prefix("{build}/").is_some_and(normal_relative)
                    || !public_receipts.insert(path.to_lowercase())
            })
        {
            return Err("public_case_receipts requires distinct explicit {build}/ relative case files".into());
        }
        if self.expected_hidden_total == 0
            || !self.review_result_path.strip_prefix("{evidence}/").is_some_and(normal_relative)
        {
            return Err(
                "expected_hidden_total must be positive and review_result_path must be relative to {evidence}/".into(),
            );
        }
        let mut seen = BTreeSet::new();
        for relative in self.readonly_paths.iter().chain(&self.writable_paths) {
            checked_file(&self.project_root, relative)?;
            if !seen.insert(relative.to_lowercase()) {
                return Err("duplicate source file grant".into());
            }
        }
        for relative in &self.writable_paths {
            if !matches!(Path::new(relative).extension().and_then(|value| value.to_str()), Some("cpp" | "hpp")) {
                return Err("this C++ example writes only explicitly listed cpp/hpp originals".into());
            }
        }
        for command in self.public_check_commands.iter().chain(std::iter::once(&self.review_command)) {
            if command.argv.is_empty() || command.timeout_seconds == 0 || command.timeout_seconds > 900 {
                return Err("commands require a nonempty argv and timeout_seconds in 1..=900".into());
            }
            if !Path::new(&command.argv[0]).is_absolute() || !Path::new(&command.argv[0]).is_file() {
                return Err("argv[0] must be an existing absolute executable; no shell command strings".into());
            }
        }
        Ok(())
    }
}

struct JournalSink {
    journal: Mutex<SessionJournal>,
    failure: Mutex<Option<String>>,
    entries: Mutex<Vec<(AgentMessage, String)>>,
    recording: RecordingSink,
    cancel: CancellationToken,
}

#[async_trait]
impl AgentEventSink for JournalSink {
    async fn emit(&self, event: AgentEvent) {
        if let AgentEvent::MessageEnd { message } = &event {
            let result = (|| -> Result<(), String> {
                let mut journal = self.journal.lock().unwrap();
                let entry = journal
                    .append_agent_message_body(&message.json().map_err(|error| error.to_string())?)
                    .map_err(|error| error.to_string())?;
                if let Some(source) = message.source() {
                    save(
                        &journal.path().with_file_name(format!("provenance-{entry}.json")),
                        &json!({"entry":entry,"source":source.as_ref()}),
                    )?;
                }
                self.entries.lock().unwrap().push((message.clone(), entry));
                Ok(())
            })();
            if let Err(error) = result {
                *self.failure.lock().unwrap() = Some(error);
                self.cancel.cancel();
            }
        }
        self.recording.emit(event).await;
    }
}

impl JournalSink {
    fn snapshot(&self, candidate: &ReviewCandidate, directory: &Path) -> Result<BTreeMap<String, String>, String> {
        if let Some(error) = self.failure.lock().unwrap().clone() {
            return Err(error);
        }
        let journal = self.journal.lock().unwrap();
        let journal_path = journal.path();
        sync(journal_path)?;
        let reopened = SessionJournal::open(journal_path).map_err(|error| error.to_string())?;
        if reopened.session_id() != candidate.basis.workflow.session_id {
            return Err("journal Session identity mismatch".into());
        }
        let entries = self.entries.lock().unwrap();
        let mut records = Vec::new();
        let mut pins = BTreeMap::new();
        for message in &candidate.run.messages {
            let (_, id) = entries
                .iter()
                .find(|(record, _)| record.same_record(message))
                .ok_or("Run record has no original append receipt")?;
            let persisted =
                reopened.entries().iter().find(|entry| &entry.id == id).ok_or("reopen lost an original Run record")?;
            let body = message.json().map_err(|error| error.to_string())?;
            if persisted.raw["message"] != body {
                return Err("persisted body differs from canonical Run record".into());
            }
            let source = message.source();
            if let Some(source) = &source {
                let original = journal_path.with_file_name(format!("provenance-{id}.json"));
                let bytes = fs::read(original).map_err(|error| error.to_string())?;
                let saved: Value = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
                if saved["entry"] != *id || saved["source"] != **source {
                    return Err("persisted provenance differs from canonical input source".into());
                }
                fs::create_dir_all(directory.join("source-provenance")).map_err(|error| error.to_string())?;
                let name = format!("source-provenance/provenance-{id}.json");
                fs::write(directory.join(&name), &bytes).map_err(|error| error.to_string())?;
                sync(&directory.join(&name))?;
                pins.insert(name, sha(&bytes));
            }
            records.push(json!({"entry_id":id,"body":body,"source":source.as_deref()}));
        }
        if records.is_empty() || !records.iter().any(|record| record["body"]["role"] == "assistant") {
            return Err("Full Core attempt requires actual assistant records".into());
        }
        save(&directory.join("full-core-attempt.json"), &json!({"basis":candidate.basis,"records":records}))?;
        fs::copy(journal_path, directory.join("journal-snapshot.jsonl")).map_err(|error| error.to_string())?;
        sync(&directory.join("journal-snapshot.jsonl"))?;
        for name in ["full-core-attempt.json", "journal-snapshot.jsonl"] {
            let bytes = fs::read(directory.join(name)).map_err(|error| error.to_string())?;
            if bytes.is_empty() {
                return Err("Full snapshot file is empty".into());
            }
            pins.insert(name.into(), sha(&bytes));
        }
        save(
            &directory.join("full-core-receipts.json"),
            &json!({"binding_scope":"full-core-attempt-snapshot","basis":candidate.basis,"pins":pins}),
        )?;
        Ok(pins)
    }
}

#[derive(Clone)]
struct CheckContext {
    source: PathBuf,
    build: PathBuf,
    evidence: PathBuf,
}
impl CheckContext {
    fn expand(&self, value: &str) -> String {
        value
            .replace("{source}", &self.source.to_string_lossy())
            .replace("{build}", &self.build.to_string_lossy())
            .replace("{evidence}", &self.evidence.to_string_lossy())
    }
}

struct ProcessResult {
    success: bool,
    output: String,
}

fn public_case_is_known(value: &Value) -> Result<(), String> {
    if value.get("exit_code").and_then(Value::as_i64).is_none() {
        return Err("case has no authoritative integer child exit code".into());
    }
    let errors = value.get("errors").and_then(Value::as_array).ok_or("case errors is not an array")?;
    for error in errors {
        let error = error.as_str().ok_or("case error is not text")?.trim_start();
        if ["Empty", "queue.Empty", "TimeoutExpired", "TimeoutError", "OSError", "PermissionError", "FileNotFoundError"]
            .iter()
            .any(|prefix| error.starts_with(prefix))
        {
            return Err("case retained an unresolved child/transport exception".into());
        }
    }
    Ok(())
}
struct UnsettledEffect<'a> {
    unknown: &'a AtomicBool,
    cancel: &'a CancellationToken,
    settled: bool,
}
impl Drop for UnsettledEffect<'_> {
    fn drop(&mut self) {
        if !self.settled {
            self.unknown.store(true, Ordering::SeqCst);
            self.cancel.cancel();
        }
    }
}

struct ProjectHost {
    config: ProjectConfig,
    paths: Vec<String>,
    protected: BTreeMap<String, String>,
    sink: Arc<JournalSink>,
    sequence: AtomicUsize,
    check_sequence: AtomicUsize,
    effects_unknown: AtomicBool,
    cancel: CancellationToken,
    deadline: Instant,
}

impl ProjectHost {
    fn latch_unknown(&self) {
        self.effects_unknown.store(true, Ordering::SeqCst);
        self.cancel.cancel();
    }

    fn admit(&self, cancel: &CancellationToken) -> Result<(), String> {
        if self.effects_unknown.load(Ordering::SeqCst) {
            return Err("unresolved prior process effects; no replay admitted".into());
        }
        if self.cancel.is_cancelled() || cancel.is_cancelled() || Instant::now() >= self.deadline {
            return Err("cancelled or deadline reached before admission".into());
        }
        if let Some(error) = self.sink.failure.lock().unwrap().clone() {
            return Err(format!("Session persistence failed: {error}"));
        }
        self.check_protected()
    }

    fn manifest(&self, root: &Path) -> Result<BTreeMap<String, String>, String> {
        self.paths
            .iter()
            .map(|relative| {
                let bytes = fs::read(checked_file(root, relative)?).map_err(|error| error.to_string())?;
                Ok((relative.clone(), sha(&bytes)))
            })
            .collect()
    }

    fn check_protected(&self) -> Result<(), String> {
        for (relative, hash) in &self.protected {
            let bytes =
                fs::read(checked_file(&self.config.project_root, relative)?).map_err(|error| error.to_string())?;
            if sha(&bytes) != *hash {
                return Err("a protected project original changed".into());
            }
        }
        Ok(())
    }

    fn seal(&self, directory: &Path) -> Result<(CheckContext, BTreeMap<String, String>), String> {
        self.check_protected()?;
        fs::create_dir(directory).map_err(|error| error.to_string())?;
        let context = CheckContext {
            source: directory.join("sealed-source"),
            build: directory.join("build"),
            evidence: directory.into(),
        };
        fs::create_dir(&context.source).map_err(|error| error.to_string())?;
        let mut frozen = BTreeMap::new();
        for relative in &self.paths {
            let bytes =
                fs::read(checked_file(&self.config.project_root, relative)?).map_err(|error| error.to_string())?;
            let target = context.source.join(relative);
            fs::create_dir_all(target.parent().unwrap()).map_err(|error| error.to_string())?;
            fs::write(&target, &bytes).map_err(|error| error.to_string())?;
            sync(&target)?;
            frozen.insert(relative.clone(), sha(&bytes));
        }
        if self.manifest(&self.config.project_root)? != frozen {
            return Err("project changed while sealing".into());
        }
        save(&directory.join("source-manifest.json"), &json!({"files":frozen}))?;
        Ok((context, frozen))
    }

    fn intact(&self, context: &CheckContext, frozen: &BTreeMap<String, String>) -> Result<bool, String> {
        self.check_protected()?;
        Ok(self.manifest(&self.config.project_root)? == *frozen && self.manifest(&context.source)? == *frozen)
    }

    async fn process(
        &self,
        command: &CommandSpec,
        context: &CheckContext,
        cancel: &CancellationToken,
    ) -> Result<ProcessResult, String> {
        self.admit(cancel)?;
        let argv = command.argv.iter().map(|value| context.expand(value)).collect::<Vec<_>>();
        let number = self.sequence.fetch_add(1, Ordering::SeqCst);
        let stem = context.evidence.join(format!("process-{number}"));
        let stdout = stem.with_extension("stdout");
        let stderr = stem.with_extension("stderr");
        let mut child = tokio::process::Command::new(&argv[0])
            .args(&argv[1..])
            .current_dir(&context.source)
            .env_remove("ARA_MODEL_API_KEY")
            .env_remove("RAY_GATEWAY_KEY")
            .stdin(Stdio::null())
            .stdout(Stdio::from(fs::File::create(&stdout).map_err(|error| error.to_string())?))
            .stderr(Stdio::from(fs::File::create(&stderr).map_err(|error| error.to_string())?))
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| error.to_string())?;
        let mut guard = UnsettledEffect { unknown: &self.effects_unknown, cancel: &self.cancel, settled: false };
        let pid = child.id();
        // A logging error after spawn must still join this original child.
        let started = save(
            &stem.with_extension("started.json"),
            &json!({"argv":argv,"pid":pid,"cwd":context.source,"child_settlement":"PENDING","descendant_settlement":"NOT_PROVEN"}),
        );
        if started.is_err() {
            self.latch_unknown();
        }
        let until = (Instant::now() + Duration::from_secs(command.timeout_seconds)).min(self.deadline);
        let mut timed_out = false;
        let mut cancelled = false;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(error) => return Err(format!("original child settlement unknown: {error}")),
            }
            if cancel.is_cancelled() || Instant::now() >= until {
                cancelled = cancel.is_cancelled();
                timed_out = !cancelled;
                child
                    .kill()
                    .await
                    .map_err(|error| format!("original child kill failed; settlement unknown: {error}"))?;
                break child
                    .wait()
                    .await
                    .map_err(|error| format!("original child wait failed; settlement unknown: {error}"))?;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        // Timeout/cancel only reaps the original child. Descendants or adapter
        // effects can remain unknown, so prevent any model-triggered replay.
        if cancelled || timed_out {
            self.latch_unknown();
        }
        let receipt = save(
            &stem.with_extension("json"),
            &json!({"argv":argv,"pid":pid,"cwd":context.source,"exit":status.code(),"success":status.success(),"timed_out":timed_out,"cancelled":cancelled,"stdout":stdout,"stderr":stderr,"child_settlement":"REAPED","descendant_settlement":"NOT_PROVEN","effects_unknown":self.effects_unknown.load(Ordering::SeqCst)}),
        );
        started?;
        receipt?;
        guard.settled = true;
        if cancelled || timed_out {
            return Err("original child killed and reaped; descendant/evaluation effects unknown".into());
        }
        let preview = |path: &Path| -> Result<String, String> {
            let mut bytes = Vec::new();
            fs::File::open(path)
                .map_err(|error| error.to_string())?
                .take(32768)
                .read_to_end(&mut bytes)
                .map_err(|error| error.to_string())?;
            let clipped = fs::metadata(path).map_err(|error| error.to_string())?.len() > bytes.len() as u64;
            Ok(format!(
                "{}{}",
                String::from_utf8_lossy(&bytes),
                if clipped { "\n[output truncated; full log retained by Host]" } else { "" }
            ))
        };
        Ok(ProcessResult {
            success: status.success(),
            output: format!("stdout:\n{}\nstderr:\n{}", preview(&stdout)?, preview(&stderr)?),
        })
    }

    async fn public_checks(&self, context: &CheckContext, cancel: &CancellationToken) -> Result<ProcessResult, String> {
        let mut success = true;
        let mut output = String::new();
        let mut ctest_executed = false;
        for (index, command) in self.config.public_check_commands.iter().enumerate() {
            let result = self.process(command, context, cancel).await?;
            ctest_executed = index + 1 == self.config.public_check_commands.len();
            output.push_str(&format!(
                "Public check {}: {}\n{}\n",
                index + 1,
                if result.success { "PASS" } else { "FAIL" },
                result.output
            ));
            if !result.success {
                success = false;
                break;
            }
        }
        // CTest can return an ordinary failure even when its test helper lost a
        // nested child result. Inspect the original case receipts, including
        // on CTest exit 8; configure/build failures have no executed cases.
        if ctest_executed {
            let checked = (|| -> Result<(), String> {
                for template in &self.config.public_case_receipts {
                    let relative = template.strip_prefix("{build}/").ok_or("invalid public receipt template")?;
                    let path = checked_file(&context.build, relative)?;
                    let value: Value = serde_json::from_slice(&fs::read(path).map_err(|error| error.to_string())?)
                        .map_err(|error| error.to_string())?;
                    public_case_is_known(&value)?;
                }
                Ok(())
            })();
            if let Err(error) = checked {
                self.latch_unknown();
                return Err(format!("nested public-case settlement unknown: {error}"));
            }
        }
        Ok(ProcessResult { success, output })
    }

    async fn check_project(&self, cancel: &CancellationToken) -> Result<ToolOutput, String> {
        self.admit(cancel)?;
        let number = self.check_sequence.fetch_add(1, Ordering::SeqCst);
        let (context, frozen) = self.seal(&self.config.evidence_root.join(format!("public-check-{number}")))?;
        let result = self.public_checks(&context, cancel).await?;
        if !self.intact(&context, &frozen)? {
            return Err("project or sealed originals changed during public check".into());
        }
        Ok(ToolOutput::text(format!(
            "Public project checks: {}\n{}\n{}",
            if result.success { "PASS" } else { "FAIL" },
            result.output,
            if result.success {
                PUBLIC_PASS_SUBMISSION
            } else {
                "Public checks failed; continue repairing the project."
            }
        )))
    }
}

struct FileTool {
    name: &'static str,
    inner: Arc<dyn AgentTool>,
    host: Arc<ProjectHost>,
}

impl FileTool {
    fn authorize_read(&self, input: &str) -> Result<(), ToolError> {
        // Parse without filesystem access until the base has an exact project grant.
        if input.contains(['%', '\\']) {
            return Err(ToolError("read paths do not accept escapes or backslashes".into()));
        }
        let parsed = split_selector(input, |_| false).map_err(ToolError)?;
        let finite = |range: &Range| matches!(range, Range::From(_, Some(_)) | Range::Tail(_));
        if parsed.1.raw
            || parsed.1.range.as_ref().is_some_and(|range| !finite(range))
            || !parsed.1.multi_ranges.iter().all(finite)
        {
            return Err(ToolError(
                "read accepts only bare paths or finite line windows; raw and open-ended selectors are not admitted"
                    .into(),
            ));
        }
        if !self.host.paths.iter().any(|path| path == &parsed.0) {
            return Err(ToolError("file is outside this tool's original project grant".into()));
        }
        checked_file(&self.host.config.project_root, &parsed.0).map_err(ToolError)?;

        // The real ReadTool gives a literal existing name priority over selectors.
        // Check that same interpretation after authorizing the base and suffix;
        // a literal colon name or Windows ADS must never change the admitted file.
        let probe_error = std::cell::Cell::new(false);
        let literal = split_selector(input, |candidate| {
            match fs::symlink_metadata(self.host.config.project_root.join(candidate)) {
                Ok(_) => true,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound
                            | std::io::ErrorKind::NotADirectory
                            | std::io::ErrorKind::InvalidFilename
                    ) =>
                {
                    false
                }
                Err(_) => {
                    probe_error.set(true);
                    true
                }
            }
        })
        .map_err(ToolError)?;
        if probe_error.get() || literal != parsed {
            return Err(ToolError(
                "read literal-path interpretation is unknown or differs from the authorized line window".into(),
            ));
        }
        Ok(())
    }
}

#[async_trait]
impl AgentTool for FileTool {
    fn definition(&self) -> Tool {
        let mut tool = self.inner.definition();
        if self.name == "write" {
            tool.description = format!(
                "Replace only the listed original project files. Exact relative path required; no selectors or URLs. Allowed paths: {}",
                self.host.config.writable_paths.join(", ")
            );
            tool.parameters["properties"]["path"]["enum"] = json!(self.host.config.writable_paths);
        } else {
            tool.description = format!(
                "Read only the listed original project files. Use an exact relative base path, optionally with finite line windows :N-M, :N+K, :-N (tail), or finite comma-separated ranges such as :1-20,80-100. Re-read omitted ranges as needed. Bare paths retain the default output limit. No :raw, :N, :N-, escapes, backslashes, URLs, or unlisted files. Allowed base paths: {}",
                self.host.paths.join(", ")
            );
            if let Some(path) = tool.parameters["properties"]["path"].as_object_mut() {
                path.remove("enum");
            }
        }
        tool
    }
    fn concurrency(&self, _: &JsonObject) -> Concurrency {
        Concurrency::Exclusive
    }
    async fn execute(
        &self,
        id: &str,
        args: JsonObject,
        cancel: CancellationToken,
        update: UpdateFn,
    ) -> Result<ToolOutput, ToolError> {
        self.host.admit(&cancel).map_err(ToolError)?;
        let relative =
            args.get("path").and_then(Value::as_str).ok_or_else(|| ToolError("missing relative path".into()))?;
        if self.name == "write" {
            if !self.host.config.writable_paths.iter().any(|path| path == relative) {
                return Err(ToolError("file is outside this tool's original project grant".into()));
            }
            checked_file(&self.host.config.project_root, relative).map_err(ToolError)?;
        } else {
            self.authorize_read(relative)?;
        }
        let result = self.inner.execute(id, args, cancel, update).await;
        self.host.check_protected().map_err(ToolError)?;
        result
    }
}

struct CheckTool(Arc<ProjectHost>);
#[async_trait]
impl AgentTool for CheckTool {
    fn definition(&self) -> Tool {
        Tool {name:"check_project".into(),description:"Seal the current project files and run only the Host's fixed public CMake/build/CTest checks. Returns public output. Independent Host review follows your submission.".into(),parameters:json!({"type":"object","properties":{},"additionalProperties":false}),..Default::default()}
    }
    fn concurrency(&self, _: &JsonObject) -> Concurrency {
        Concurrency::Exclusive
    }
    async fn execute(
        &self,
        _: &str,
        _: JsonObject,
        cancel: CancellationToken,
        _: UpdateFn,
    ) -> Result<ToolOutput, ToolError> {
        self.0.check_project(&cancel).await.map_err(ToolError)
    }
}

#[async_trait]
impl WorkflowHost for ProjectHost {
    async fn review(
        &self,
        candidate: &ReviewCandidate,
        cancel: &CancellationToken,
    ) -> Result<ReviewReceipt, ReviewError> {
        self.admit(cancel).map_err(ReviewError)?;
        let mut settlement = UnsettledEffect { unknown: &self.effects_unknown, cancel: &self.cancel, settled: false };
        let attempt = self.config.evidence_root.join(format!("attempt-{}", candidate.basis.attempt));
        let (context, frozen) = self.seal(&attempt).map_err(ReviewError)?;
        let pins = self.sink.snapshot(candidate, &attempt).map_err(ReviewError)?;
        let receipt_manifest =
            sha(&fs::read(attempt.join("full-core-receipts.json")).map_err(|error| ReviewError(error.to_string()))?);
        let public = self.public_checks(&context, cancel).await.map_err(ReviewError)?;
        let result_path = PathBuf::from(context.expand(&self.config.review_result_path));
        if result_path.exists() {
            return Err(ReviewError("independent review result already exists before its original invocation".into()));
        }
        let adapter = self.process(&self.config.review_command, &context, cancel).await.map_err(ReviewError)?;
        if !adapter.success {
            self.latch_unknown();
        }
        no_link(&result_path).map_err(ReviewError)?;
        let canonical_result = fs::canonicalize(&result_path).map_err(|error| ReviewError(error.to_string()))?;
        if !canonical_result.starts_with(fs::canonicalize(&attempt).map_err(|error| ReviewError(error.to_string()))?) {
            return Err(ReviewError("review result must be a new file inside this attempt evidence directory".into()));
        }
        let raw = fs::read(&result_path).map_err(|error| ReviewError(error.to_string()))?;
        let result: ReviewResult = serde_json::from_slice(&raw).map_err(|error| ReviewError(error.to_string()))?;
        if result.effects_unknown {
            self.latch_unknown();
        }
        let pins_intact = pins
            .iter()
            .all(|(name, expected)| fs::read(attempt.join(name)).is_ok_and(|bytes| sha(&bytes) == *expected))
            && fs::read(attempt.join("full-core-receipts.json")).is_ok_and(|bytes| sha(&bytes) == receipt_manifest);
        let source_intact = self.intact(&context, &frozen).map_err(ReviewError)?;
        let statuses_known = matches!(result.public_status.as_str(), "PASS" | "FAIL" | "NOT_RUN")
            && matches!(result.hidden_status.as_str(), "PASS" | "FAIL" | "NOT_RUN")
            && matches!(result.status.as_str(), "PASS" | "FAIL");
        let pass = result.public_status == "PASS"
            && result.hidden_status == "PASS"
            && result.public_executed
            && result.hidden_executed
            && result.hidden_passed == Some(result.hidden_total)
            && public.success;
        let hidden_consistent = match result.hidden_status.as_str() {
            "PASS" => result.hidden_executed && result.hidden_passed == Some(result.hidden_total),
            "FAIL" => result.hidden_executed && result.hidden_passed.is_some_and(|count| count < result.hidden_total),
            "NOT_RUN" => !result.hidden_executed && result.hidden_passed.is_none() && result.compile_known_failure,
            _ => false,
        };
        let public_consistent = match result.public_status.as_str() {
            "PASS" | "FAIL" => result.public_executed,
            "NOT_RUN" => !result.public_executed && result.compile_known_failure,
            _ => false,
        };
        let statuses_consistent = hidden_consistent
            && public_consistent
            && !result.stage.is_empty()
            && result.hidden_total == self.config.expected_hidden_total
            && (result.status == "PASS") == pass;
        let known = adapter.success
            && statuses_known
            && statuses_consistent
            && !result.effects_unknown
            && result.source_integrity_after_evaluation
            && result.oracle_unchanged_after_evaluation
            && source_intact
            && pins_intact
            && !self.effects_unknown.load(Ordering::SeqCst);
        if !known {
            self.latch_unknown();
        }
        let verdict = if !known {
            ReviewVerdict::Unknown {
                reason: "project, Full receipt, evaluator, or process settlement is unproven".into(),
            }
        } else if pass {
            ReviewVerdict::Verified
        } else {
            let hidden = if let Some(passed) = result.hidden_passed {
                format!("Hidden contract checks passed {passed}/{}.", result.hidden_total)
            } else {
                format!("Private checks NOT_RUN (planned {}).", result.hidden_total)
            };
            ReviewVerdict::NeedsChanges {
                feedback: format!(
                    "Independent project verification failed. {hidden} Repair the original project using its public requirements. Public checks only:\n{}",
                    public.output
                ),
            }
        };
        let evidence = attempt.join("host-review.json");
        save(&evidence,&json!({"basis":candidate.basis,"source_manifest_sha256":sha(&serde_json::to_vec(&frozen).unwrap()),"full_pins_intact":pins_intact,"source_intact":source_intact,"public_status":if public.success {"PASS"}else{"FAIL"},"hidden_passed":result.hidden_passed,"hidden_total":result.hidden_total,"known":known,"verdict":format!("{verdict:?}"),"adapter_result_sha256":sha(&raw),"task_accepted":false})).map_err(ReviewError)?;
        let receipt = ReviewReceipt {
            basis: candidate.basis.clone(),
            artifacts: frozen
                .iter()
                .map(|(name, revision)| ArtifactEvidence {
                    artifact_id: name.clone(),
                    revision: revision.clone(),
                    evidence_ref: evidence.to_string_lossy().into_owned(),
                })
                .collect(),
            settlement: if known {
                Settlement::Settled
            } else {
                Settlement::Unknown { reason: "Host reconciliation required".into() }
            },
            commit: ReceiptCommit::Committed { receipt_id: format!("project-review-{}", candidate.basis.attempt) },
            verdict,
        };
        settlement.settled = true;
        Ok(receipt)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args_os().collect::<Vec<_>>();
    if args.get(1).and_then(|value| value.to_str()) != Some("--run-live") {
        println!(
            "Opt-in: --run-live CONFIG_JSON. Set ARA_MODEL_BASE_URL, ARA_MODEL_ID, ARA_MODEL_API_KEY. Uses one existing public project, new evidence directory, 32 logical requests, 3 attempts, 900 seconds, 16384 max tokens. Config generation_policy defaults to provider_default; thinking_type_disabled explicitly opts into the Host thinking.type=disabled wire contract. No model call occurs in usage mode."
        );
        return Ok(());
    }
    if args.len() != 3 {
        return Err("expected --run-live CONFIG_JSON".into());
    }
    let config: ProjectConfig = serde_json::from_slice(&fs::read(&args[2])?)?;
    config.validate()?;
    let model_id = std::env::var("ARA_MODEL_ID")?;
    let base_url = std::env::var("ARA_MODEL_BASE_URL")?;
    let key = std::env::var("ARA_MODEL_API_KEY")?;
    let mut route = reqwest::Url::parse(&base_url)?;
    let _ = route.set_username("");
    let _ = route.set_password(None);
    route.set_query(None);
    route.set_fragment(None);
    fs::create_dir(&config.evidence_root)?;
    save(&config.evidence_root.join("host-config.json"), &serde_json::to_value(&config)?)?;
    let cancel = CancellationToken::new();
    let journal = SessionJournal::create(&config.evidence_root.join("session"), &config.project_root)?;
    let session_id = journal.session_id().to_owned();
    let sink = Arc::new(JournalSink {
        journal: Mutex::new(journal),
        failure: Mutex::new(None),
        entries: Mutex::new(vec![]),
        recording: RecordingSink::default(),
        cancel: cancel.clone(),
    });
    let paths = config.readonly_paths.iter().chain(&config.writable_paths).cloned().collect::<Vec<_>>();
    let protected = config
        .readonly_paths
        .iter()
        .map(|relative| Ok((relative.clone(), sha(&fs::read(checked_file(&config.project_root, relative)?)?))))
        .collect::<Result<BTreeMap<_, _>, Box<dyn std::error::Error>>>()?;
    let deadline = Instant::now() + Duration::from_secs(900);
    let host = Arc::new(ProjectHost {
        config: config.clone(),
        paths,
        protected,
        sink: sink.clone(),
        sequence: AtomicUsize::new(0),
        check_sequence: AtomicUsize::new(0),
        effects_unknown: AtomicBool::new(false),
        cancel: cancel.clone(),
        deadline,
    });
    let initial = host.manifest(&config.project_root)?;
    let criteria_revision = sha(&serde_json::to_vec(&config)?);
    let prompt = format!(
        "Inspect this unfamiliar C++ project's public requirements and implementation using read. Repair the requested behavior in the listed writable originals. Use write to replace a complete file and check_project to run the fixed public checks. Submit a candidate by finishing your answer; the Host independently evaluates actual sealed artifacts and may provide bounded repair feedback in this same Session. {PUBLIC_PASS_SUBMISSION} Preserve protected files and function interfaces. Readonly files: {}. Writable implementation files: {}. A successful Run is not Task acceptance.",
        config.readonly_paths.join(", "),
        config.writable_paths.join(", ")
    );
    save(
        &config.evidence_root.join("trial-plan.json"),
        &json!({"scope":"public Full Core project Host consumer; original failures retained; feedback against reused oracle is adaptive validation","requested_model":model_id,"wire_model_echo":"UNKNOWN","remote_weights":"UNKNOWN","api":"openai-completions","route":route.as_str(),"logical_request_limit":32,"attempt_limit":3,"deadline_seconds":900,"max_tokens":16384,"generation_policy":config.generation_policy,"catalog_reasoning":false,"host_explicit_wire_contract":if config.generation_policy == GenerationPolicy::ThinkingTypeDisabled {"thinking_type_disabled"} else {"none_provider_default"},"remote_policy_effect":"NOT_VERIFIED","initial_files":initial,"criteria_revision":criteria_revision,"prompt":prompt,"task_accepted":false,"process_scope":"original child only; not all descendants"}),
    )?;
    let context = ToolContext::new(&config.project_root);
    let tools: Vec<Arc<dyn AgentTool>> = vec![
        Arc::new(FileTool { name: "read", inner: Arc::new(ReadTool { ctx: context.clone() }), host: host.clone() }),
        Arc::new(FileTool { name: "write", inner: Arc::new(WriteTool::new(context)), host: host.clone() }),
        Arc::new(CheckTool(host.clone())),
    ];
    let provider = Arc::new(OpenAICompletionsProvider {
        client: reqwest::Client::new(),
        base: stream_options(key, config.generation_policy)?,
    });
    let agent_config = AgentConfig {
        model: Model {
            id: model_id,
            api: "openai-completions".into(),
            provider: "explicit-host-route".into(),
            base_url,
            reasoning: false,
            max_tokens: Some(16384),
            context_window: None,
            tokenizer: None,
        },
        provider,
        system_prompt: vec![format!(
            "Repair the supplied project using only its public files and fixed tools. Treat compiler and test results as evidence. {PUBLIC_PASS_SUBMISSION} Do not claim Task acceptance."
        )],
        tools,
        tool_choice: None,
        max_tokens: Some(16384),
        temperature: None,
        deadline: None,
        max_model_calls: Some(32),
        hooks: Arc::new(NoHooks),
        interrupt_mode: InterruptMode::Immediate,
    };
    let agent = Agent::new(agent_config.clone(), vec![]);
    let signal_cancel = cancel.clone();
    let signal = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            signal_cancel.cancel();
        }
    });
    let handle = Workflow::start(
        agent,
        host.clone(),
        WorkflowRequest {
            basis: WorkflowBasis {
                workflow_id: format!("project-{}", uuid::Uuid::new_v4()),
                session_id,
                session_generation: "1".into(),
                criteria_revision,
                artifact_evidence_basis: sha(&serde_json::to_vec(&initial)?),
            },
            inputs: vec![AgentInput::from(Message::User(UserMessage::text(prompt)))],
        },
        agent_config,
        sink.clone(),
        WorkflowPolicy {
            max_attempts: NonZeroUsize::new(3).unwrap(),
            max_prepared_requests: NonZeroUsize::new(32).unwrap(),
            deadline,
        },
        cancel,
    )?;
    let outcome = handle.wait().await?;
    signal.abort();
    fs::write(config.evidence_root.join("agent-events.txt"), format!("{:#?}", sink.recording.events.lock().await))?;
    fs::write(config.evidence_root.join("workflow-outcome.txt"), format!("{outcome:#?}"))?;
    save(
        &config.evidence_root.join("result.json"),
        &json!({"stop":format!("{:?}",outcome.stop),"attempts":outcome.attempts.len(),"logical_prepared_requests":outcome.prepared_requests,"effects_unknown":host.effects_unknown.load(Ordering::SeqCst),"reconciliation_required":outcome.reconciliation.is_some(),"wire_model_echo":"UNKNOWN","task_accepted":false}),
    )?;
    println!(
        "stop={:?}; attempts={}; logical_requests={}; evidence={}",
        outcome.stop,
        outcome.attempts.len(),
        outcome.prepared_requests,
        config.evidence_root.display()
    );
    if outcome.reconciliation.is_some() {
        return Err("Host reconciliation required; original effects are not replayed".into());
    }
    if outcome.stop != WorkflowStop::VerifiedArtifact {
        return Err("project artifact not verified; Task remains unaccepted".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn capture_generation_bodies(
        options: [ara_ai::providers::openai_completions::StreamOptions; 2],
    ) -> Vec<Value> {
        use ara_ai::{CallOptions, Context, ModelProvider, StopReason, event::AssistantMessageEvent};
        let response = json!({"events":[
            {"data":{"id":"controlled-generation","choices":[{"delta":{"content":"candidate submitted"}}]}},
            {"data":{"id":"controlled-generation","choices":[{"delta":{},"finish_reason":"stop"}]}},
            {"done":true}
        ]});
        let server = ara_testkit::FakeUpstream::start(
            serde_json::from_value(json!({"responses":[response.clone(),response]})).unwrap(),
            None,
        )
        .await
        .unwrap();
        let model = Model {
            id: "controlled-project-model".into(),
            api: "openai-completions".into(),
            provider: "explicit-host-route".into(),
            base_url: server.base_url(),
            reasoning: false,
            max_tokens: Some(16384),
            context_window: None,
            tokenizer: None,
        };
        let context = Context {
            system_prompt: vec![PUBLIC_PASS_SUBMISSION.into()],
            messages: vec![Message::User(UserMessage::text("submit the candidate"))],
            tools: Some(vec![Tool {
                name: "check_project".into(),
                description: "Run public checks".into(),
                parameters: json!({"type":"object","properties":{},"additionalProperties":false}),
                ..Default::default()
            }]),
        };
        for base in options {
            let provider = OpenAICompletionsProvider { client: reqwest::Client::new(), base };
            tokio::time::timeout(Duration::from_secs(10), async {
                let mut stream =
                    provider.stream(&model, &context, CallOptions { max_tokens: Some(16384), ..Default::default() });
                let mut terminal_count = 0;
                while let Some(event) = stream.recv().await {
                    terminal_count += usize::from(event.is_terminal());
                    match event {
                        AssistantMessageEvent::Done { reason, message } => {
                            assert_eq!(reason, StopReason::Stop);
                            assert_eq!(message.text(), "candidate submitted");
                        }
                        AssistantMessageEvent::Error { error, .. } => panic!("controlled generation failed: {error:?}"),
                        _ => {}
                    }
                }
                assert_eq!(terminal_count, 1, "the real HTTP provider must finish exactly once");
            })
            .await
            .expect("controlled HTTP stream timed out");
        }
        assert!(!model.reasoning, "Host wire policy must not change the catalog model flag");
        assert_eq!(server.served(), 2);
        let requests = server.requests.lock().await;
        assert_eq!(requests.len(), 2);
        for request in requests.iter() {
            assert_eq!(request["request"], "POST /v1/chat/completions HTTP/1.1");
            assert_eq!(request["body"]["max_tokens"], 16384);
            assert_eq!(request["body"]["model"], "controlled-project-model");
            assert!(request["body"].get("temperature").is_none());
        }
        requests.iter().map(|request| request["body"].clone()).collect()
    }

    #[tokio::test]
    async fn provider_default_preserves_original_http_body_and_rejects_unknown_generation_config() {
        let root = tempfile::tempdir().unwrap();
        let (host, _, _) = native_fixture(root.path());
        let mut config = serde_json::to_value(&host.config).unwrap();
        config.as_object_mut().unwrap().remove("generation_policy");
        let legacy: ProjectConfig = serde_json::from_value(config.clone()).unwrap();
        assert_eq!(legacy.generation_policy, GenerationPolicy::ProviderDefault);
        for invalid in [
            json!(false),
            json!(null),
            json!("auto"),
            json!("ThinkingTypeDisabled"),
            json!({"thinking_type_disabled":true}),
        ] {
            config["generation_policy"] = invalid;
            assert!(serde_json::from_value::<ProjectConfig>(config.clone()).is_err());
        }
        config["generation_policy"] = json!("thinking_type_disabled");
        assert_eq!(
            serde_json::from_value::<ProjectConfig>(config).unwrap().generation_policy,
            GenerationPolicy::ThinkingTypeDisabled
        );
        let original = ara_ai::providers::openai_completions::StreamOptions {
            api_key: Some("controlled-test-key".into()),
            ..Default::default()
        };
        let body = capture_generation_bodies([
            original,
            stream_options("controlled-test-key".into(), legacy.generation_policy).unwrap(),
        ])
        .await;
        assert_eq!(body[0], body[1], "omitting generation_policy must preserve the complete original request body");
        for key in [
            "thinking",
            "reasoning",
            "reasoning_effort",
            "enable_thinking",
            "chat_template_kwargs",
            "preserve_thinking",
            "venice_parameters",
        ] {
            assert!(body[1].get(key).is_none(), "provider_default unexpectedly added {key}");
        }
    }

    #[tokio::test]
    async fn explicit_disabled_policy_changes_real_http_body_only_by_thinking_type() {
        let body = capture_generation_bodies([
            stream_options("controlled-test-key".into(), GenerationPolicy::ProviderDefault).unwrap(),
            stream_options("controlled-test-key".into(), GenerationPolicy::ThinkingTypeDisabled).unwrap(),
        ])
        .await;
        let mut expected = body[0].clone();
        expected["thinking"] = json!({"type":"disabled"});
        assert_eq!(body[1], expected, "explicit Host wire contract must add exactly thinking.type=disabled");
    }

    #[test]
    fn ordinary_case_mismatch_and_unresolved_child_receipts_are_distinct() {
        assert!(public_case_is_known(&json!({"exit_code":0,"errors":[]})).is_ok());
        assert!(public_case_is_known(&json!({"exit_code":1,"errors":["stdout mismatch","snapshot differs"]})).is_ok());
        for prefix in [
            "Empty",
            "queue.Empty",
            "TimeoutExpired",
            "TimeoutError",
            "OSError",
            "PermissionError",
            "FileNotFoundError",
        ] {
            assert!(
                public_case_is_known(&json!({"exit_code":1,"errors":[format!("{prefix}: retained exception")]}))
                    .is_err()
            );
        }
        for receipt in [
            json!({"exit_code":null,"errors":[]}),
            json!({"exit_code":1.5,"errors":[]}),
            json!({"errors":[]}),
            json!({"exit_code":1}),
            json!({"exit_code":1,"errors":[null]}),
            json!({"exit_code":1,"errors":"timeout"}),
        ] {
            assert!(public_case_is_known(&receipt).is_err());
        }
    }

    fn native_command(sleep: bool) -> CommandSpec {
        #[cfg(windows)]
        let argv = vec![
            PathBuf::from(std::env::var_os("SystemRoot").unwrap())
                .join("System32/WindowsPowerShell/v1.0/powershell.exe")
                .to_string_lossy()
                .into_owned(),
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            if sleep { "Start-Sleep -Seconds 30".into() } else { "exit 8".into() },
        ];
        #[cfg(not(windows))]
        let argv = if sleep {
            vec!["/bin/sleep".into(), "30".into()]
        } else {
            vec!["/bin/sh".into(), "-c".into(), "exit 8".into()]
        };
        CommandSpec { argv, timeout_seconds: if sleep { 1 } else { 5 } }
    }

    fn native_fixture(root: &Path) -> (ProjectHost, CheckContext, CancellationToken) {
        let project = root.join("project");
        let evidence = root.join("evidence");
        let build = root.join("build");
        fs::create_dir(&project).unwrap();
        fs::create_dir(&evidence).unwrap();
        fs::create_dir(&build).unwrap();
        fs::write(project.join("README.txt"), "protected requirement").unwrap();
        fs::write(project.join("source.cpp"), "int answer() { return 1; }").unwrap();
        let cancel = CancellationToken::new();
        let journal = SessionJournal::create(&evidence.join("session"), &project).unwrap();
        let command = native_command(false);
        let host = ProjectHost {
            config: ProjectConfig {
                project_root: project.clone(),
                evidence_root: evidence.clone(),
                readonly_paths: vec!["README.txt".into()],
                writable_paths: vec!["source.cpp".into()],
                public_check_commands: vec![command.clone()],
                public_case_receipts: vec!["{build}/ctest-T01/case.json".into()],
                review_command: command,
                review_result_path: "{evidence}/review-result.json".into(),
                expected_hidden_total: 1,
                generation_policy: GenerationPolicy::ProviderDefault,
            },
            paths: vec!["README.txt".into(), "source.cpp".into()],
            protected: BTreeMap::from([("README.txt".into(), sha(b"protected requirement"))]),
            sink: Arc::new(JournalSink {
                journal: Mutex::new(journal),
                failure: Mutex::new(None),
                entries: Mutex::new(vec![]),
                recording: RecordingSink::default(),
                cancel: cancel.clone(),
            }),
            sequence: AtomicUsize::new(0),
            check_sequence: AtomicUsize::new(0),
            effects_unknown: AtomicBool::new(false),
            cancel: cancel.clone(),
            deadline: Instant::now() + Duration::from_secs(20),
        };
        (host, CheckContext { source: project, build, evidence }, cancel)
    }

    fn native_file_tools(host: ProjectHost) -> (FileTool, FileTool) {
        let context = ToolContext::new(&host.config.project_root);
        let host = Arc::new(host);
        (
            FileTool { name: "read", inner: Arc::new(ReadTool { ctx: context.clone() }), host: host.clone() },
            FileTool { name: "write", inner: Arc::new(WriteTool::new(context)), host },
        )
    }

    async fn execute_file(tool: &FileTool, args: Value) -> Result<ToolOutput, ToolError> {
        tool.execute("native-file", args.as_object().unwrap().clone(), CancellationToken::new(), Arc::new(|_| {})).await
    }

    fn output_text(output: &ToolOutput) -> String {
        output
            .content
            .iter()
            .filter_map(|block| match block {
                ara_ai::UserBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tokio::test]
    async fn native_read_recovers_omitted_middle_and_tail_without_raising_default_limits() {
        let root = tempfile::tempdir().unwrap();
        let (host, context, _) = native_fixture(root.path());
        let text = (1..=1200)
            .map(|line| match line {
                1 => "void fixture() {\n".to_owned(),
                900 => "  // MIDDLE_WINDOW_RECOVERED\n".to_owned(),
                1199 => "  // TAIL_WINDOW_RECOVERED\n".to_owned(),
                1200 => "}\n".to_owned(),
                _ => format!("  consume({line});\n"),
            })
            .collect::<String>();
        fs::write(context.source.join("source.cpp"), &text).unwrap();
        let (read, _) = native_file_tools(host);
        let bare = execute_file(&read, json!({"path":"source.cpp"})).await.unwrap();
        let bare_text = output_text(&bare);
        assert!(
            !bare_text.contains("MIDDLE_WINDOW_RECOVERED"),
            "bare read unexpectedly emitted the middle marker: {bare_text}"
        );
        assert!(
            bare_text.contains("elided")
                || bare_text.contains("more lines")
                || bare.details.as_ref().is_some_and(|details| details["truncation"]["truncated"] == true),
            "bare read must retain a real omission/truncation receipt: {bare_text}"
        );
        for selector in ["source.cpp:895-905", "source.cpp:895+11"] {
            let output = execute_file(&read, json!({"path":selector})).await.unwrap();
            assert!(
                output_text(&output).contains("MIDDLE_WINDOW_RECOVERED"),
                "middle window not returned for {selector}"
            );
        }
        let multi = execute_file(&read, json!({"path":"source.cpp:895-905,1195-1200"})).await.unwrap();
        assert!(output_text(&multi).contains("MIDDLE_WINDOW_RECOVERED"));
        assert!(output_text(&multi).contains("TAIL_WINDOW_RECOVERED"));
        let tail = execute_file(&read, json!({"path":"source.cpp:-10"})).await.unwrap();
        assert!(output_text(&tail).contains("TAIL_WINDOW_RECOVERED"));
        assert!(!output_text(&tail).contains("MIDDLE_WINDOW_RECOVERED"));
        assert_eq!(fs::read_to_string(context.source.join("source.cpp")).unwrap(), text);
    }

    #[tokio::test]
    async fn native_file_grants_reject_unbounded_or_foreign_reads_and_all_write_selectors() {
        let root = tempfile::tempdir().unwrap();
        let (host, context, _) = native_fixture(root.path());
        let (read, write) = native_file_tools(host);
        let before = fs::read(context.source.join("source.cpp")).unwrap();
        for path in [
            "other.cpp:1-3",
            "../source.cpp:1-3",
            "./source.cpp:1-3",
            "source.cpp/../source.cpp:1-3",
            "/source.cpp:1-3",
            "C:/source.cpp:1-3",
            "https://example.com/source.cpp:1-3",
            "skill://source.cpp:1-3",
            "source.cpp%3A1-3",
            "source.cpp%00:1-3",
            "sub\\source.cpp:1-3",
            "source.cpp:0-5",
            "source.cpp:9-3",
            "source.cpp:1+0",
            "source.cpp:1-3,",
            "source.cpp:-0",
            "source.cpp:bogus",
            "source.cpp:raw",
            "source.cpp:raw:1-3",
            "source.cpp:1-3:raw",
            "source.cpp:1",
            "source.cpp:1-",
            "source.cpp:1-3,8-",
            "source.cpp:1-3:8-9",
        ] {
            assert!(execute_file(&read, json!({"path":path})).await.is_err(), "read unexpectedly admitted {path}");
        }
        for path in
            ["source.cpp:1-3", "source.cpp:-2", "source.cpp:raw", "README.txt", "README.txt:1-3", "../source.cpp"]
        {
            assert!(
                execute_file(&write, json!({"path":path,"content":"unauthorized replacement"})).await.is_err(),
                "write unexpectedly admitted {path}"
            );
        }
        assert_eq!(fs::read(context.source.join("source.cpp")).unwrap(), before);
        let protected = execute_file(&read, json!({"path":"README.txt"})).await.unwrap();
        assert!(output_text(&protected).contains("protected requirement"));
        execute_file(&write, json!({"path":"source.cpp","content":"int answer() { return 2; }"})).await.unwrap();
        assert_eq!(fs::read_to_string(context.source.join("source.cpp")).unwrap(), "int answer() { return 2; }");
        assert_eq!(fs::read_to_string(context.source.join("README.txt")).unwrap(), "protected requirement");
    }

    #[test]
    fn read_schema_allows_line_windows_while_write_schema_preserves_exact_originals() {
        let root = tempfile::tempdir().unwrap();
        let (host, _, _) = native_fixture(root.path());
        let (read, write) = native_file_tools(host);
        let read_schema = read.definition();
        assert_eq!(read_schema.parameters["properties"]["path"]["type"], "string");
        assert!(read_schema.parameters["properties"]["path"].get("enum").is_none());
        assert!(read_schema.description.contains("source.cpp"));
        assert!(read_schema.description.contains(":N-M"));
        assert_eq!(write.definition().parameters["properties"]["path"]["enum"], json!(["source.cpp"]));
    }

    #[tokio::test]
    async fn native_literal_colon_or_ads_collision_cannot_change_the_authorized_base() {
        let root = tempfile::tempdir().unwrap();
        let (host, context, _) = native_fixture(root.path());
        let collision = context.source.join("source.cpp:2-4");
        let created = fs::write(&collision, "LITERAL_COLLISION_MUST_NOT_BE_READ");
        match fs::symlink_metadata(&collision) {
            Ok(_) => {
                created.unwrap();
                let (read, _) = native_file_tools(host);
                let raw = read
                    .inner
                    .execute(
                        "literal-control",
                        json!({"path":"source.cpp:2-4"}).as_object().unwrap().clone(),
                        CancellationToken::new(),
                        Arc::new(|_| {}),
                    )
                    .await
                    .unwrap();
                assert!(output_text(&raw).contains("LITERAL_COLLISION_MUST_NOT_BE_READ"));
                assert!(execute_file(&read, json!({"path":"source.cpp:2-4"})).await.is_err());
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound
                        | std::io::ErrorKind::NotADirectory
                        | std::io::ErrorKind::InvalidFilename
                ) =>
            {
                eprintln!(
                    "literal colon / Windows ADS collision coverage unavailable on this filesystem: creation={created:?}, lstat={error:?}"
                );
                let (read, _) = native_file_tools(host);
                assert!(read.authorize_read("source.cpp:2-4").is_ok());
            }
            Err(error) => {
                eprintln!(
                    "literal collision probe is unknown on this filesystem: creation={created:?}, lstat={error:?}"
                );
                let (read, _) = native_file_tools(host);
                assert!(execute_file(&read, json!({"path":"source.cpp:2-4"})).await.is_err());
            }
        }
    }

    fn process_receipt(context: &CheckContext) -> Value {
        serde_json::from_slice(&fs::read(context.evidence.join("process-0.json")).unwrap()).unwrap()
    }

    #[tokio::test]
    async fn native_timeout_cancels_the_shared_workflow_token_and_retains_original_child_receipt() {
        let root = tempfile::tempdir().unwrap();
        let (host, context, shared) = native_fixture(root.path());
        let result = tokio::time::timeout(
            Duration::from_secs(8),
            host.process(&native_command(true), &context, &shared.child_token()),
        )
        .await
        .unwrap();
        assert!(result.is_err());
        assert!(shared.is_cancelled());
        assert!(host.effects_unknown.load(Ordering::SeqCst));
        assert!(host.admit(&CancellationToken::new()).is_err());
        let receipt = process_receipt(&context);
        assert_eq!(receipt["timed_out"], true);
        assert_eq!(receipt["child_settlement"], "REAPED");
        assert_eq!(receipt["descendant_settlement"], "NOT_PROVEN");
        assert_eq!(receipt["effects_unknown"], true);
        assert!(receipt["pid"].as_u64().is_some());
        assert!(context.evidence.join("process-0.started.json").is_file());
        assert_eq!(host.sequence.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn native_start_receipt_failure_cancels_shared_admission_and_still_reaps_original_child() {
        let root = tempfile::tempdir().unwrap();
        let (host, context, shared) = native_fixture(root.path());
        fs::create_dir(context.evidence.join("process-0.started.json")).unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(8),
            host.process(&native_command(true), &context, &shared.child_token()),
        )
        .await
        .unwrap();
        assert!(result.is_err());
        assert!(shared.is_cancelled());
        assert!(host.effects_unknown.load(Ordering::SeqCst));
        assert!(host.admit(&CancellationToken::new()).is_err());
        let receipt = process_receipt(&context);
        assert_eq!(receipt["cancelled"], true);
        assert_eq!(receipt["child_settlement"], "REAPED");
        assert_eq!(receipt["effects_unknown"], true);
        assert!(receipt["pid"].as_u64().is_some());
    }

    #[tokio::test]
    async fn native_ctest_failure_with_missing_nested_receipt_is_unknown_and_cancels_next_admission() {
        let root = tempfile::tempdir().unwrap();
        let (host, context, shared) = native_fixture(root.path());
        let result = host.public_checks(&context, &shared.child_token()).await;
        assert!(result.is_err());
        assert!(shared.is_cancelled());
        assert!(host.effects_unknown.load(Ordering::SeqCst));
        assert!(host.admit(&CancellationToken::new()).is_err());
        let receipt = process_receipt(&context);
        assert_eq!(receipt["exit"], 8);
        assert_eq!(receipt["timed_out"], false);
        assert_eq!(receipt["cancelled"], false);
        assert_eq!(receipt["child_settlement"], "REAPED");
    }

    #[tokio::test]
    async fn native_ctest_failure_with_authoritative_mismatch_receipt_remains_known() {
        let root = tempfile::tempdir().unwrap();
        let (host, context, shared) = native_fixture(root.path());
        fs::create_dir(context.build.join("ctest-T01")).unwrap();
        save(&context.build.join("ctest-T01/case.json"), &json!({"exit_code":1,"errors":["stdout mismatch"]})).unwrap();
        let result = host.public_checks(&context, &shared.child_token()).await.unwrap();
        assert!(!result.success);
        assert!(!shared.is_cancelled());
        assert!(!host.effects_unknown.load(Ordering::SeqCst));
        assert!(host.admit(&CancellationToken::new()).is_ok());
        assert_eq!(process_receipt(&context)["exit"], 8);
    }

    #[tokio::test]
    async fn native_configure_failure_does_not_invent_executed_public_cases() {
        let root = tempfile::tempdir().unwrap();
        let (mut host, context, shared) = native_fixture(root.path());
        host.config.public_check_commands.push(native_command(false));
        let result = host.public_checks(&context, &shared.child_token()).await.unwrap();
        assert!(!result.success);
        assert!(!shared.is_cancelled());
        assert!(!host.effects_unknown.load(Ordering::SeqCst));
        assert!(!context.build.join("ctest-T01/case.json").exists());
        assert_eq!(host.sequence.load(Ordering::SeqCst), 1);
    }
}
