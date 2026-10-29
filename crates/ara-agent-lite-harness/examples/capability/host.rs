use anyhow::{ensure, Context, Result};
use ara_agent_lite_harness::{
    core::{
        self,
        model::ModelHost,
        rpi_agent::{AgentError, AgentTool, AgentToolResult, ToolResultPartial},
        rpi_ai::{Schema, Tool},
        RunMode, RunRequest, RuntimePaths, ToolRegistration,
    },
    Candidate, Continuation, HarnessHost, ReviewReceipt, Settlement, TaskKind, TaskSpec, Verdict,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio_util::sync::CancellationToken;

#[path = "process.rs"]
pub mod process;
#[path = "tasks.rs"]
mod tasks;
pub use process::save_new;
pub use tasks::Kind;

fn calls() -> usize {
    24
}
fn runs() -> usize {
    2
}
fn seconds() -> u64 {
    240
}
fn process_seconds() -> u64 {
    30
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub kind: Kind,
    pub root: PathBuf,
    pub compiler: Option<PathBuf>,
    pub objdump: Option<PathBuf>,
    pub strip: Option<PathBuf>,
    #[serde(default = "calls")]
    pub max_calls: usize,
    #[serde(default = "runs")]
    pub max_runs: usize,
    #[serde(default = "seconds")]
    pub max_seconds: u64,
    #[serde(default = "process_seconds")]
    pub process_timeout_seconds: u64,
}
pub struct RouteConfig {
    pub endpoint: String,
    pub model: String,
    pub token: String,
}
pub struct Host {
    pub config: Config,
    pub private: PathBuf,
    pub processes: Arc<process::Processes>,
    engine: Arc<Engine>,
    route: Mutex<Option<RouteConfig>>,
    model_host: Arc<dyn ModelHost>,
    tools: Vec<Arc<dyn AgentTool>>,
    steering: Arc<Mutex<core::steering::SteeringBuffer>>,
    reviews: AtomicUsize,
}
struct Engine {
    config: Config,
    workspace: PathBuf,
    private: PathBuf,
    processes: Arc<process::Processes>,
    checks: AtomicUsize,
    read_spec: AtomicBool,
    inspected_binary: AtomicBool,
    object_hash: Option<String>,
    immutable: Vec<(String, String)>,
}
struct Evaluation {
    passed: bool,
    feedback: String,
    report: PathBuf,
}

impl Host {
    pub async fn prepare(config: Config, live: bool) -> Result<Self> {
        ensure!(config.root.is_absolute() && !config.root.exists(), "root must be a fresh absolute directory");
        ensure!(
            !config.root.components().any(|p| matches!(p, std::path::Component::ParentDir)),
            "root cannot contain parent traversal"
        );
        ensure!(config.root.parent().is_some_and(Path::is_dir), "root parent must already exist");
        ensure!(
            (1..=64).contains(&config.max_calls) && (1..=4).contains(&config.max_runs),
            "bounded positive calls/runs required"
        );
        ensure!(
            (10..=900).contains(&config.max_seconds) && (1..=120).contains(&config.process_timeout_seconds),
            "bounded seconds required"
        );
        if config.kind != Kind::Reasoning {
            process::require_program(config.compiler.as_deref().context("explicit GNU C++ compiler required")?)?;
        }
        if config.kind == Kind::Reverse {
            process::require_program(config.objdump.as_deref().context("explicit GNU objdump required")?)?;
            process::require_program(config.strip.as_deref().context("explicit GNU strip required")?)?;
        }
        fs::create_dir(&config.root)?;
        let workspace = config.root.join("workspace");
        let private = config.root.join("host");
        fs::create_dir(&workspace)?;
        fs::create_dir(&private)?;
        fs::create_dir(config.root.join("state"))?;
        save_new(
            &private.join("configuration.json"),
            &json!({"config":config,"mode":if live{"live"}else{"controlled"},"model_configuration":"environment only; not persisted","os_sandbox":false}),
        )?;
        tasks::prepare(config.kind, &workspace, &private)?;
        let processes = Arc::new(process::Processes::new(
            private.join("processes"),
            Duration::from_secs(config.process_timeout_seconds),
        )?);
        let object_hash = if config.kind == Kind::Reverse {
            let compiler = config.compiler.as_deref().unwrap();
            let object = workspace.join("mystery.o");
            let compiled = processes
                .run(
                    "build-self-owned-reverse-object",
                    compiler,
                    vec![
                        "-std=c++17".into(),
                        "-O1".into(),
                        "-fno-exceptions".into(),
                        "-fno-asynchronous-unwind-tables".into(),
                        "-c".into(),
                        private.join("original-transform.cpp").to_string_lossy().into_owned(),
                        "-o".into(),
                        object.to_string_lossy().into_owned(),
                    ],
                    &workspace,
                    CancellationToken::new(),
                )
                .await?;
            ensure!(compiled.success(), "reverse fixture compilation failed; inspect process receipt");
            let before = process::hash(&fs::read(&object)?);
            let stripped = processes
                .run(
                    "strip-self-owned-object",
                    config.strip.as_deref().unwrap(),
                    vec!["--strip-all".into(), object.to_string_lossy().into_owned()],
                    &workspace,
                    CancellationToken::new(),
                )
                .await?;
            ensure!(stripped.success(), "reverse fixture strip failed; inspect process receipt");
            let after = process::hash(&fs::read(&object)?);
            save_new(
                &private.join("reverse-provenance.json"),
                &json!({"self_owned":true,"source_sha256":process::hash(tasks::REVERSE_SOURCE.as_bytes()),"object_before_strip_sha256":before,"object_sha256":after,"build_receipt":compiled.receipt_path,"strip_receipt":stripped.receipt_path}),
            )?;
            Some(after)
        } else {
            None
        };
        let immutable = tasks::read_paths(config.kind)
            .into_iter()
            .filter(|name| *name != tasks::candidate(config.kind))
            .map(|name| Ok((name.to_owned(), process::hash(&fs::read(workspace.join(name))?))))
            .collect::<Result<Vec<_>>>()?;
        let engine = Arc::new(Engine {
            config: config.clone(),
            workspace,
            private: private.clone(),
            processes: processes.clone(),
            checks: AtomicUsize::new(0),
            read_spec: AtomicBool::new(false),
            inspected_binary: AtomicBool::new(false),
            object_hash,
            immutable,
        });
        let mut tools: Vec<Arc<dyn AgentTool>> = Vec::new();
        let readable = tasks::read_paths(config.kind);
        for (kind,name,description,properties,required) in [
            (ToolKind::Read,"read_task","Read one explicitly granted task file; no absolute paths or Host files.",json!({"path":{"type":"string","enum":readable}}),vec!["path"]),
            (ToolKind::Write,"write_task","Replace the single authorized candidate file with complete text. This is submission, not acceptance.",json!({"path":{"type":"string","enum":[tasks::candidate(config.kind)]},"content":{"type":"string"}}),vec!["path","content"]),
            (ToolKind::Check,"check_candidate","Compile and validate the current artifact using the Host's fixed commands and independent checks. Hidden inputs and oracle files are not returned.",json!({}),vec![]),
        ] {
            tools.push(Arc::new(TaskTool{kind,engine:engine.clone(),schema:Tool{name:name.into(),description:description.into(),parameters:Schema(json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})),constrained_sampling:None}}));
        }
        if config.kind == Kind::Reverse {
            tools.push(Arc::new(TaskTool{kind:ToolKind::Inspect,engine:engine.clone(),schema:Tool{name:"inspect_binary".into(),description:"Run the configured GNU objdump on the one granted stripped mystery.o; no arbitrary arguments or files.".into(),parameters:Schema(json!({"type":"object","properties":{},"additionalProperties":false})),constrained_sampling:None}}));
        }
        Ok(Self {
            config,
            private,
            processes,
            engine,
            route: Mutex::new(None),
            model_host: Arc::new(core::model::CatalogModelHost),
            tools,
            steering: Arc::new(Mutex::new(core::steering::SteeringBuffer::default())),
            reviews: AtomicUsize::new(0),
        })
    }
    pub fn set_route(&self, route: RouteConfig) -> Result<()> {
        ensure!(
            !route.endpoint.is_empty() && !route.model.is_empty() && !route.token.is_empty(),
            "explicit nonempty model settings required"
        );
        let mut slot = self.route.lock().unwrap();
        ensure!(slot.is_none(), "route already configured");
        *slot = Some(route);
        Ok(())
    }
    pub fn objective(&self) -> String {
        tasks::objective(self.config.kind).into()
    }
    pub fn request(&self, number: usize, objective: String) -> Result<RunRequest> {
        let route = self.route.lock().unwrap();
        let route = route.as_ref().context("route not configured")?;
        let mut request = RunRequest::new(
            format!("capability-run-{number}"),
            objective,
            RuntimePaths::new(
                &self.engine.workspace,
                self.config.root.join("state/session.json"),
                self.config.root.join("state/environment"),
            ),
        );
        request.endpoint = route.endpoint.clone();
        request.token = route.token.clone();
        request.model = route.model.clone();
        request.route = core::model::Route {
            model: route.model.clone(),
            protocol: "openai-completions".into(),
            binding: "capability-host-v1".into(),
            decision: "explicit-host-selection".into(),
        };
        request.model_host = self.model_host.clone();
        request.steering = self.steering.clone();
        request.device_fp = "capability-host-v1".into();
        request.session_key = "capability/same-session".into();
        request.mode = RunMode::Execute;
        request.enable_local_tools = false;
        request.worker_executable = None;
        request.max_calls = self.config.max_calls;
        request.max_seconds = self.config.max_seconds;
        request.max_output = 4096;
        request.extra_tools =
            self.tools.iter().map(|tool| ToolRegistration { tool: tool.clone(), allow_in_plan: false }).collect();
        Ok(request)
    }
    pub fn spec(&self) -> Result<TaskSpec> {
        Ok(TaskSpec {
            task_id: format!("capability-{:?}", self.config.kind),
            criteria_revision: "capability-fixtures-v1".into(),
            authorization_revision: "single-candidate-no-shell-v1".into(),
            kind: match self.config.kind {
                Kind::Coding => TaskKind::Coding,
                Kind::Reasoning => TaskKind::Reasoning,
                Kind::Reverse => TaskKind::ReverseAnalysis,
            },
            evidence_dir: self.config.root.join("harness"),
            max_runs: self.config.max_runs,
            max_calls: self.config.max_calls,
            deadline_unix_ms: SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64
                + self.config.max_seconds * 1000,
        })
    }
    pub fn controlled_responses(&self) -> Vec<Value> {
        use crate::provider::{call, text};
        let mut responses = vec![call("controlled-read-spec", "read_task", json!({"path":"task.md"}))];
        if self.config.kind == Kind::Coding {
            responses.push(call("controlled-read-implementation", "read_task", json!({"path":"window.cpp"})));
        }
        if self.config.kind == Kind::Reverse {
            responses.push(call("controlled-disassemble", "inspect_binary", json!({})));
        }
        responses.push(call(
            "controlled-first-submission",
            "write_task",
            json!({"path":tasks::candidate(self.config.kind),"content":tasks::wrong(self.config.kind)}),
        ));
        responses
            .push(text("Submitted candidate file. Host should inspect the artifact and return independent feedback."));
        responses.push(call(
            "controlled-corrected-submission",
            "write_task",
            json!({"path":tasks::candidate(self.config.kind),"content":tasks::correct(self.config.kind)}),
        ));
        responses
            .push(text("Submitted the corrected artifact in the same Session. Host verification is still required."));
        responses
    }
}

#[async_trait]
impl HarnessHost for Host {
    async fn verify(&self, candidate: Candidate, cancellation: CancellationToken) -> Result<ReviewReceipt> {
        let review = self.reviews.fetch_add(1, Ordering::SeqCst) + 1;
        let evaluation = self.engine.evaluate(cancellation).await?;
        let unknown = self.processes.unknown();
        let receipt = ReviewReceipt {
            basis: candidate.basis.clone(),
            committed: true,
            settlement: if unknown { Settlement::Unknown } else { Settlement::Settled },
            verdict: if unknown {
                Verdict::Unknown
            } else if evaluation.passed {
                Verdict::Verified
            } else {
                Verdict::NeedsChanges { feedback: evaluation.feedback.clone() }
            },
            evidence: vec![format!(
                "{}#sha256={}",
                evaluation.report.display(),
                process::hash(&fs::read(&evaluation.report)?)
            )],
        };
        save_new(
            &self.private.join(format!("review-{review}-committed.json")),
            &json!({"basis":candidate.basis,"receipt":receipt,"actual_artifact_review":evaluation.report,"model_prose_used_as_proof":false}),
        )?;
        Ok(receipt)
    }
    async fn prepare_continuation(
        &self,
        continuation: Continuation,
        cancellation: CancellationToken,
    ) -> Result<RunRequest> {
        ensure!(
            !cancellation.is_cancelled() && !self.processes.unknown(),
            "no continuation after canceled or unknown effects"
        );
        let Verdict::NeedsChanges { feedback } = &continuation.receipt.verdict else {
            anyhow::bail!("continuation requires Host feedback")
        };
        self.request(continuation.next_run_number,format!("{}\nIndependent Host verification of the actual prior artifact failed: {feedback}\nCorrect the same authorized file using the retained context; do not replay any unknown operation. The original task budget and deadline remain unchanged.",self.objective()))
    }
}

impl Engine {
    fn candidate_path(&self) -> PathBuf {
        self.workspace.join(tasks::candidate(self.config.kind))
    }
    fn granted(&self, name: &str) -> Result<PathBuf> {
        ensure!(tasks::read_paths(self.config.kind).contains(&name), "file is not granted");
        let path = self.workspace.join(name);
        ensure!(!fs::symlink_metadata(&path)?.file_type().is_symlink(), "symlink task file refused");
        ensure!(path.canonicalize()?.starts_with(self.workspace.canonicalize()?), "task path escaped workspace");
        Ok(path)
    }
    fn check_immutable(&self) -> Result<()> {
        for (name, expected) in &self.immutable {
            ensure!(process::hash(&fs::read(self.granted(name)?)?) == *expected, "immutable task file changed");
        }
        if let Some(expected) = &self.object_hash {
            ensure!(
                process::hash(&fs::read(self.workspace.join("mystery.o"))?) == *expected,
                "stripped object changed"
            );
        }
        Ok(())
    }
    async fn evaluate(&self, signal: CancellationToken) -> Result<Evaluation> {
        let number = self.checks.fetch_add(1, Ordering::SeqCst) + 1;
        let directory = self.private.join(format!("check-{number}"));
        fs::create_dir(&directory)?;
        if let Err(error) = self.check_immutable() {
            self.processes.mark_unknown();
            save_new(&directory.join("report.json"), &json!({"status":"effects_unknown","reason":error.to_string()}))?;
            return Ok(Evaluation {
                passed: false,
                feedback: "immutable task identity changed; reconcile original evidence".into(),
                report: directory.join("report.json"),
            });
        }
        let bytes = fs::read(self.granted(tasks::candidate(self.config.kind))?)?;
        ensure!(bytes.len() <= 64 * 1024, "candidate exceeds 64 KiB");
        let artifact_hash = process::hash(&bytes);
        let snapshot = directory.join(tasks::candidate(self.config.kind));
        fs::write(&snapshot, &bytes)?;
        let mut records = Vec::new();
        let mut details = json!({});
        let (mut passed, mut feedback) = if self.processes.unknown() || signal.is_cancelled() {
            self.processes.mark_unknown();
            (false, "original effects unknown or cancellation observed; reconcile receipts".into())
        } else if !self.read_spec.load(Ordering::SeqCst) {
            (false, "read task.md before submitting".into())
        } else if self.config.kind == Kind::Reverse && !self.inspected_binary.load(Ordering::SeqCst) {
            (false, "inspect the actual stripped object before submitting".into())
        } else if self.config.kind == Kind::Reasoning {
            match tasks::validate_plan(&bytes) {
                Ok(value) => {
                    details = value;
                    (true, "constraints and exhaustive optimum verified".into())
                }
                Err(error) => (false, format!("JSON schedule rejected: {error}")),
            }
        } else {
            let compiler = self.config.compiler.as_deref().unwrap();
            let object = directory.join("candidate.o");
            let compiled = self
                .processes
                .run(
                    "compile-candidate",
                    compiler,
                    vec![
                        "-std=c++17".into(),
                        "-O0".into(),
                        "-c".into(),
                        snapshot.to_string_lossy().into_owned(),
                        "-I".into(),
                        self.workspace.to_string_lossy().into_owned(),
                        "-o".into(),
                        object.to_string_lossy().into_owned(),
                    ],
                    &self.workspace,
                    signal.clone(),
                )
                .await?;
            let compile_ok = compiled.success();
            let diagnostic =
                fs::read(&compiled.stderr_path).ok().map(|bytes| process::text(&bytes)).unwrap_or_default();
            records.push(compiled);
            if !compile_ok {
                (false, format!("candidate compilation failed; {}", core::local_tools::bounded(diagnostic, 4000)))
            } else {
                let (driver, expected) = tasks::build_oracle(self.config.kind);
                let driver_path = directory.join("host-probe.cpp");
                fs::write(&driver_path, driver)?;
                let executable = directory.join(if cfg!(windows) { "probe.exe" } else { "probe" });
                let linked = self
                    .processes
                    .run(
                        "link-independent-probe",
                        compiler,
                        vec![
                            "-std=c++17".into(),
                            "-O0".into(),
                            driver_path.to_string_lossy().into_owned(),
                            object.to_string_lossy().into_owned(),
                            "-I".into(),
                            self.workspace.to_string_lossy().into_owned(),
                            "-o".into(),
                            executable.to_string_lossy().into_owned(),
                        ],
                        &self.workspace,
                        signal.clone(),
                    )
                    .await?;
                let link_ok = linked.success();
                records.push(linked);
                if !link_ok {
                    (false, "independent probe failed to link; preserve the required function signature".into())
                } else {
                    let executed = self
                        .processes
                        .run("execute-independent-probe", &executable, vec![], &self.workspace, signal.clone())
                        .await?;
                    let output = if executed.stdout_present { executed.stdout()? } else { String::new() };
                    let actual = output.lines().map(|line| line.trim().to_owned()).collect::<Vec<_>>();
                    let success = executed.success() && actual == expected;
                    details = json!({"independent_cases":expected.len(),"expected":expected,"actual":actual,"exact_stdout_match":success,"probe_sha256":process::hash(&fs::read(&executable)?)});
                    records.push(executed);
                    (
                        success,
                        if success {
                            "independent executable behavior verified".into()
                        } else {
                            "independent executable behavior differs from the task contract; review boundary, wraparound and unsigned arithmetic as applicable".into()
                        },
                    )
                }
            }
        };
        if process::hash(&fs::read(self.candidate_path())?) != artifact_hash || self.check_immutable().is_err() {
            self.processes.mark_unknown();
            passed = false;
            feedback = "artifact changed during verification; reconcile original effects".into();
        }
        if self.processes.unknown() {
            passed = false;
            feedback = "process effects unknown; do not replay; inspect original process receipts".into();
        }
        let report = directory.join("report.json");
        save_new(
            &report,
            &json!({"kind":self.config.kind,"candidate_path":self.candidate_path(),"candidate_snapshot":snapshot,"candidate_sha256":artifact_hash,"passed":passed,"feedback":feedback,"processes":records,"details":details,"effects_unknown":self.processes.unknown(),"oracle_granted_to_model":false}),
        )?;
        Ok(Evaluation { passed, feedback, report })
    }
}

#[derive(Clone, Copy)]
enum ToolKind {
    Read,
    Write,
    Check,
    Inspect,
}
struct TaskTool {
    kind: ToolKind,
    schema: Tool,
    engine: Arc<Engine>,
}
impl TaskTool {
    async fn perform(&self, args: &Value, signal: CancellationToken) -> Result<Value> {
        let object = args.as_object().context("arguments must be an object")?;
        let properties = match self.kind {
            ToolKind::Read => vec!["path"],
            ToolKind::Write => vec!["path", "content"],
            ToolKind::Check | ToolKind::Inspect => vec![],
        };
        ensure!(
            object.len() == properties.len() && properties.iter().all(|key| object.contains_key(*key)),
            "exact tool fields required"
        );
        match self.kind {
            ToolKind::Read => {
                let name = args["path"].as_str().context("path string required")?;
                let path = self.engine.granted(name)?;
                ensure!(fs::metadata(&path)?.len() <= 64 * 1024, "task file exceeds limit");
                let text = fs::read_to_string(path)?;
                if name == "task.md" {
                    self.engine.read_spec.store(true, Ordering::SeqCst);
                }
                Ok(json!({"path":name,"sha256":process::hash(text.as_bytes()),"text":text}))
            }
            ToolKind::Write => {
                let name = args["path"].as_str().context("path string required")?;
                ensure!(name == tasks::candidate(self.engine.config.kind), "file is not writable");
                let path = self.engine.granted(name)?;
                let content = args["content"].as_str().context("content string required")?;
                ensure!(content.len() <= 64 * 1024, "candidate exceeds limit");
                let before = process::hash(&fs::read(&path)?);
                if fs::write(&path, content).is_err() {
                    self.engine.processes.mark_unknown();
                    anyhow::bail!("write failed; effects unknown");
                }
                let actual = match fs::read(&path) {
                    Ok(actual) if actual == content.as_bytes() => actual,
                    _ => {
                        self.engine.processes.mark_unknown();
                        anyhow::bail!("candidate readback failed; effects unknown");
                    }
                };
                Ok(
                    json!({"path":name,"before_sha256":before,"after_sha256":process::hash(&actual),"submitted":true,"accepted":false}),
                )
            }
            ToolKind::Check => {
                let check = self.engine.evaluate(signal).await?;
                Ok(json!({"passed":check.passed,"feedback":check.feedback,"oracle_disclosed":false}))
            }
            ToolKind::Inspect => {
                self.engine.check_immutable()?;
                let result = self
                    .engine
                    .processes
                    .run(
                        "inspect-granted-object",
                        self.engine.config.objdump.as_deref().context("objdump unavailable")?,
                        vec![
                            "-d".into(),
                            "-r".into(),
                            "-M".into(),
                            "intel".into(),
                            self.engine.workspace.join("mystery.o").to_string_lossy().into_owned(),
                        ],
                        &self.engine.workspace,
                        signal,
                    )
                    .await?;
                ensure!(result.success(), "objdump failed; inspect process receipt");
                let output = result.stdout()?;
                ensure!(!output.trim().is_empty() && output.len() <= 48 * 1024, "objdump output empty or over limit");
                self.engine.inspected_binary.store(true, Ordering::SeqCst);
                Ok(
                    json!({"tool":"GNU objdump","object_sha256":self.engine.object_hash,"disassembly":output,"receipt_sha256":process::hash(&fs::read(&result.receipt_path)?)}),
                )
            }
        }
    }
}
#[async_trait]
impl AgentTool for TaskTool {
    fn schema(&self) -> &Tool {
        &self.schema
    }
    fn label(&self) -> &str {
        &self.schema.name
    }
    async fn execute(
        &self,
        id: &str,
        args: Value,
        signal: CancellationToken,
        _: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        if signal.is_cancelled() {
            return Err(AgentError::Abort);
        }
        if self.engine.processes.unknown() {
            let mut result =
                AgentToolResult::text("effects_unknown: this Host is fenced; no further operations are admitted");
            result.details = json!({"executed":"unknown","effects_unknown":true,"operation_id":id});
            return Ok(result);
        }
        if id.is_empty() || id.len() > 256 {
            return Err(AgentError::Tool("invalid operation ID".into()));
        }
        let key = process::hash(id.as_bytes());
        let intent = self.engine.private.join(format!("operation-{key}-started.json"));
        if save_new(&intent,&json!({"id":id,"tool":self.schema.name,"arguments_sha256":process::hash(args.to_string().as_bytes()),"state":"started"})).is_err() {
            self.engine.processes.mark_unknown();
            let mut result=AgentToolResult::text("effects_unknown: original operation receipt exists or could not be committed; inspect before any replay");
            result.details=json!({"executed":"unknown","effects_unknown":true,"operation_id":id});
            return Ok(result);
        }
        let performed = self.perform(&args, signal).await;
        if self.engine.processes.unknown() {
            let mut result = AgentToolResult::text(
                "effects_unknown: original process or file effects require reconciliation; do not replay",
            );
            result.details = json!({"executed":"unknown","effects_unknown":true,"operation_id":id});
            return Ok(result);
        }
        match performed {
            Ok(value) => {
                if save_new(
                    &self.engine.private.join(format!("operation-{key}-completed.json")),
                    &json!({"id":id,"tool":self.schema.name,"result":value}),
                )
                .is_err()
                {
                    self.engine.processes.mark_unknown();
                    let mut result = AgentToolResult::text(
                        "effects_unknown: tool result persistence failed; inspect original artifacts",
                    );
                    result.details = json!({"executed":"unknown","effects_unknown":true,"operation_id":id});
                    return Ok(result);
                }
                Ok(AgentToolResult::text(value.to_string()))
            }
            Err(error) => {
                let _ = save_new(
                    &self.engine.private.join(format!("operation-{key}-failed.json")),
                    &json!({"id":id,"tool":self.schema.name,"error":error.to_string(),"replay":false}),
                );
                Err(AgentError::Tool(error.to_string()))
            }
        }
    }
}
