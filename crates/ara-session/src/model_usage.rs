//! Fixed OMP 596f2da session-entries.ts:79-92 and
//! session-manager.ts:2324-2345. Usage belongs to the initiating branch,
//! independently of where the Host later installs a classifier result.
//! MIT; see THIRD_PARTY_NOTICES.md.
//! Copyright (c) 2025 Mario Zechner
//! Copyright (c) 2025-2026 Can Bölük
//! Copyright (c) 2026 Stencil Labs, Inc.

use ara_ai::{StopReason, Usage};
use serde::{Deserialize, Serialize};

use crate::{Result, SessionJournal};

/// Captured before a side call. The Host retains the initiating journal too.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelUsageOwner {
    pub session_id: String,
    pub parent_id: Option<String>,
}

/// An observed completion attempt, never a conversation assistant message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsageRecord {
    pub purpose: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub api: String,
    pub provider: String,
    pub model: String,
    pub usage: Usage,
    pub stop_reason: StopReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

impl SessionJournal {
    pub fn model_usage_owner(&self) -> ModelUsageOwner {
        ModelUsageOwner { session_id: self.session_id().to_owned(), parent_id: self.leaf.clone() }
    }

    /// Native stale-owner rejection does not touch IDs, entries or bytes.
    /// On success the caller can chain subsequent attempts to the returned ID.
    /// A successor active branch is preserved even on persistence failure.
    pub fn append_model_usage(&mut self, record: &ModelUsageRecord, owner: &ModelUsageOwner) -> Result<Option<String>> {
        if self.session_id() != owner.session_id
            || owner.parent_id.as_ref().is_some_and(|parent| !self.ids.contains(parent))
        {
            return Ok(None);
        }
        let fields = serde_json::to_value(record).expect("model usage serializes").as_object().unwrap().clone();
        let active_leaf = self.leaf.clone();
        self.leaf = owner.parent_id.clone();
        let result = self.append_raw("model_usage", fields);
        if active_leaf != owner.parent_id {
            self.leaf = active_leaf;
        }
        result.map(Some)
    }

    /// Includes late, off-branch attempts. They were observed on this Session,
    /// even when their result was superseded and never entered model context.
    pub fn model_usage_records(&self) -> impl Iterator<Item = ModelUsageRecord> + '_ {
        self.entries
            .iter()
            .filter(|entry| entry.kind == "model_usage")
            .filter_map(|entry| serde_json::from_value(entry.raw.clone()).ok())
    }
}
