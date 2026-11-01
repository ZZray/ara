//! A restricted reference adapter: one result file, explicit source windows and
//! durable human questions. Credentials are resolved only from the environment.
use crate::{
    journal::Journal,
    runtime::{Adapter, Prepared},
    types::*,
};
use anyhow::{ensure, Context, Result};
use ara_agent_lite_harness::{
    core::{
        self,
        model::ModelHost,
        rpi_agent::{AgentError, AgentTool, AgentToolResult, ToolResultPartial},
        rpi_ai::{Schema, Tool},
        RunMode, RunRequest, RuntimePaths, ToolRegistration,
    },
    *,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelConfig {
    pub endpoint: String,
    pub model: String,
    pub key_env: String,
    pub expected_result: String,
    pub max_run_calls: usize,
    pub max_output: u32,
}
pub struct ReferenceAdapter(pub ModelConfig);
impl Adapter for ReferenceAdapter {
    fn revision(&self) -> String {
        format!("reference-v1:{:x}", Sha256::digest(serde_json::to_vec(&self.0).expect("model config serializes")))
    }
    fn prepare(
        &self,
        binding: &ExecutionBinding,
        task: &ara_lite::types::Task,
        journal: Arc<Journal>,
    ) -> Result<Prepared> {
        let token = std::env::var(&self.0.key_env).context("configured credential environment variable missing")?;
        ensure!(
            !token.is_empty() && self.0.max_run_calls > 0 && self.0.max_output > 0,
            "invalid reference model configuration"
        );
        let steering = Arc::new(Mutex::new(core::steering::SteeringBuffer::default()));
        let model_host: Arc<dyn ModelHost> = Arc::new(core::model::CatalogModelHost);
        let tools: Vec<Arc<dyn AgentTool>> = vec![
            Arc::new(HostTool::new(
                "write_result",
                "Write UTF-8 text to this Task's result.txt only.",
                json!({"text":{"type":"string"}}),
                vec!["text"],
                binding,
                journal.clone(),
            )),
            Arc::new(HostTool::new(
                "request_input",
                "Ask one concrete question. Host waits for an explicit answer without model polling.",
                json!({"question":{"type":"string"}}),
                vec!["question"],
                binding,
                journal.clone(),
            )),
            Arc::new(HostTool::new(
                "source_list",
                "List explicitly authorized source IDs and versions, without full content.",
                json!({}),
                vec![],
                binding,
                journal.clone(),
            )),
            Arc::new(HostTool::new(
                "source_read",
                "Read an authorized, hash-checked UTF-8 source window (at most 16384 bytes).",
                json!({"id":{"type":"string"},"version":{"type":"integer"},"offset":{"type":"integer"},"limit":{"type":"integer"}}),
                vec!["id", "version", "offset", "limit"],
                binding,
                journal.clone(),
            )),
            Arc::new(HostTool::new(
                "source_search",
                "Locate a literal in one authorized current source, returning its byte offset.",
                json!({"id":{"type":"string"},"version":{"type":"integer"},"query":{"type":"string"}}),
                vec!["id", "version", "query"],
                binding,
                journal.clone(),
            )),
        ];
        let host = Arc::new(ReferenceHost {
            config: self.0.clone(),
            binding: binding.clone(),
            objective: task.instruction.clone(),
            token,
            steering,
            model_host,
            tools,
        });
        let request = host.request(
            if binding.attempt == 0 {
                binding.initial_run_id.clone()
            } else {
                format!("resume-{}-{}", task.id, binding.attempt)
            },
            task.instruction.clone(),
        );
        Ok(Prepared { request, host })
    }
}

struct ReferenceHost {
    config: ModelConfig,
    binding: ExecutionBinding,
    objective: String,
    token: String,
    steering: Arc<Mutex<core::steering::SteeringBuffer>>,
    model_host: Arc<dyn ModelHost>,
    tools: Vec<Arc<dyn AgentTool>>,
}
impl ReferenceHost {
    fn request(&self, id: String, objective: String) -> RunRequest {
        let mut request = RunRequest::new(
            id,
            objective,
            RuntimePaths::new(&self.binding.workspace, &self.binding.session_path, &self.binding.environment_path),
        );
        request.endpoint = self.config.endpoint.clone();
        request.token = self.token.clone();
        request.model = self.config.model.clone();
        request.route.model = self.config.model.clone();
        request.route.protocol = "openai-completions".into();
        request.model_host = self.model_host.clone();
        request.steering = self.steering.clone();
        request.mode = RunMode::Execute;
        request.max_calls = self.config.max_run_calls.min(self.binding.spec.max_calls);
        request.max_seconds = 3600;
        request.max_output = self.config.max_output;
        request.enable_local_tools = false;
        request.extra_tools =
            self.tools.iter().map(|tool| ToolRegistration { tool: tool.clone(), allow_in_plan: false }).collect();
        request
    }
}
#[async_trait]
impl HarnessHost for ReferenceHost {
    async fn verify(&self, candidate: Candidate, _: CancellationToken) -> Result<ReviewReceipt> {
        let path = self.binding.workspace.join("result.txt");
        ensure!(std::fs::metadata(&path)?.len() <= 65536, "artifact exceeds declared limit");
        let bytes = std::fs::read(path).context("no actual result.txt artifact")?;
        let correct = bytes == self.config.expected_result.as_bytes();
        let proof = self.binding.spec.evidence_dir.join(format!("artifact-{}.json", candidate.basis.run_number));
        let report = json!({"basis":candidate.basis,"actual_sha256":format!("{:x}",Sha256::digest(&bytes)),"passed":correct,"check":"exact trusted reference artifact contract"});
        let mut file = std::fs::File::create(&proof)?;
        std::io::Write::write_all(&mut file, &serde_json::to_vec(&report)?)?;
        file.sync_all()?;
        Ok(ReviewReceipt {
            basis: candidate.basis,
            committed: true,
            settlement: Settlement::Settled,
            verdict: if correct {
                Verdict::Verified
            } else {
                Verdict::NeedsChanges{feedback:"The actual result file does not meet the Host contract; recheck original sources and correct it.".into()}
            },
            evidence: vec![proof.display().to_string()],
        })
    }
    async fn prepare_continuation(&self, next: Continuation, _: CancellationToken) -> Result<RunRequest> {
        let Verdict::NeedsChanges { feedback } = next.receipt.verdict else {
            anyhow::bail!("continuation requires actual review feedback")
        };
        Ok(self.request(
            format!("continue-{}-{}", self.binding.spec.task_id, next.next_run_number),
            format!("{}\nHost feedback: {}", self.objective, feedback),
        ))
    }
    async fn reconcile_resume(&self, review: ResumeReview, _: CancellationToken) -> Result<ResumeReceipt> {
        let path = self.binding.session_path.parent().context("no task state parent")?.join("reconciliation.json");
        let proof: Reconciliation =
            serde_json::from_slice(&std::fs::read(&path).context("explicit operator reconciliation is required")?)?;
        ensure!(
            proof.task_id == review.task_id
                && proof.generation == self.binding.generation
                && proof.session_path == review.session_path,
            "reconciliation identity mismatch"
        );
        ensure!(
            proof.fence_sha256 == format!("{:x}", Sha256::digest(serde_json::to_vec(&review.fence)?)),
            "reconciliation fence changed"
        );
        ensure!(!proof.evidence.is_empty(), "operator settlement evidence required");
        Ok(ResumeReceipt {
            committed: true,
            settlement: Settlement::Settled,
            tool_calls: proof.tool_calls,
            evidence: vec![path.display().to_string()].into_iter().chain(proof.evidence).collect(),
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reconciliation {
    pub task_id: String,
    pub generation: i64,
    pub session_path: PathBuf,
    pub fence_sha256: String,
    pub tool_calls: Vec<ToolCallReconciliation>,
    pub evidence: Vec<String>,
}

struct HostTool {
    schema: Tool,
    binding: ExecutionBinding,
    journal: Arc<Journal>,
}
impl HostTool {
    fn new(
        name: &str,
        description: &str,
        properties: Value,
        required: Vec<&str>,
        binding: &ExecutionBinding,
        journal: Arc<Journal>,
    ) -> Self {
        Self {
            schema: Tool {
                name: name.into(),
                description: description.into(),
                parameters: Schema(
                    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}),
                ),
                constrained_sampling: None,
            },
            binding: binding.clone(),
            journal,
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TextInput {
    text: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QuestionInput {
    question: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadInput {
    id: String,
    version: u64,
    offset: usize,
    limit: usize,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchInput {
    id: String,
    version: u64,
    query: String,
}

#[async_trait]
impl AgentTool for HostTool {
    fn schema(&self) -> &Tool {
        &self.schema
    }
    fn label(&self) -> &str {
        &self.schema.name
    }
    async fn execute(
        &self,
        call_id: &str,
        args: Value,
        cancel: CancellationToken,
        _: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> std::result::Result<AgentToolResult, AgentError> {
        let operation=async {
            ensure!(!cancel.is_cancelled(),"tool canceled before invocation");
            let current=self.journal.binding(&self.binding.spec.task_id)?.context("Task binding disappeared")?;
            ensure!(current.generation==self.binding.generation && current.spec.authorization_revision==self.binding.spec.authorization_revision,"tool authority changed");
            let task=&self.binding.spec.task_id;
            let value=match self.schema.name.as_str(){
                "write_result"=>{let input:TextInput=serde_json::from_value(args)?;ensure!(!input.text.is_empty()&&input.text.len()<=65536,"invalid artifact size");
                    let target=self.binding.workspace.join("result.txt");
                    match std::fs::symlink_metadata(&target){
                        Ok(meta)=>ensure!(meta.is_file()&&!meta.file_type().is_symlink(),"result.txt must be a regular non-symlink file"),
                        Err(error) if error.kind()==std::io::ErrorKind::NotFound=>{},Err(error)=>return Err(error.into()),
                    }
                    let intent=self.binding.session_path.parent().context("no Task state parent")?.join(format!("write-{:x}.json",Sha256::digest(call_id.as_bytes())));
                    let write=(||->Result<Value>{
                        let mut receipt=std::fs::OpenOptions::new().write(true).create_new(true).open(&intent)?;
                        std::io::Write::write_all(&mut receipt,&serde_json::to_vec(&json!({"call_id":call_id,"sha256":format!("{:x}",Sha256::digest(input.text.as_bytes())),"state":"dispatching"}))?)?;receipt.sync_all()?;
                        let temporary=self.binding.workspace.join(format!(".ara-result-{}.tmp",core::fresh_id()));
                        let mut file=std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary)?;
                        std::io::Write::write_all(&mut file,input.text.as_bytes())?;file.sync_all()?;
                        drop(file);std::fs::rename(&temporary,&target)?;
                        let mut receipt=std::fs::File::create(&intent)?;
                        std::io::Write::write_all(&mut receipt,&serde_json::to_vec(&json!({"call_id":call_id,"sha256":format!("{:x}",Sha256::digest(input.text.as_bytes())),"state":"completed"}))?)?;receipt.sync_all()?;
                        Ok(json!({"written":input.text.len()}))
                    })();
                    match write{Ok(value)=>value,Err(error)=>{
                        let mut result=AgentToolResult::text(format!("Artifact write did not settle: {error:#}"));
                        result.details=json!({"executed":"unknown","effects_unknown":true,"receipt":intent});
                        return Ok(result);
                    }}},
                "request_input"=>{let input:QuestionInput=serde_json::from_value(args)?;ensure!(!input.question.trim().is_empty()&&input.question.len()<=4096,"invalid question");
                    let id=format!("question:{:x}",Sha256::digest(format!("{task}:{call_id}").as_bytes()));
                    self.journal.put_attention(&Attention{id:id.clone(),task_id:task.clone(),generation:self.binding.generation,question:input.question.clone(),answer:None})?;
                    let mut waiting=current;waiting.phase=ExecutionPhase::WaitingHuman;self.journal.put_binding(&waiting)?;
                    self.journal.put_notice(&Notice{id:id.clone(),task_id:task.clone(),generation:self.binding.generation,kind:"question".into(),body:input.question,state:NoticeState::Pending})?;
                    loop{let attention=self.journal.attention(&id)?.context("question disappeared")?;if let Some(answer)=attention.answer{waiting.phase=ExecutionPhase::Running;self.journal.put_binding(&waiting)?;break json!({"question_id":id,"answer":answer});}
                        tokio::select!{_=cancel.cancelled()=>anyhow::bail!("question canceled; original record retained"),_=tokio::time::sleep(Duration::from_millis(50))=>{}}}
                },
                "source_list"=>{ensure!(args.as_object().is_some_and(|args|args.is_empty()),"source_list takes no arguments");json!(self.journal.sources()?.into_iter().filter(|source|source.active&&(source.task_id==*task||source.allowed_tasks.contains(task))).map(|source|json!({"id":source.id,"version":source.version,"sha256":source.sha256,"owner":source.task_id})).collect::<Vec<_>>())},
                "source_read"=>{let input:ReadInput=serde_json::from_value(args)?;json!(crate::evidence::read(&self.journal,task,self.binding.generation,&input.id,input.version,input.offset,input.limit)?)},
                "source_search"=>{let input:SearchInput=serde_json::from_value(args)?;json!({"offset":crate::evidence::search(&self.journal,task,self.binding.generation,&input.id,input.version,&input.query)?})},
                _=>anyhow::bail!("unknown reference tool"),
            };
            Ok::<_,anyhow::Error>(AgentToolResult::text(serde_json::to_string(&value)?))
        }.await;
        operation.map_err(|error| AgentError::Tool(format!("{error:#}")))
    }
}
