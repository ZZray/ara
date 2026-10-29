//! Actual request values share the one neutral heap/factory. These are request
//! projections, not a second transcript or canonical message owner.
use crate::agent_message::GraphOwnerError;
use ara_ai::{ActualAssistantStream, graph_values::GraphValueFactory};
use ara_rpc::{
    WireString,
    value_heap::{ValueError, ValueHandle, ValueHeap, ValueRuntime},
};
use std::{fmt, sync::Arc};

pub struct SessionValues {
    factory: Arc<GraphValueFactory>,
}
impl fmt::Debug for SessionValues {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("SessionValues").field("heap_token", &self.heap().heap_token()).finish()
    }
}
impl SessionValues {
    pub fn new(heap: Arc<ValueHeap>, runtime: Arc<dyn ValueRuntime>) -> Result<Self, ValueError> {
        Ok(Self { factory: Arc::new(GraphValueFactory::new(heap, runtime)?) })
    }
    pub fn factory(&self) -> &Arc<GraphValueFactory> {
        &self.factory
    }
    pub fn heap(&self) -> &Arc<ValueHeap> {
        self.factory.heap()
    }
    pub fn runtime(&self) -> &dyn ValueRuntime {
        self.factory.runtime()
    }
    pub fn check_handle(&self, value: &ValueHandle) -> Result<(), ValueError> {
        if self.heap().heap_token() != value.heap().heap_token() {
            return Err(ValueError::ForeignHeap);
        }
        self.heap().kind(value).map(|_| ())
    }
    pub fn same_lineage(&self, other: &Self) -> bool {
        self.heap().heap_token() == other.heap().heap_token()
    }
}

#[derive(Clone, Debug)]
pub enum ActualProviderError {
    Value(ValueError),
    Owner(GraphOwnerError),
    Unsupported(&'static str),
    PreparationUnavailable,
    Protocol(&'static str),
    Callback(String),
    Compound { primary: Arc<ActualProviderError>, secondary: Arc<ActualProviderError> },
}
impl From<ValueError> for ActualProviderError {
    fn from(value: ValueError) -> Self {
        Self::Value(value)
    }
}
impl From<GraphOwnerError> for ActualProviderError {
    fn from(value: GraphOwnerError) -> Self {
        Self::Owner(value)
    }
}
impl fmt::Display for ActualProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Value(error) => error.fmt(formatter),
            Self::Owner(error) => error.fmt(formatter),
            Self::Unsupported(domain) => write!(formatter, "Actual request domain is not connected: {domain}"),
            Self::PreparationUnavailable => {
                formatter.write_str("Actual Graph tool preparation is not connected (R2-B required)")
            }
            Self::Protocol(reason) => write!(formatter, "Actual stream protocol: {reason}"),
            Self::Callback(reason) => write!(formatter, "Actual request callback: {reason}"),
            Self::Compound { primary, secondary } => {
                write!(formatter, "{primary}; subsequent failure: {secondary}")
            }
        }
    }
}
impl std::error::Error for ActualProviderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Value(error) => Some(error),
            Self::Owner(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ActualProviderContext {
    /// Original request Context after Native hooks, in the same Session heap.
    /// Core/Host owns construction and replacement. Derived Vec fields cannot
    /// stand in for its messages/tools/systemPrompt arrays.
    pub original_context: Option<ValueHandle>,
    pub system_prompt: Vec<WireString>,
    pub messages: Vec<ValueHandle>,
    pub tools: Option<Arc<[ValueHandle]>>,
}
impl ActualProviderContext {
    pub fn check_lineage(&self, values: &SessionValues) -> Result<(), ActualProviderError> {
        if let Some(original) = &self.original_context {
            values.check_handle(original)?;
        }
        for value in self.messages.iter().chain(self.tools.iter().flat_map(|tools| tools.iter())) {
            values.check_handle(value)?;
        }
        Ok(())
    }
}

pub(crate) enum PreparedRequestContext {
    Typed(ara_ai::Context),
    Actual { context: ActualProviderContext, values: Arc<SessionValues> },
}
pub(crate) enum PreparedAssistantStream {
    Typed(ara_ai::AssistantStream),
    Actual { stream: ActualAssistantStream },
}
#[allow(clippy::large_enum_variant)]
pub(crate) enum PreparedStreamEvent {
    Typed(ara_ai::AssistantMessageEvent),
    Actual(ara_ai::ActualAssistantEvent),
}
impl PreparedAssistantStream {
    pub async fn recv(&mut self) -> Option<PreparedStreamEvent> {
        match self {
            Self::Typed(stream) => stream.recv().await.map(PreparedStreamEvent::Typed),
            Self::Actual { stream, .. } => stream.recv().await.map(PreparedStreamEvent::Actual),
        }
    }
}
