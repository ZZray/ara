//! Host-owned immutable display observations. Fixed OMP 596f2da (MIT).
//! Original native handles remain distinct from the derived display DTO.
use crate::native_factory::PreparedNativeSession;
use ara_agent::AgentMessage;
use ara_session::context_view::TranscriptView;
use std::{collections::HashMap, sync::Arc};

pub struct SessionTranscriptObservation {
    pub owner: Option<Arc<PreparedNativeSession>>,
    pub session_id: String,
    pub revision: u64,
    pub collapsed: bool,
    pub view: TranscriptView,
    pub originals: HashMap<String, AgentMessage>,
}
impl SessionTranscriptObservation {
    pub(crate) fn same_owner(&self, owner: Option<&Arc<PreparedNativeSession>>) -> bool {
        match (self.owner.as_ref(), owner) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
    }
}
