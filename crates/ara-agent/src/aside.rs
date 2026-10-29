//! Passive aside entries and ephemeral claim callbacks. Fixed OMP types.ts
//! 34-50 and agent-loop.ts 1008-1027 at 596f2da (MIT, THIRD_PARTY_NOTICES.md).

use crate::agent_message::AgentMessage;
use async_trait::async_trait;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AsideError(pub String);

impl std::fmt::Display for AsideError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for AsideError {}

pub type AsideCommit = Arc<dyn Fn() -> Result<(), AsideError> + Send + Sync>;
pub type AsideDiscard = Arc<dyn Fn(&AsideError) -> Result<(), AsideError> + Send + Sync>;
pub type AsideFactory = Arc<dyn Fn() -> Result<Option<AgentMessage>, AsideError> + Send + Sync>;

#[derive(Clone, Default)]
pub struct AsideCallbacks {
    pub commit: Option<AsideCommit>,
    pub discard: Option<AsideDiscard>,
}

#[derive(Clone)]
pub enum AsideMessage {
    Ready(AgentMessage),
    Lazy(AsideFactory),
}

impl From<AgentMessage> for AsideMessage {
    fn from(message: AgentMessage) -> Self {
        Self::Ready(message)
    }
}

#[async_trait]
pub trait AsideMessageProvider: Send + Sync {
    async fn messages(&self) -> Result<Vec<AsideMessage>, AsideError>;
}

#[async_trait]
pub trait BeforeYieldHook: Send + Sync {
    async fn before_yield(&self) -> Result<(), AsideError>;
}

pub(crate) fn callback<T>(stage: &str, callback: impl FnOnce() -> Result<T, AsideError>) -> Result<T, AsideError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(callback)).unwrap_or_else(|payload| {
        let message = payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|value| (*value).to_string()))
            .unwrap_or_else(|| "non-string panic".into());
        Err(AsideError(format!("Aside {stage} panicked: {message}")))
    })
}

pub(crate) fn discard_asides(messages: &[AgentMessage], reason: &AsideError) -> Result<(), AsideError> {
    // Native propagates the first discard failure and stops this callback loop.
    for message in messages {
        message.discard_aside(reason)?;
    }
    Ok(())
}

pub(crate) fn resolve_asides(entries: Vec<AsideMessage>) -> Result<Vec<AgentMessage>, AsideError> {
    let mut resolved = Vec::new();
    for entry in entries {
        let result = match entry {
            AsideMessage::Ready(message) => Ok(Some(message)),
            AsideMessage::Lazy(factory) => callback("resolver", || factory()),
        };
        match result {
            Ok(Some(message)) => resolved.push(message),
            Ok(None) => {}
            Err(error) => {
                // Only already resolved non-null records have become claims.
                discard_asides(&resolved, &error)?;
                return Err(error);
            }
        }
    }
    Ok(resolved)
}
