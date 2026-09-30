//! Narrow REPL Skill host adapter. Core sees the user projection; Session
//! and host events retain the original Skill custom message.

use super::{HostSink, Mode};
use ara_agent::{AgentEvent, AgentEventSink};
use ara_ai::{Message, UserContent};
use ara_context::build_skill_prompt;
use ara_discovery::{LoadedSkill, parse_skill_invocation};
use ara_session::UserSkillPrompt;
use async_trait::async_trait;
use serde_json::json;
use std::{
    future::pending,
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
pub enum PreparationError {
    Load(String),
    Cancelled,
    Deadline,
}

/// No second discovery pass or input-derived filesystem path. A known Skill
/// is read off the runtime worker, within this turn's cancellation/deadline.
pub async fn prepare(
    input: &str,
    original: &str,
    skills: &[LoadedSkill],
    cancel: &CancellationToken,
    deadline: Option<Instant>,
) -> Result<Option<UserSkillPrompt>, PreparationError> {
    let Some(invocation) = parse_skill_invocation(input) else { return Ok(None) };
    let Some(skill) = skills.iter().find(|skill| skill.name == invocation.name).cloned() else { return Ok(None) };
    if cancel.is_cancelled() {
        return Err(PreparationError::Cancelled);
    }
    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
        return Err(PreparationError::Deadline);
    }
    let name = skill.name.clone();
    let mut read = tokio::task::spawn_blocking(move || build_skill_prompt(&skill, &invocation.args));
    let deadline_wait = async {
        match deadline {
            Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
            None => pending::<()>().await,
        }
    };
    let built = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(PreparationError::Cancelled),
        _ = deadline_wait => return Err(PreparationError::Deadline),
        built = &mut read => built,
    };
    // Dropping the JoinHandle stops admission of its result, not the OS read.
    if cancel.is_cancelled() {
        return Err(PreparationError::Cancelled);
    }
    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
        return Err(PreparationError::Deadline);
    }
    let mut built = built
        .map_err(|error| PreparationError::Load(format!("{name}: {error}")))?
        .map_err(|error| PreparationError::Load(format!("{name}: {error}")))?;
    built.details["originalText"] = json!(original);
    Ok(Some(UserSkillPrompt::new(UserContent::Text(built.message), Some(built.details))))
}

/// One adapter per turn: only its initial projected prompt is persisted as
/// custom. Subsequent messages, tool receipts and Run outcomes use HostSink.
pub struct SkillPromptSink<'a> {
    pub host: &'a HostSink,
    pub prompt: &'a UserSkillPrompt,
    pub projected: &'a Message,
    pub recorded: AtomicBool,
}

impl SkillPromptSink<'_> {
    fn public_event(&self, event: &AgentEvent) -> serde_json::Value {
        // Redact provider payloads/signatures before adapting host provenance.
        let mut public = event.printable();
        match event {
            AgentEvent::MessageStart { .. } | AgentEvent::MessageEnd { .. } => {
                public["message"] = self.prompt.event_message();
            }
            AgentEvent::AgentEnd { .. } => public["messages"][0] = self.prompt.event_message(),
            _ => {}
        }
        public
    }
}

#[async_trait]
impl AgentEventSink for SkillPromptSink<'_> {
    async fn emit(&self, event: AgentEvent) {
        let initial = match &event {
            AgentEvent::MessageStart { message } | AgentEvent::MessageEnd { message } => {
                message == self.projected && !self.recorded.load(Ordering::SeqCst)
            }
            _ => false,
        };
        if initial {
            if matches!(event, AgentEvent::MessageEnd { .. })
                && !self.recorded.swap(true, Ordering::SeqCst)
                && let Some(journal) = self.host.journal.lock().await.as_mut()
                && let Err(error) = journal.append_skill_prompt(self.prompt)
            {
                self.host.persistence_failure(&error);
            }
            if self.host.stream {
                self.host.stream_progress(&event);
            }
            if self.host.mode == Mode::Json {
                self.host.write_line(&self.public_event(&event).to_string());
            }
            return;
        }
        if let AgentEvent::AgentEnd { messages } = &event
            && messages.first() == Some(self.projected)
            && self.host.mode == Mode::Json
        {
            self.host.write_line(&self.public_event(&event).to_string());
            return;
        }
        self.host.emit(event).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_ai::{AssistantBlock, AssistantMessage, ThinkingContent};
    use ara_discovery::{Level, SourceMeta};

    #[tokio::test]
    async fn cancelled_and_expired_preparation_never_admit_a_skill_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing-SKILL.md");
        let skill = LoadedSkill {
            name: "proof".into(),
            description: "fixture".into(),
            file_path: path.clone(),
            base_dir: dir.path().into(),
            source: "test:project".into(),
            hide: false,
            meta: SourceMeta::new("test", &path, Level::Project),
        };
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(matches!(
            prepare("/skill:proof", "raw", std::slice::from_ref(&skill), &cancel, None).await,
            Err(PreparationError::Cancelled)
        ));
        assert!(matches!(
            prepare("/skill:proof", "raw", &[skill], &CancellationToken::new(), Some(Instant::now())).await,
            Err(PreparationError::Deadline)
        ));
    }

    #[test]
    fn custom_agent_end_preserves_provider_payload_redaction() {
        let cancel = CancellationToken::new();
        let host = HostSink::new(Mode::Json, false, None, cancel, ara_edit::EditMode::Hashline);
        let prompt =
            UserSkillPrompt::new(UserContent::Text("body".into()), Some(json!({"originalText":"/skill:proof\n"})));
        let projected = prompt.model_message();
        let sink =
            SkillPromptSink { host: &host, prompt: &prompt, projected: &projected, recorded: AtomicBool::new(true) };
        let mut assistant = AssistantMessage::empty("openai-responses", "fixture", "model");
        assistant.provider_payload = Some(json!({"native":"private provider data"}));
        assistant.content.push(AssistantBlock::Thinking(ThinkingContent {
            thinking: "public thought".into(),
            thinking_signature: Some("private signature".into()),
        }));
        let event = AgentEvent::AgentEnd { messages: vec![projected.clone(), Message::Assistant(assistant)] };
        let value = sink.public_event(&event);
        assert_eq!(value["messages"][0]["role"], "custom");
        assert_eq!(value["messages"][0]["attribution"], "user");
        assert_eq!(value["messages"][1]["content"][0]["thinking"], "public thought");
        assert!(value["messages"][1].get("providerPayload").is_none());
        assert!(value["messages"][1]["content"][0].get("thinkingSignature").is_none());
    }
}
