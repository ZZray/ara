#![allow(dead_code)]
use ara_agent::{AgentConfig, AgentInput, AgentTool, InterruptMode, NoHooks};
use ara_ai::{Message, Model, OpenAICompletionsProvider, UserMessage};
use ara_testkit::{FakeUpstream, Script};
use ara_workflow::*;
use serde_json::{Value, json};
use std::{
    num::NonZeroUsize,
    sync::Arc,
    time::{Duration, Instant},
};

pub async fn server(responses: Vec<Value>) -> FakeUpstream {
    let script: Script = serde_json::from_value(json!({"responses":responses})).unwrap();
    FakeUpstream::start(script, None).await.unwrap()
}

pub fn text(finish: &str) -> Value {
    use ara_testkit::chunks;
    json!({"events":[chunks::text("candidate"), chunks::finish(finish), chunks::done()]})
}

pub fn write(content: &str) -> Value {
    use ara_testkit::chunks;
    json!({"events":[chunks::tool_call(0,&format!("write-{content}"),"write",&json!({"path":"answer.txt","content":content}).to_string()), chunks::finish("tool_calls"),chunks::done()]})
}

pub fn config(server: &FakeUpstream, tools: Vec<Arc<dyn AgentTool>>) -> AgentConfig {
    AgentConfig {
        model: Model {
            id: "controlled".into(),
            api: "openai-completions".into(),
            provider: "fixture".into(),
            base_url: server.base_url(),
            reasoning: false,
            max_tokens: Some(256),
            context_window: None,
            tokenizer: None,
        },
        provider: Arc::new(OpenAICompletionsProvider { client: reqwest::Client::new(), base: Default::default() }),
        system_prompt: vec!["Controlled workflow test".into()],
        tools,
        tool_choice: None,
        max_tokens: Some(256),
        temperature: None,
        deadline: None,
        max_model_calls: Some(8),
        hooks: Arc::new(NoHooks),
        interrupt_mode: InterruptMode::Immediate,
    }
}

pub fn basis() -> WorkflowBasis {
    WorkflowBasis {
        workflow_id: uuid::Uuid::new_v4().to_string(),
        session_id: "session".into(),
        session_generation: "generation-1".into(),
        criteria_revision: "criteria-1".into(),
        artifact_evidence_basis: "exclusive-artifacts-1".into(),
    }
}
pub fn request() -> WorkflowRequest {
    WorkflowRequest {
        basis: basis(),
        inputs: vec![AgentInput::from(Message::User(UserMessage::text("produce answer")))],
    }
}
pub fn policy(attempts: usize, requests: usize) -> WorkflowPolicy {
    WorkflowPolicy {
        max_attempts: NonZeroUsize::new(attempts).unwrap(),
        max_prepared_requests: NonZeroUsize::new(requests).unwrap(),
        deadline: Instant::now() + Duration::from_secs(10),
    }
}
pub fn receipt(basis: AttemptBasis, verdict: ReviewVerdict) -> ReviewReceipt {
    ReviewReceipt {
        basis,
        artifacts: vec![ArtifactEvidence {
            artifact_id: "answer".into(),
            revision: "v1".into(),
            evidence_ref: "fixture:checked".into(),
        }],
        settlement: Settlement::Settled,
        commit: ReceiptCommit::Committed { receipt_id: "fixture-receipt".into() },
        verdict,
    }
}
